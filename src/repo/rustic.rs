//! [`Repo`] on top of rustic_core. The only file that sees rustic types.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustic_backend::BackendOptions;
use rustic_core::repofile::{Node as RNode, NodeType, SnapshotFile};
use rustic_core::{
    CommandInput, CredentialOptions, Credentials, ErrorKind, IndexedFullStatus, IndexedIdsStatus,
    LocalDestination, LsOptions, OpenStatus, Progress, ProgressBars, ProgressType, Repository,
    RepositoryBackends, RepositoryOptions, RestoreOptions, RusticError, RusticProgress,
    RusticResult,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    BlobId, Id, Node, NodeKind, Repo, RestoreStep, SnapshotId, SnapshotInfo, Stopped, Summary,
    Tree, TreeId,
};

/// `text`, an error from running the password command, with the command
/// hidden: a failing command is quoted in the error, and its text often
/// holds the password (`echo hunter2`). Hidden as `given` and as rustic
/// prints it, re-joined from its words (`echo "hunter2"` comes out `echo
/// hunter2`). A bare command name carries no secret, and hiding it would
/// blank that word wherever the message uses it (`pass: x is not in the
/// password store`), so it stays.
fn hide_command(text: &str, given: &str, parsed: &CommandInput) -> String {
    if parsed.args().is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for c in [given, parsed.to_string().as_str()] {
        if !c.is_empty() {
            out = out.replace(c, "***");
        }
    }
    out
}

/// `c` as a command. A parse error (an unbalanced quote, say) quotes the
/// whole text, and with it any password in it: the text as given is
/// hidden, as a failing command's is.
fn parse_command(c: &str) -> Result<CommandInput> {
    c.parse::<CommandInput>().map_err(|e| {
        let text = e.to_string();
        let shown = if c.is_empty() {
            text
        } else {
            text.replace(c, "***")
        };
        anyhow!("bad password command: {shown}")
    })
}

/// Names the cache's backend version: a new rustic_core starts a new cache.
pub const BACKEND: &str = "rustic_core 0.13.0";

/// Where the repository is and how to unlock it, as restic takes them.
#[derive(Clone, Default)]
pub struct OpenOptions {
    pub repo: Option<String>,
    pub repo_file: Option<PathBuf>,
    pub password: Option<Zeroizing<String>>,
    pub password_file: Option<PathBuf>,
    pub password_command: Option<String>,
    /// The repository has no password (`restic init --insecure-no-password`).
    pub no_password: bool,
    /// rustic's own cache (index, snapshots, tree packs); default is rustic's.
    pub cache_dir: Option<PathBuf>,
}

/// Why a repository wouldn't unlock, when asking for a password can help.
#[derive(Debug, thiserror::Error)]
pub enum Locked {
    #[error(
        "no password given: set RESTIC_PASSWORD, RESTIC_PASSWORD_FILE or RESTIC_PASSWORD_COMMAND, \
         or pass --insecure-no-password for a repository without one"
    )]
    NoPassword,
    #[error("wrong password")]
    WrongPassword,
}

pub struct RusticRepo {
    id: Id,
    /// Index of tree blobs plus the ids of data blobs: enough to walk trees.
    trees: Repository<IndexedIdsStatus>,
    /// The full index, loaded on the first file read. It costs more memory.
    full: Mutex<Option<Arc<Repository<IndexedFullStatus>>>>,
    reopen: Reopen,
    /// Where the full repository reports how far a restore has copied.
    meter: Arc<Meter>,
    /// One restore at a time: they share `meter`.
    restoring: Mutex<()>,
}

/// The byte count of the restore that's running, as rustic reports it.
#[derive(Debug, Default)]
struct Meter {
    total: AtomicU64,
    done: AtomicU64,
    /// rustic has started copying file contents.
    copying: AtomicBool,
}

