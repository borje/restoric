use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};

use restoric::app::App;
use restoric::cache::Cache;
use restoric::config::{Access, Config, RepoEntry, resolve_access};
use restoric::disk::RealDisk;
use restoric::index::timeline::{Filter, explain_empty, timeline_set};
use restoric::index::{Index, Mode};
use restoric::picker::Picker;
use restoric::repo::Repo;
use restoric::repo::fake::FakeRepo;
use restoric::repo::rustic::{BACKEND, Connection, OpenOptions, RusticRepo};
use restoric::repos::{self, Candidate, ProbeCache};
use restoric::tui::{Io, Term};
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

/// The environment is read in `env_access`, not by clap: a `[[repo]]` in the
/// config comes between the flags and the environment.
#[derive(Args)]
struct RepoArgs {
    /// Repository, as for restic [env: RESTIC_REPOSITORY, after the config's [[repo]]]
    #[arg(short, long, global = true)]
    repo: Option<String>,
    /// File holding the repository location [env: RESTIC_REPOSITORY_FILE]
    #[arg(long, global = true)]
    repository_file: Option<PathBuf>,
    /// File holding the password [env: RESTIC_PASSWORD_FILE]
    #[arg(long, global = true)]
    password_file: Option<PathBuf>,
    /// Command that prints the password [env: RESTIC_PASSWORD_COMMAND]
    #[arg(long, global = true)]
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
    /// Start in the repository picker: other hosts and backup paths
    #[arg(long)]
    browse: bool,
}

fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = restoric::dirs::state()?;
    std::fs::create_dir_all(&dir).ok()?;
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

impl RepoArgs {
    fn access(&self) -> Access {
        Access {
            repo: self.repo.clone(),
            repo_file: self.repository_file.clone(),
            password: None,
            password_file: self.password_file.clone(),
            password_command: self.password_command.clone(),
            no_password: self.insecure_no_password,
        }
    }
}

/// restic's environment variables.
fn env_access() -> Access {
    let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
    Access {
        repo: var("RESTIC_REPOSITORY"),
        repo_file: var("RESTIC_REPOSITORY_FILE").map(PathBuf::from),
        password: std::env::var("RESTIC_PASSWORD").ok(),
        password_file: var("RESTIC_PASSWORD_FILE").map(PathBuf::from),
        password_command: var("RESTIC_PASSWORD_COMMAND"),
        no_password: false,
    }
}

fn open_options(a: Access) -> OpenOptions {
    OpenOptions {
        repo: a.repo,
        repo_file: a.repo_file,
        password: a.password,
        password_file: a.password_file,
        password_command: a.password_command,
        no_password: a.no_password,
        cache_dir: None,
    }
}

/// Which snapshots are ours in the repository `entry` names (or the one
/// from the flags or environment, for `None`): `--host`, then the entry's
/// `host`, then the config's, then this machine's name.
fn filter(view: &ViewArgs, config: &Config, entry: Option<&RepoEntry>) -> Filter {
    let entry_hosts = entry.map(RepoEntry::hosts).unwrap_or_default();
    let hosts = if !view.host.is_empty() {
        view.host.clone()
    } else if !entry_hosts.is_empty() {
        entry_hosts
    } else if !config.hosts().is_empty() {
        config.hosts()
    } else {
        vec![gethostname::gethostname().to_string_lossy().into_owned()]
    };
    let tag = view
        .tag
        .clone()
        .or(entry.and_then(|e| e.tag.clone()))
        .or(config.tag.clone())
        .filter(|t| !t.is_empty());
    Filter { hosts, tag }
}

/// The line of an error that says what went wrong. rustic_core's errors
/// run to many lines, with the message under a `Message:` heading.
fn short(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    let lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.clone().next().unwrap_or_default().to_string();
    lines
        .skip_while(|l| *l != "Message:")
        .nth(1)
        .map_or(first, str::to_string)
}

