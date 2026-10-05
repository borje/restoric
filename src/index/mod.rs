//! Everything restoric computes from the repository: what's at a path in a
//! snapshot, whether two versions differ, change points and counts.
//! Written against [`Repo`] only.

pub mod find;
pub mod fingerprint;
pub mod folder;
pub mod listing;
pub mod live;
pub mod timeline;
pub mod versions;

use std::ffi::OsString;
use std::path::{Component, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::Result;
use quick_cache::Weighter;

use crate::cache::{Cache, Table};
use crate::repo::{Id, Node, Repo, SnapshotInfo, Tree, TreeId};
use fingerprint::Fp;

/// What counts as a change (PLAN.md §2.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Content, mode, owner and group; metadata churn is ignored.
    #[default]
    Content,
    /// Any difference restic stored (`--strict`).
    Strict,
}

impl Mode {
    fn byte(self) -> u8 {
        match self {
            Mode::Content => 0,
            Mode::Strict => 1,
        }
    }
}

/// Switches an [`Index`]'s mode from elsewhere (the UI thread).
#[derive(Clone, Debug)]
pub struct ModeSwitch(Arc<AtomicU8>);

impl ModeSwitch {
    pub fn set(&self, mode: Mode) {
        self.0.store(mode.byte(), Ordering::Relaxed);
    }
}

/// What a path points to in one snapshot, as far as change detection cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRef {
    Missing,
    Dir(TreeId),
    Leaf { fp: Fp, raw: Id },
}

impl NodeRef {
    pub fn of(node: &Node) -> Self {
        match node.subtree {
            Some(t) if node.is_dir() => NodeRef::Dir(t),
            _ => NodeRef::Leaf {
                fp: fingerprint::leaf(node),
                raw: node.raw,
            },
        }
    }

    pub fn exists(&self) -> bool {
        *self != NodeRef::Missing
    }

    fn encode(&self) -> Vec<u8> {
        match self {
            NodeRef::Missing => vec![0],
            NodeRef::Dir(t) => [&[1u8][..], &t.0.0].concat(),
            NodeRef::Leaf { fp, raw } => [&[2u8][..], fp, &raw.0].concat(),
        }
    }

    fn decode(b: &[u8]) -> Option<Self> {
        let id = |r: &[u8]| -> Option<[u8; 32]> { r.try_into().ok() };
        match b.first()? {
            0 => Some(NodeRef::Missing),
            1 => Some(NodeRef::Dir(TreeId(Id(id(b.get(1..33)?)?)))),
            2 => Some(NodeRef::Leaf {
                fp: id(b.get(1..33)?)?,
                raw: Id(id(b.get(33..65)?)?),
            }),
            _ => None,
        }
    }
}

#[derive(Clone)]
struct TreeWeight;

impl Weighter<TreeId, Arc<Tree>> for TreeWeight {
    fn weight(&self, _: &TreeId, t: &Arc<Tree>) -> u64 {
        t.weight()
    }
}

/// The repository, a size-capped in-memory tree cache and the on-disk cache.
pub struct Index {
    repo: Arc<dyn Repo>,
    trees: quick_cache::sync::Cache<TreeId, Arc<Tree>, TreeWeight>,
    cache: Cache,
    /// The current [`Mode`], switchable while running (`:set strict`).
    mode: Arc<AtomicU8>,
}

pub fn components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect()
}

impl Index {
    /// `memory_bytes` caps the in-memory tree cache.
    pub fn new(repo: Arc<dyn Repo>, cache: Cache, mode: Mode, memory_bytes: u64) -> Self {
        Self {
            repo,
            trees: quick_cache::sync::Cache::with_weighter(10_000, memory_bytes, TreeWeight),
            cache,
            mode: Arc::new(AtomicU8::new(mode.byte())),
        }
    }

    pub fn mode(&self) -> Mode {
        match self.mode.load(Ordering::Relaxed) {
            1 => Mode::Strict,
            _ => Mode::Content,
        }
    }

    /// A handle that switches the mode for every user of this index.
    pub fn mode_switch(&self) -> ModeSwitch {
        ModeSwitch(self.mode.clone())
    }

    pub fn repo(&self) -> &Arc<dyn Repo> {
        &self.repo
    }

    pub fn tree(&self, id: TreeId) -> Result<Arc<Tree>> {
        self.trees.get_or_insert_with(&id, || self.repo.tree(&id))
    }

    /// Saves what was computed to the on-disk cache.
    pub fn flush(&self) -> Result<()> {
        self.cache.flush()
    }

    /// What's at `path` (absolute) in `snap`. Each folder on the way is
    /// cached too, so siblings and children are found without a walk.
    pub fn node_ref(&self, snap: &SnapshotInfo, path: &Path) -> Result<NodeRef> {
        let parts = components(path);
        self.node_ref_parts(snap, &parts)
    }

    fn node_ref_parts(&self, snap: &SnapshotInfo, parts: &[OsString]) -> Result<NodeRef> {
        let Some((name, parent)) = parts.split_last() else {
            return Ok(NodeRef::Dir(snap.tree));
        };
        let mut key = snap.id.0.0.to_vec();
        for p in parts {
            key.push(b'/');
            key.extend(p.as_encoded_bytes());
        }
        if let Some(r) = self
            .cache
            .get(Table::PathRef, &key)?
            .and_then(|b| NodeRef::decode(&b))
        {
            return Ok(r);
        }
        let r = match self.node_ref_parts(snap, parent)? {
            NodeRef::Dir(t) => self
                .tree(t)?
                .get(name)
                .map(NodeRef::of)
                .unwrap_or(NodeRef::Missing),
            _ => NodeRef::Missing,
        };
        self.cache.put(Table::PathRef, key, r.encode());
        Ok(r)
    }

    /// The full node at `path` in `snap`, if there is one.
    pub fn node_at(&self, snap: &SnapshotInfo, path: &Path) -> Result<Option<Node>> {
        let parts = components(path);
        let Some((name, parent)) = parts.split_last() else {
            return Ok(None);
        };
        match self.node_ref_parts(snap, parent)? {
            NodeRef::Dir(t) => Ok(self.tree(t)?.get(name).cloned()),
            _ => Ok(None),
        }
    }
}
