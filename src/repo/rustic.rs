//! [`Repo`] on top of rustic_core. The only file that sees rustic types.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustic_backend::BackendOptions;
use rustic_core::repofile::{Node as RNode, NodeType, SnapshotFile};
use rustic_core::{
    CommandInput, CredentialOptions, Credentials, IndexedFullStatus, IndexedIdsStatus,
    LocalDestination, LsOptions, OpenStatus, Progress, ProgressBars, ProgressType, Repository,
    RepositoryBackends, RepositoryOptions, RestoreOptions, RusticProgress,
};
use sha2::{Digest, Sha256};

use super::{
    BlobId, Id, Node, NodeKind, Repo, RestoreStep, SnapshotId, SnapshotInfo, Stopped, Tree, TreeId,
};

/// Names the cache's backend version: a new rustic_core starts a new cache.
pub const BACKEND: &str = "rustic_core 0.13.0";

/// Where the repository is and how to unlock it, as restic takes them.
#[derive(Clone, Default)]
pub struct OpenOptions {
    pub repo: Option<String>,
    pub repo_file: Option<PathBuf>,
    pub password: Option<String>,
    pub password_file: Option<PathBuf>,
    pub password_command: Option<String>,
    /// The repository has no password (`restic init --insecure-no-password`).
    pub no_password: bool,
    /// rustic's own cache (index, snapshots, tree packs); default is rustic's.
    pub cache_dir: Option<PathBuf>,
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
    opts: RepositoryOptions,
    backends: RepositoryBackends,
    credentials: Credentials,
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

        let mut cred = CredentialOptions::default();
        let given =
            o.password.is_some() || o.password_file.is_some() || o.password_command.is_some();
        if o.no_password && given {
            bail!("--insecure-no-password can't be used together with a password");
        }
        if o.no_password {
            // restic encrypts the key with an empty password then.
            cred = cred.password(String::new());
        } else if let Some(p) = &o.password {
            cred = cred.password(p.clone());
        } else if let Some(f) = &o.password_file {
            cred = cred.password_file(f.clone());
        } else if let Some(c) = &o.password_command {
            cred = cred.password_command(
                c.parse::<CommandInput>()
                    .map_err(|e| anyhow!("bad password command: {e}"))?,
            );
        }
        let credentials = cred.credentials().map_err(|e| anyhow!("{e}"))?.context(
            "no password given: set RESTIC_PASSWORD, RESTIC_PASSWORD_FILE or RESTIC_PASSWORD_COMMAND, \
             or pass --insecure-no-password for a repository without one",
        )?;

        let mut opts = RepositoryOptions::default();
        if let Some(d) = &o.cache_dir {
            opts = opts.cache_dir(d.clone());
        }
        let backends = BackendOptions::default()
            .repository(repo)
            .to_backends()
            .map_err(|e| anyhow!("{e}"))?;

        let open = Repository::new(&opts, &backends)
            .and_then(|r| r.open(&credentials))
            .map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            open,
            reopen: Reopen {
                opts,
                backends,
                credentials,
            },
        })
    }

    /// Every snapshot in the repository, without loading the index.
    pub fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let snaps = self.open.get_all_snapshots().map_err(|e| anyhow!("{e}"))?;
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
        let trees = c.open.to_indexed_ids().map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            id,
            trees,
            full: Mutex::new(None),
            reopen: c.reopen,
            meter: Arc::default(),
            restoring: Mutex::new(()),
        })
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
            .map_err(|e| anyhow!("{e}"))?;
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
        let snaps = self.trees.get_all_snapshots().map_err(|e| anyhow!("{e}"))?;
        Ok(snaps.into_iter().map(snapshot_from).collect())
    }

    fn tree(&self, id: &TreeId) -> Result<Arc<Tree>> {
        let rid = rustic_core::TreeId::from(rustic_core::Id::new(id.0.0));
        let t = self.trees.get_tree(&rid).map_err(|e| anyhow!("{e}"))?;
        Ok(Arc::new(Tree {
            nodes: t.nodes.iter().map(node_from).collect(),
        }))
    }

    fn read_at(&self, node: &Node, offset: u64, len: u64) -> Result<Vec<u8>> {
        if node.kind != NodeKind::File {
            bail!("not a file");
        }
        let repo = self.full()?;
        let open = repo
            .open_file(&rnode_from(node))
            .map_err(|e| anyhow!("{e}"))?;
        let len = node.size.saturating_sub(offset).min(len) as usize;
        let data = repo
            .read_file_at(&open, offset as usize, len)
            .map_err(|e| anyhow!("{e}"))?;
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
        if dest.symlink_metadata().is_ok() {
            bail!("{} already exists", dest.display());
        }
        let repo = self.full()?;
        let meter = &self.meter;
        meter.reset();
        let stop = AtomicBool::new(false);
        // rustic runs on its own thread; this one watches the meter and
        // `cancelled`, which needn't be shareable between threads.
        let res = std::thread::scope(|s| {
            let run = s.spawn(|| restore_with(&repo, snap, path, dest, &stop));
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
        if res.as_ref().is_err_and(super::is_stopped) {
            let _ = match dest.symlink_metadata() {
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(dest),
                Ok(_) => std::fs::remove_file(dest),
                Err(_) => Ok(()),
            };
        }
        res
    }
}

/// rustic's restore of `path` to `dest`; gives up with [`Stopped`] if `stop`
/// is set by the time the plan is made.
fn restore_with(
    repo: &Repository<IndexedFullStatus>,
    snap: &SnapshotInfo,
    path: &Path,
    dest: &Path,
    stop: &AtomicBool,
) -> Result<()> {
    let tree = rustic_core::TreeId::from(rustic_core::Id::new(snap.tree.0.0));
    let rel: PathBuf = path.components().skip(1).collect();
    let node = repo
        .node_from_path(tree, &rel)
        .map_err(|e| anyhow!("{e}"))?;
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
    let dest = LocalDestination::new(dest_str, true, !is_dir).map_err(|e| anyhow!("{e}"))?;
    let ls = repo
        .ls(&node, &LsOptions::default())
        .map_err(|e| anyhow!("{e}"))?;
    let plan = repo
        .prepare_restore(&opts, ls.clone(), &dest, false)
        .map_err(|e| anyhow!("{e}"))?;
    if stop.load(Ordering::Relaxed) {
        return Err(Stopped.into());
    }
    repo.restore(plan, &opts, ls, &dest)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(())
}