/// The config's `[[repo]]` entries as candidates, each with its own idea
/// of which snapshots are ours.
fn candidates(view: &ViewArgs, config: &Config) -> Vec<Candidate> {
    config
        .repo
        .iter()
        .map(|e| Candidate {
            repo: e.repository.clone(),
            filter: filter(view, config, Some(e)),
        })
        .collect()
}

/// Where `start` got to.
enum Start {
    /// An opened repository: the one holding the folder, or the only one.
    Repo {
        repo: Box<RusticRepo>,
        /// Which `[[repo]]` it is, if any.
        entry: Option<usize>,
        location: String,
    },
    /// Several `[[repo]]` and none holds the folder, or `--browse`: nothing
    /// is opened, the picker lists them.
    Repos,
}

fn save_cache(path: Option<&Path>, cache: &ProbeCache) {
    if let Some(p) = path
        && let Err(e) = cache.save(p)
    {
        tracing::warn!("{e:#}");
    }
}

/// Opens the repository that holds `folder`: the one from `--repo`, else
/// the config's `[[repo]]` whose snapshots hold it, else the one from the
/// environment.
fn start(
    args: &RepoArgs,
    view: &ViewArgs,
    config: &Config,
    folder: &Path,
    browse: bool,
) -> Result<Start> {
    let home = restoric::dirs::home();
    let home = home.as_deref();
    let flags = args.access();
    let env = env_access();
    let direct = |a: Access| -> Result<Start> {
        let location = a.repo.clone().unwrap_or_else(|| {
            a.repo_file
                .as_ref()
                .map(|f| f.display().to_string())
                .unwrap_or_default()
        });
        Ok(Start::Repo {
            repo: Box::new(RusticRepo::open(&open_options(a))?),
            entry: None,
            location,
        })
    };
    if flags.has_repo() || config.repo.is_empty() {
        return direct(resolve_access(&flags, &env, None, home));
    }
    let cands = candidates(view, config);
    let cache_path = ProbeCache::default_path();
    let mut cache = cache_path
        .as_deref()
        .map(ProbeCache::load)
        .unwrap_or_default();
    let now = jiff::Timestamp::now();
    if let [e] = config.repo.as_slice() {
        // Nothing to choose from: open it and remember what it holds.
        let a = resolve_access(&flags, &env, Some(e), home);
        let c = Connection::open(&open_options(a))?;
        cache.record(&e.repository, &c.snapshots()?, now);
        save_cache(cache_path.as_deref(), &cache);
        return Ok(Start::Repo {
            repo: Box::new(RusticRepo::from_connection(c)?),
            entry: Some(0),
            location: e.repository.clone(),
        });
    }
    if browse {
        return Ok(Start::Repos);
    }
    let tty = std::io::stderr().is_terminal();
    let chosen = repos::choose(
        &cands,
        folder,
        &mut cache,
        now,
        |i, probing| {
            let e = &config.repo[i];
            if probing && tty {
                eprint!("\rchecking {}…\x1b[K", e.repository);
            }
            let a = resolve_access(&flags, &env, Some(e), home);
            let c = Connection::open(&open_options(a))?;
            let snaps = c.snapshots()?;
            Ok((c, snaps))
        },
        |i, err| {
            let repo = &config.repo[i].repository;
            tracing::warn!("skipping {repo}: {err:#}");
            if tty {
                eprint!("\r\x1b[K");
            }
            eprintln!("restoric: skipping {repo}: {}", short(err));
        },
    );
    if tty {
        eprint!("\r\x1b[K");
    }
    save_cache(cache_path.as_deref(), &cache);
    match chosen {
        Some((i, c)) => Ok(Start::Repo {
            repo: Box::new(RusticRepo::from_connection(c)?),
            entry: Some(i),
            location: config.repo[i].repository.clone(),
        }),
        None if env.has_repo() => direct(resolve_access(&flags, &env, None, home)),
        None => Ok(Start::Repos),
    }
}

