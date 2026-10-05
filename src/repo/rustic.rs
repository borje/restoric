//! [`Repo`] on top of rustic_core. The only file that sees rustic types.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use rustic_backend::BackendOptions;
use rustic_core::repofile::{Node as RNode, NodeType, SnapshotFile};
use rustic_core::{
    CommandInput, CredentialOptions, Credentials, IndexedFullStatus, IndexedIdsStatus, Repository,
    RepositoryBackends, RepositoryOptions,
};
use sha2::{Digest, Sha256};

use super::{BlobId, FileBytes, Id, Node, NodeKind, Repo, SnapshotId, SnapshotInfo, Tree, TreeId};

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
}

/// What it takes to open the repository again. No `Debug`: it holds the password.
struct Reopen {
    opts: RepositoryOptions,
    backends: RepositoryBackends,
    credentials: Credentials,
}

impl RusticRepo {
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
        if let Some(p) = &o.password {
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
            "no password given: set RESTIC_PASSWORD, RESTIC_PASSWORD_FILE or RESTIC_PASSWORD_COMMAND",
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
        let id = id_from(&open.config().id);
        let trees = open.to_indexed_ids().map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            id,
            trees,
            full: Mutex::new(None),
            reopen: Reopen {
                opts,
                backends,
                credentials,
            },
        })
    }

    fn full(&self) -> Result<Arc<Repository<IndexedFullStatus>>> {
        let mut full = self.full.lock().unwrap();
        if let Some(r) = &*full {
            return Ok(r.clone());
        }
        let r = &self.reopen;
        let repo = Repository::new(&r.opts, &r.backends)
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

    fn read_file(&self, node: &Node, limit: u64) -> Result<FileBytes> {
        if node.kind != NodeKind::File {
            bail!("not a file");
        }
        let repo = self.full()?;
        let open = repo
            .open_file(&rnode_from(node))
            .map_err(|e| anyhow!("{e}"))?;
        let len = node.size.min(limit) as usize;
        let data = repo
            .read_file_at(&open, 0, len)
            .map_err(|e| anyhow!("{e}"))?;
        Ok(FileBytes {
            data: data.to_vec(),
            size: node.size,
        })
    }
}