impl Meter {
    fn reset(&self) {
        self.total.store(0, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.copying.store(false, Ordering::Relaxed);
    }
}

/// Hands rustic a progress that fills `Meter` for byte counts (only the
/// restore's content step uses one) and hides spinners and counters.
#[derive(Debug, Clone)]
struct ToMeter(Arc<Meter>);

impl ProgressBars for ToMeter {
    fn progress(&self, kind: ProgressType, _prefix: &str) -> Progress {
        match kind {
            ProgressType::Bytes => Progress::new(self.clone()),
            _ => Progress::hidden(),
        }
    }
}

impl RusticProgress for ToMeter {
    fn is_hidden(&self) -> bool {
        false
    }

    fn set_length(&self, len: u64) {
        self.0.total.store(len, Ordering::Relaxed);
        self.0.copying.store(true, Ordering::Relaxed);
    }

    fn set_title(&self, _title: &str) {}

    fn inc(&self, n: u64) {
        self.0.done.fetch_add(n, Ordering::Relaxed);
    }

    fn finish(&self) {}
}

/// What it takes to open the repository again. No `Debug`: it holds the password.
struct Reopen {
    /// As given, for hiding its password in rustic's errors.
    location: String,
    opts: RepositoryOptions,
    backends: RepositoryBackends,
    credentials: Credentials,
}

/// rustic's error as ours, with the password of `location`'s URL hidden:
/// rustic_backend's REST errors quote the whole URL.
fn err_at(location: &str, e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{}", crate::repos::scrub(&e.to_string(), location))
}

/// An opened repository whose index isn't loaded yet: enough to list
/// snapshots, which is how restoric finds the repo that holds a folder.
pub struct Connection {
    open: Repository<OpenStatus>,
    reopen: Reopen,
}

impl Connection {
    pub fn open(o: &OpenOptions) -> Result<Self> {
        let repo = match (&o.repo, &o.repo_file) {
            (Some(r), _) => r.clone(),
            (None, Some(f)) => std::fs::read_to_string(f)
                .with_context(|| format!("reading {}", f.display()))?
                .trim()
                .to_string(),
            (None, None) => bail!(
                "no repository given: use --repo, or set RESTIC_REPOSITORY or RESTIC_REPOSITORY_FILE"
            ),
        };

        let given =
            o.password.is_some() || o.password_file.is_some() || o.password_command.is_some();
        if o.no_password && given {
            bail!("--insecure-no-password can't be used together with a password");
        }
        let credentials = if o.no_password {
            // restic encrypts the key with an empty password then.
            Credentials::password("")
        } else if let Some(p) = &o.password {
            // Not through CredentialOptions: it would keep a copy that
            // Debug prints.
            Credentials::password(p.as_str())
        } else {
            let mut cred = CredentialOptions::default();
            let mut command = None;
            if let Some(f) = &o.password_file {
                cred = cred.password_file(f.clone());
            } else if let Some(c) = &o.password_command {
                let parsed = parse_command(c)?;
                cred = cred.password_command(parsed.clone());
                command = Some((c.as_str(), parsed));
            }
            cred.credentials()
                .map_err(|e| {
                    let e = err_at(&repo, e);
                    match &command {
                        Some((given, parsed)) => {
                            anyhow!("{}", hide_command(&e.to_string(), given, parsed))
                        }
                        None => e,
                    }
                })?
                .ok_or(Locked::NoPassword)?
        };

        let mut opts = RepositoryOptions::default();
        if let Some(d) = &o.cache_dir {
            opts = opts.cache_dir(d.clone());
        }
        let backends = BackendOptions::default()
            .repository(repo.clone())
            .to_backends()
            .map_err(|e| err_at(&repo, e))?;

        let open = Repository::new(&opts, &backends)
            .and_then(|r| r.open(&credentials))
            .map_err(|e| {
                if e.is_incorrect_password() {
                    Locked::WrongPassword.into()
                } else {
                    err_at(&repo, e)
                }
            })?;
        Ok(Self {
            open,
            reopen: Reopen {
                location: repo,
                opts,
                backends,
                credentials,
            },
        })
    }

    /// Every snapshot in the repository, without loading the index.
    pub fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let snaps = self
            .open
            .get_all_snapshots()
            .map_err(|e| err_at(&self.reopen.location, e))?;
        Ok(snaps.into_iter().map(snapshot_from).collect())
    }
}