/// The error for a folder no configured repository holds, without a TUI.
fn none_holds(config: &Config, folder: &Path) -> anyhow::Error {
    let names: Vec<&str> = config.repo.iter().map(|e| e.repository.as_str()).collect();
    anyhow!(
        "none of the repositories in the config has snapshots of {} ({}); \
         use --repo, set RESTIC_REPOSITORY, or pick one with --browse",
        folder.display(),
        names.join(", ")
    )
}

/// The index over an opened repository, with its on-disk cache.
fn index_for(repo: RusticRepo, view: &ViewArgs, config: &Config) -> Index {
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
    Index::new(Arc::new(repo), cache, mode, config.memory_cache())
}

/// For `restoric log` and `versions`: the repository holding `folder`, or
/// an error.
fn open(
    args: &RepoArgs,
    view: &ViewArgs,
    config: &Config,
    folder: &Path,
) -> Result<(Index, Filter)> {
    match start(args, view, config, folder, false)? {
        Start::Repo { repo, entry, .. } => Ok((
            index_for(*repo, view, config),
            filter(view, config, entry.map(|i| &config.repo[i])),
        )),
        Start::Repos => Err(none_holds(config, folder)),
    }
}

/// Runs the folder view in `term`. True when `q` asks to go back to the
/// picker.
fn tui(
    term: &mut Term,
    index: Arc<Index>,
    disk: Arc<dyn restoric::disk::Disk>,
    app: App,
    view: &ViewArgs,
    config: &Config,
) -> Result<bool> {
    let mut app = app;
    app.mode_switch = Some(index.mode_switch());
    app.strict = index.mode() == Mode::Strict;
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
    term.run(app, worker, rx, theme)
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
    let index = Arc::new(Index::new(
        Arc::new(repo),
        Cache::in_memory(),
        Mode::Content,
        config.memory_cache(),
    ));
    let mut app = App::new(
        snaps,
        Filter::default(),
        root.join("src"),
        jiff::tz::TimeZone::system(),
        Some(dir.clone()),
    );
    app.places.restore_dir = dir.join("Restored");
    app.places.undo_dir = dir.join("undo");
    let result = Term::new()
        .and_then(|mut term| tui(&mut term, index, Arc::new(RealDisk), app, view, config));
    let _ = std::fs::remove_dir_all(&dir);
    result.map(|_| ())
}

fn run(cli: Cli) -> Result<()> {
    let config = Config::load()?;
    match cli.command {
        Some(Command::Demo) => demo(&cli.view, &config),
        Some(Command::Log { path, json }) => {
            let path = std::path::absolute(&path).context("resolving the path")?;
            let (index, filter) = open(&cli.repo, &cli.view, &config, &path)?;
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
            let (index, filter) = open(&cli.repo, &cli.view, &config, &path)?;
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
            browse(&cli.repo, &mut view, &config, folder)
        }
    }
}

