use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use restoric::app::App;
use restoric::cache::Cache;
use restoric::disk::RealDisk;
use restoric::index::timeline::{Filter, explain_empty, timeline_set};
use restoric::index::{Index, Mode};
use restoric::repo::Repo;
use restoric::repo::rustic::{BACKEND, OpenOptions, RusticRepo};
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

fn open(args: &RepoArgs, view: &ViewArgs) -> Result<(Index, Filter)> {
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
        Some(path) => Cache::open(&path, BACKEND).unwrap_or_else(|e| {
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
    let index = Index::new(Arc::new(repo), cache, mode, 256 << 20);
    let hosts = if view.host.is_empty() {
        vec![gethostname::gethostname().to_string_lossy().into_owned()]
    } else {
        view.host.clone()
    };
    Ok((
        index,
        Filter {
            hosts,
            tag: view.tag.clone(),
        },
    ))
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Log { path, json }) => {
            let path = std::path::absolute(&path).context("resolving the path")?;
            let (index, filter) = open(&cli.repo, &cli.view)?;
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
            let (index, filter) = open(&cli.repo, &cli.view)?;
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
            // A file opens its folder.
            let folder = if path.is_file() {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            } else {
                path
            };
            let (index, filter) = open(&cli.repo, &cli.view)?;
            let snaps = index.repo().snapshots()?;
            if timeline_set(&snaps, &filter, &folder).is_empty() {
                anyhow::bail!("{}", explain_empty(&snaps, &filter, &folder));
            }
            let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
            let mut app = App::new(snaps, filter, folder, jiff::tz::TimeZone::system(), home);
            app.mode_switch = Some(index.mode_switch());
            app.strict = cli.view.strict;
            let (tx, rx) = crossbeam_channel::unbounded();
            let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
            let places = app.places.clone();
            let ctx = Ctx {
                index,
                disk: Arc::new(RealDisk),
                places,
            };
            let worker = Worker::start(Arc::new(ctx), threads, tx);
            restoric::tui::run(app, worker, rx)
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