impl RusticRepo {
    pub fn open(o: &OpenOptions) -> Result<Self> {
        Self::from_connection(Connection::open(o)?)
    }

    /// Loads the index of an opened repository.
    pub fn from_connection(c: Connection) -> Result<Self> {
        let id = id_from(&c.open.config().id);
        let trees = c
            .open
            .to_indexed_ids()
            .map_err(|e| err_at(&c.reopen.location, e))?;
        Ok(Self {
            id,
            trees,
            full: Mutex::new(None),
            reopen: c.reopen,
            meter: Arc::default(),
            restoring: Mutex::new(()),
        })
    }

    /// rustic's error as ours, with the repository's password hidden.
    fn err(&self, e: impl std::fmt::Display) -> anyhow::Error {
        err_at(&self.reopen.location, e)
    }

    fn full(&self) -> Result<Arc<Repository<IndexedFullStatus>>> {
        let mut full = self.full.lock().unwrap();
        if let Some(r) = &*full {
            return Ok(r.clone());
        }
        let r = &self.reopen;
        let repo = Repository::new_with_progress(&r.opts, &r.backends, ToMeter(self.meter.clone()))
            .and_then(|repo| repo.open(&r.credentials))
            .and_then(|repo| repo.to_indexed())
            .map_err(|e| self.err(e))?;
        let repo = Arc::new(repo);
        *full = Some(repo.clone());
        Ok(repo)
    }
}

fn id_from(id: &impl std::ops::Deref<Target = rustic_core::Id>) -> Id {
    Id::from_hex(id.to_hex().as_str()).expect("rustic ids are 32 bytes")
}

fn snapshot_from(s: SnapshotFile) -> SnapshotInfo {
    SnapshotInfo {
        id: SnapshotId(id_from(&s.id)),
        time: s.time.timestamp(),
        host: s.hostname,
        paths: s.paths.iter().map(PathBuf::from).collect(),
        tags: s.tags.iter().cloned().collect(),
        tree: TreeId(id_from(&s.tree)),
        label: s.label,
        description: s.description,
        program: s.program_version,
        parent: s.parent.map(|p| SnapshotId(id_from(&p))),
        summary: s.summary.map(|m| Summary {
            files: (m.files_new, m.files_changed, m.files_unmodified),
            dirs: (m.dirs_new, m.dirs_changed, m.dirs_unmodified),
            total_files: m.total_files_processed,
            total_bytes: m.total_bytes_processed,
            added: m.data_added,
            added_packed: m.data_added_packed,
            // restic doesn't record backup_duration; both record start and end.
            duration: m
                .backup_end
                .timestamp()
                .duration_since(m.backup_start.timestamp()),
            command: m.command,
        }),
    }
}

fn node_from(n: &RNode) -> Node {
    let kind = match &n.node_type {
        NodeType::File => NodeKind::File,
        NodeType::Dir => NodeKind::Dir,
        NodeType::Symlink { .. } => NodeKind::Symlink {
            target: n.node_type.to_link().as_os_str().to_os_string(),
        },
        other => NodeKind::Other(other.to_string()),
    };
    let raw = Sha256::digest(serde_json::to_vec(n).unwrap_or_default());
    Node {
        name: n.name().into_owned(),
        kind,
        size: n.meta.size,
        mode: n.meta.mode,
        uid: n.meta.uid,
        gid: n.meta.gid,
        mtime: n.meta.mtime,
        content: n
            .content
            .iter()
            .flatten()
            .map(|c| BlobId(id_from(c)))
            .collect(),
        subtree: n.subtree.as_ref().map(|t| TreeId(id_from(t))),
        raw: Id(raw.into()),
    }
}

// Metadata is `non_exhaustive`, so it can't be built with a struct literal.
#[allow(clippy::field_reassign_with_default)]
fn rnode_from(n: &Node) -> RNode {
    use rustic_core::repofile::Metadata;
    let mut meta = Metadata::default();
    meta.size = n.size;
    meta.mode = n.mode;
    let mut r = RNode::new_node(&n.name, NodeType::File, meta);
    r.content = Some(
        n.content
            .iter()
            .map(|c| rustic_core::DataId::from(rustic_core::Id::new(c.0.0)))
            .collect(),
    );
    r
}