/// The folder view on `folder`, through the picker when the folder
/// isn't in this machine's snapshots or `--browse` says so.
fn browse(args: &RepoArgs, view: &mut ViewArgs, config: &Config, folder: PathBuf) -> Result<()> {
    let home = restoric::dirs::home();
    let tz = jiff::tz::TimeZone::system();
    let now = jiff::Timestamp::now();
    let tty = std::io::stdout().is_terminal();
    let theme = config.apply_colors(Theme::from_env())?;
    let mut picker = Picker::new(folder.clone(), home.clone(), tz.clone(), now);
    // The repository in use: the one opened before the picker, or the one
    // picked at the repo level. Its index lasts from one pick to the next.
    let mut current = match start(args, view, config, &folder, view.browse)? {
        Start::Repo {
            repo,
            entry,
            location,
        } => {
            let f = filter(view, config, entry.map(|i| &config.repo[i]));
            let snaps = repo.snapshots()?;
            let held = !timeline_set(&snaps, &f, &folder).is_empty();
            if held && !view.browse {
                let mut app = App::new(snaps, f.clone(), folder, tz, home);
                app.mine = f.hosts;
                let mut term = Term::new()?;
                let index = Arc::new(index_for(*repo, view, config));
                return tui(&mut term, index, Arc::new(RealDisk), app, view, config).map(|_| ());
            }
            let message = (!held).then(|| explain_empty(&snaps, &f, &folder));
            if !tty {
                anyhow::bail!(
                    "{}",
                    message.unwrap_or_else(|| "--browse needs a terminal".to_string())
                );
            }
            picker = picker.with_groups(&location, &snaps, &f.hosts, message);
            Some((entry, Arc::new(index_for(*repo, view, config))))
        }
        Start::Repos => {
            if !tty {
                if view.browse {
                    anyhow::bail!("--browse needs a terminal");
                }
                return Err(none_holds(config, &folder));
            }
            let cands = candidates(view, config);
            let cache = ProbeCache::default_path()
                .as_deref()
                .map(ProbeCache::load)
                .unwrap_or_default();
            picker = picker.with_repos(&cands, &cache);
            None
        }
    };

    let mut term = Term::new()?;
    let flags = args.access();
    let env = env_access();
    let cache_path = ProbeCache::default_path();
    let mut cache = cache_path
        .as_deref()
        .map(ProbeCache::load)
        .unwrap_or_default();
    loop {
        // The repository opened from the repo level, kept for after the pick.
        let mut opened: Option<(usize, RusticRepo)> = None;
        let pick = {
            let mut io = |io: Io| -> Result<Vec<restoric::repo::SnapshotInfo>> {
                let i = match io {
                    Io::Open(i) | Io::Refresh(i) => i,
                };
                let e = &config.repo[i];
                // The repository in use opens again from what it has loaded.
                if let (Io::Open(_), Some((Some(c), index))) = (io, &current)
                    && *c == i
                {
                    let snaps = index.repo().snapshots()?;
                    cache.record(&e.repository, &snaps, now);
                    save_cache(cache_path.as_deref(), &cache);
                    return Ok(snaps);
                }
                let a = resolve_access(&flags, &env, Some(e), home.as_deref());
                let c = Connection::open(&open_options(a)).map_err(|e| anyhow!("{}", short(&e)))?;
                let snaps = c.snapshots().map_err(|e| anyhow!("{}", short(&e)))?;
                cache.record(&e.repository, &snaps, now);
                save_cache(cache_path.as_deref(), &cache);
                if matches!(io, Io::Open(_)) {
                    let r = RusticRepo::from_connection(c).map_err(|e| anyhow!("{}", short(&e)))?;
                    opened = Some((i, r));
                }
                Ok(snaps)
            };
            term.pick(&mut picker, &theme, &mut io)?
        };
        let Some(pick) = pick else {
            return Ok(());
        };
        if let Some(i) = pick.repo
            && let Some((j, r)) = opened.take()
            && i == j
        {
            // Close the old index's cache first: it may be the same file.
            drop(current.take());
            current = Some((Some(i), Arc::new(index_for(r, view, config))));
        }
        let (entry, index) = current.clone().context("no repository was opened")?;

        // The picked host replaces this machine's; the tag stays only for one
        // of this machine's hosts.
        let base = filter(view, config, entry.map(|i| &config.repo[i]));
        let foreign = !base.hosts.contains(&pick.host);
        let f = Filter {
            hosts: vec![pick.host.clone()],
            tag: if foreign { None } else { base.tag },
        };
        let snaps = index.repo().snapshots()?;
        let (open_at, select) = pick.open_at(&folder, &snaps, &index);
        let mut app = App::new(snaps, f, open_at, tz.clone(), home.clone());
        app.mine = base.hosts;
        app.shown_host = Some(pick.host);
        app.start_dir = folder.clone();
        app.from_picker = true;
        if let Some(name) = select {
            app.select_name(name);
        }
        if !tui(&mut term, index, Arc::new(RealDisk), app, view, config)? {
            return Ok(());
        }
        // Why the picker first opened, and where to start, no longer apply.
        picker.message = None;
        view.select = None;
        view.at = None;
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
