use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use restoric::app::App;
use restoric::cache::Cache;
use restoric::config::Config;
use restoric::disk::RealDisk;
use restoric::index::timeline::{Filter, explain_empty, timeline_set};
use restoric::index::{Index, Mode};
use restoric::repo::Repo;
use restoric::repo::fake::FakeRepo;
use restoric::repo::rustic::{BACKEND, OpenOptions, RusticRepo};
use restoric::ui::theme::Theme;
use restoric::worker::{Ctx, Worker};

/// Browse and restore a restic repository by time, anchored on a folder.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Folder to open (default: the current folder)
    path: Option<PathBuf>,
    #[command(flatten)]
    repo: RepoArgs,
    #[command(flatten)]
    view: ViewArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Print the change points of PATH
    Log {
        path: PathBuf,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Try it on a sample project, without a repository
    Demo,
    /// Print the distinct versions of FILE
    Versions {
        file: PathBuf,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct RepoArgs {
    /// Repository, as for restic
    #[arg(short, long, global = true, env = "RESTIC_REPOSITORY")]
    repo: Option<String>,
    /// File holding the repository location
    #[arg(long, global = true, env = "RESTIC_REPOSITORY_FILE")]
    repository_file: Option<PathBuf>,
    /// File holding the password
    #[arg(long, global = true, env = "RESTIC_PASSWORD_FILE")]
    password_file: Option<PathBuf>,
    /// Command that prints the password
    #[arg(long, global = true, env = "RESTIC_PASSWORD_COMMAND")]
    password_command: Option<String>,
    /// Open a repository that has no password (restic's --insecure-no-password)
    #[arg(long, global = true)]
    insecure_no_password: bool,
}

#[derive(Args)]
struct ViewArgs {
    /// Hostname whose snapshots to show (default: this machine's); repeat for several
    #[arg(long, global = true)]
    host: Vec<String>,
    /// Only snapshots with this tag
    #[arg(long, global = true)]
    tag: Option<String>,
    /// Count any metadata change, not only content, mode and owner
    #[arg(long, global = true)]
    strict: bool,
    /// Plain icons instead of Nerd Font glyphs
    #[arg(long, global = true)]
    no_icons: bool,
    /// Start with NAME selected
    #[arg(long, value_name = "NAME")]
    select: Option<String>,
    /// Start at a date (as :sep 1, :2026-09-01, :3d)
    #[arg(long, value_name = "DATE")]
    at: Option<String>,
}

fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dirs = directories::ProjectDirs::from("", "", "restoric")?;
    let dir = dirs.state_dir().unwrap_or(dirs.data_local_dir());
    std::fs::create_dir_all(dir).ok()?;
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(dir, "restoric.log"));
    let filter = tracing_subscriber::EnvFilter::try_from_env("RESTORIC_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Some(guard)
}

fn open(args: &RepoArgs, view: &ViewArgs, config: &Config) -> Result<(Index, Filter)> {
    let repo = RusticRepo::open(&OpenOptions {
        repo: args.repo.clone(),
        repo_file: args.repository_file.clone(),
        password: std::env::var("RESTIC_PASSWORD").ok(),
        password_file: args.password_file.clone(),
        password_command: args.password_command.clone(),
        no_password: args.insecure_no_password,
        cache_dir: None,
    })?;
    let cache = match Cache::default_path(repo.id()) {
        Some(path) => Cache::open(&path, BACKEND, config.disk_cache()).unwrap_or_else(|e| {
            tracing::warn!("cache unavailable, using memory: {e:#}");
            Cache::in_memory()
        }),
        None => Cache::in_memory(),
    };
    let mode = if view.strict {
        Mode::Strict
    } else {
        Mode::Content
    };
    let index = Index::new(Arc::new(repo), cache, mode, config.memory_cache());
    // --host, then the config, then this machine's name.
    let hosts = if !view.host.is_empty() {
        view.host.clone()
    } else if !config.hosts().is_empty() {
        config.hosts()
    } else {
        vec![gethostname::gethostname().to_string_lossy().into_owned()]
    };
    let tag = view
        .tag
        .clone()
        .or(config.tag.clone().filter(|t| !t.is_empty()));
    Ok((index, Filter { hosts, tag }))
}

/// Starts the TUI.
fn tui(
    index: Index,
    disk: Arc<dyn restoric::disk::Disk>,
    app: App,
    view: &ViewArgs,
    config: &Config,
) -> Result<()> {
    let mut app = app;
    app.mode_switch = Some(index.mode_switch());
    app.strict = view.strict;
    app.icons = !view.no_icons && config.icons.unwrap_or(true);
    app.diff_limit = config.diff_limit();
    app.keymap = config.keymap()?;
    app.start_at = view.at.clone();
    if let Some(d) = config.restore_dir(app.home.as_deref()) {
        app.places.restore_dir = d;
    }
    if let Some(name) = &view.select {
        app.select_name(name.into());
    }
    let theme = config.apply_colors(Theme::from_env())?;
    let (tx, rx) = crossbeam_channel::unbounded();
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
    let ctx = Ctx {
        index,
        disk,
        places: app.places.clone(),
        preview_limit: config.preview_limit(),
    };
    let worker = Worker::start(Arc::new(ctx), threads, tx);
    restoric::tui::run(app, worker, rx, theme)
}

/// `restoric demo`: the sample project, written to a temporary folder so
/// restores have somewhere real to go.
fn demo(view: &ViewArgs, config: &Config) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("restoric-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let root = dir.join("dev/project");
    let dsl = include_str!("../tests/fixtures/project.dsl").replace(
        "root /home/bege/dev/project",
        &format!("root {}", root.display()),
    );
    let repo = FakeRepo::parse(&dsl)?;
    repo.disk().write_files()?;
    let snaps = repo.snapshots()?;
    let index = Index::new(
        Arc::new(repo),
        Cache::in_memory(),
        Mode::Content,
        config.memory_cache(),
    );
    let mut app = App::new(
        snaps,
        Filter::default(),
        root.join("src"),
        jiff::tz::TimeZone::system(),
        Some(dir.clone()),
    );
    app.places.restore_dir = dir.join("Restored");
    app.places.undo_dir = dir.join("undo");
    let result = tui(index, Arc::new(RealDisk), app, view, config);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn run(cli: Cli) -> Result<()> {
    let config = Config::load()?;
    match cli.command {
        Some(Command::Demo) => demo(&cli.view, &config),
        Some(Command::Log { path, json }) => {
            let path = std::path::absolute(&path).context("resolving the path")?;
            let (index, filter) = open(&cli.repo, &cli.view, &config)?;
            let tty = std::io::stderr().is_terminal();
            let mut progress = |done: usize, total: usize| {
                if tty && (done.is_multiple_of(16) || done == total) {
                    eprint!("\rindexing {done}/{total}");
                    if done == total {
                        eprint!("\r\x1b[K");
                    }
                }
            };
            let log = restoric::log::log(&index, &filter, &path, &mut progress)?;
            let mut out = std::io::stdout().lock();
            if json {
                serde_json::to_writer_pretty(&mut out, &log.entries)?;
                writeln!(out)?;
            } else {
                let tz = jiff::tz::TimeZone::system();
                write!(out, "{}", restoric::log::render(&log, &path, &tz))?;
            }
            Ok(())
        }
        Some(Command::Versions { file, json }) => {
            let path = std::path::absolute(&file).context("resolving the path")?;
            let (index, filter) = open(&cli.repo, &cli.view, &config)?;
            let versions = restoric::log::versions(&index, &filter, &path)?;
            let mut out = std::io::stdout().lock();
            if json {
                serde_json::to_writer_pretty(&mut out, &versions.list)?;
                writeln!(out)?;
            } else {
                let tz = jiff::tz::TimeZone::system();
                let text = restoric::log::render_versions(&versions, &path, &tz);
                write!(out, "{text}")?;
            }
            Ok(())
        }
        None => {
            let path = match &cli.path {
                Some(p) => std::path::absolute(p).context("resolving the path")?,
                None => std::env::current_dir().context("finding the current folder")?,
            };
            // A file opens its folder, with the file selected.
            let mut view = cli.view;
            let folder = if path.is_file() {
                if view.select.is_none() {
                    view.select = path.file_name().map(|n| n.to_string_lossy().into_owned());
                }
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            } else {
                path
            };
            let (index, filter) = open(&cli.repo, &view, &config)?;
            let snaps = index.repo().snapshots()?;
            if timeline_set(&snaps, &filter, &folder).is_empty() {
                anyhow::bail!("{}", explain_empty(&snaps, &filter, &folder));
            }
            let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
            let app = App::new(snaps, filter, folder, jiff::tz::TimeZone::system(), home);
            tui(index, Arc::new(RealDisk), app, &view, &config)
        }
    }
}

fn main() -> ExitCode {
    let _guard = init_logging();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            eprintln!("restoric: {e:#}");
            ExitCode::FAILURE
        }
    }
}