impl Repo for RusticRepo {
    fn id(&self) -> Id {
        self.id
    }

    fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let snaps = self.trees.get_all_snapshots().map_err(|e| self.err(e))?;
        Ok(snaps.into_iter().map(snapshot_from).collect())
    }

    fn tree(&self, id: &TreeId) -> Result<Arc<Tree>> {
        let rid = rustic_core::TreeId::from(rustic_core::Id::new(id.0.0));
        let t = self.trees.get_tree(&rid).map_err(|e| self.err(e))?;
        Ok(Arc::new(Tree::from_stored(
            t.nodes.iter().map(node_from).collect(),
        )))
    }

    fn read_at(&self, node: &Node, offset: u64, len: u64) -> Result<Vec<u8>> {
        if node.kind != NodeKind::File {
            bail!("not a file");
        }
        let repo = self.full()?;
        let open = repo.open_file(&rnode_from(node)).map_err(|e| self.err(e))?;
        let len = node.size.saturating_sub(offset).min(len) as usize;
        let data = repo
            .read_file_at(&open, offset as usize, len)
            .map_err(|e| self.err(e))?;
        Ok(data.to_vec())
    }

    fn restore(
        &self,
        snap: &SnapshotInfo,
        path: &Path,
        dest: &Path,
        progress: &mut dyn FnMut(RestoreStep),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        let _one = self.restoring.lock().unwrap_or_else(|e| e.into_inner());
        progress(RestoreStep::Preparing);
        super::check_dest(dest)?;
        let repo = self.full()?;
        let meter = &self.meter;
        meter.reset();
        let stop = AtomicBool::new(false);
        // rustic runs on its own thread; this one watches the meter and
        // `cancelled`, which needn't be shareable between threads.
        let res = std::thread::scope(|s| {
            let run =
                s.spawn(|| restore_with(&repo, snap, path, dest, &stop, &self.reopen.location));
            while !run.is_finished() {
                if cancelled() {
                    stop.store(true, Ordering::Relaxed);
                }
                if meter.copying.load(Ordering::Relaxed) {
                    progress(RestoreStep::Bytes {
                        done: meter.done.load(Ordering::Relaxed),
                        total: meter.total.load(Ordering::Relaxed),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let res = run
                .join()
                .unwrap_or_else(|_| Err(anyhow!("the restore panicked")));
            // The last count, which a short restore may finish before.
            if res.is_ok() && meter.copying.load(Ordering::Relaxed) {
                progress(RestoreStep::Bytes {
                    done: meter.done.load(Ordering::Relaxed),
                    total: meter.total.load(Ordering::Relaxed),
                });
            }
            res
        });
        // Stopped before copying. A folder at `dest` is the one rustic
        // made for a folder (or the claimed one), and goes. A file or
        // link there is the caller's claim, or something else's: the
        // caller knows which, and removes its own.
        if res.as_ref().is_err_and(super::is_stopped)
            && dest.symlink_metadata().is_ok_and(|m| m.is_dir())
        {
            let _ = std::fs::remove_dir_all(dest);
        }
        res
    }
}

/// rustic's restore of `path` to `dest`; gives up with [`Stopped`] if `stop`
/// is set by the time the plan is made. `location` is the repository as
/// given, for hiding its password in rustic's errors.
fn restore_with(
    repo: &Repository<IndexedFullStatus>,
    snap: &SnapshotInfo,
    path: &Path,
    dest: &Path,
    stop: &AtomicBool,
    location: &str,
) -> Result<()> {
    let err = |e: Box<RusticError>| err_at(location, e);
    let tree = rustic_core::TreeId::from(rustic_core::Id::new(snap.tree.0.0));
    let rel: PathBuf = path.components().skip(1).collect();
    let node = repo.node_from_path(tree, &rel).map_err(err)?;
    let is_dir = node.is_dir();
    // RestoreOptions is `non_exhaustive`: no struct literal.
    #[allow(clippy::field_reassign_with_default)]
    let opts = {
        let mut o = RestoreOptions::default();
        o.no_ownership = !super::is_root();
        o
    };
    let dest_str = dest
        .to_str()
        .context("restoring to a path that isn't UTF-8 isn't supported")?;
    let dest = LocalDestination::new(dest_str, true, !is_dir).map_err(err)?;
    let ls = repo.ls(&node, &LsOptions::default()).map_err(err)?;
    let plan = repo
        .prepare_restore(&opts, plain_names(ls.clone()), &dest, false)
        .map_err(err)?;
    if stop.load(Ordering::Relaxed) {
        return Err(Stopped.into());
    }
    repo.restore(plan, &opts, plain_names(ls), &dest)
        .map_err(err)?;
    Ok(())
}

/// rustic's walk of what to restore, failing at an entry whose name isn't
/// [plain](super::is_plain_name): rustic joins names onto the destination,
/// and such a name would write outside it. Both of rustic's passes stop at
/// the first error, and the planning pass sees every entry before any
/// content is written.
fn plain_names(
    ls: impl Iterator<Item = RusticResult<(PathBuf, RNode)>>,
) -> impl Iterator<Item = RusticResult<(PathBuf, RNode)>> {
    ls.map(|item| {
        let (path, node) = item?;
        if super::is_plain_name(&node.name()) {
            return Ok((path, node));
        }
        Err(RusticError::new(
            ErrorKind::InvalidInput,
            "The snapshot holds an entry named `{name}` in `{path}`, which would restore outside the destination.",
        )
        .attach_context("name", format!("{:?}", node.name()))
        .attach_context("path", path.parent().unwrap_or(&path).display().to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_core::repofile::Metadata;
    use std::ffi::OsStr;

    fn entry(name: &str) -> RusticResult<(PathBuf, RNode)> {
        let node = RNode::new_node(OsStr::new(name), NodeType::File, Metadata::default());
        Ok((PathBuf::from("src").join(name), node))
    }

    #[test]
    fn a_password_command_with_arguments_is_hidden_and_a_bare_one_is_not() {
        let hide = |text: &str, given: &str| {
            hide_command(text, given, &given.parse::<CommandInput>().unwrap())
        };
        assert_eq!(
            hide(
                "password command `echo hunter2` failed: echo \"hunter2\" exited",
                "echo \"hunter2\""
            ),
            "password command `***` failed: *** exited"
        );
        assert_eq!(
            hide(
                "password command `pass show restic/nas` failed: pass: restic/nas is not in the password store",
                "pass show restic/nas"
            ),
            "password command `***` failed: pass: restic/nas is not in the password store"
        );
        // A bare command: no secret in it, and the message stays readable.
        assert_eq!(
            hide(
                "password command `pass` failed: pass: restic/nas is not in the password store",
                "pass"
            ),
            "password command `pass` failed: pass: restic/nas is not in the password store"
        );
    }

    #[test]
    fn a_password_command_that_doesnt_parse_is_hidden_from_the_error() {
        let e = parse_command("echo \"hunter2").unwrap_err().to_string();
        assert!(e.starts_with("bad password command: "), "{e}");
        assert!(!e.contains("hunter2") && e.contains("***"), "{e}");
    }

    #[test]
    fn a_restore_stops_at_a_name_that_reaches_outside() {
        let names = |ns: &[&str]| -> Vec<RusticResult<(PathBuf, RNode)>> {
            plain_names(ns.iter().map(|n| entry(n)).collect::<Vec<_>>().into_iter()).collect()
        };
        assert!(names(&["a", ".b", "c"]).iter().all(Result::is_ok));
        for bad in ["..", "../../.bashrc", "/etc/cron.d/x", "a/b", ""] {
            let out = names(&["a", bad, "c"]);
            assert!(out[0].is_ok());
            let err = out[1].as_ref().expect_err(bad).to_string();
            assert!(err.contains("outside the destination"), "{err}");
        }
    }
}
