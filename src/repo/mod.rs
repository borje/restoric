//! The repository as the rest of restoric sees it: the [`Repo`] trait and our
//! own types. rustic_core types stay inside `rustic.rs`.

pub mod fake;
pub mod rustic;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use jiff::Timestamp;

/// A 32-byte content address, as restic uses for everything.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub [u8; 32]);

impl Id {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The first 8 hex digits, as restic shows snapshot ids.
    pub fn short(&self) -> String {
        self.to_hex()[..8].to_string()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub Id);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }
    };
}

typed_id!(SnapshotId);
typed_id!(TreeId);
typed_id!(BlobId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub id: SnapshotId,
    pub time: Timestamp,
    pub host: String,
    pub paths: Vec<PathBuf>,
    pub tags: Vec<String>,
    pub tree: TreeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir,
    Symlink {
        target: OsString,
    },
    /// Devices, fifos and sockets: shown, never restored as content.
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Raw name; may not be valid UTF-8.
    pub name: OsString,
    pub kind: NodeKind,
    pub size: u64,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub mtime: Option<Timestamp>,
    pub content: Vec<BlobId>,
    pub subtree: Option<TreeId>,
    /// Hash of everything restic stored for this node, metadata included.
    /// `--strict` compares this instead of the content fingerprint.
    pub raw: Id,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }
}

/// A folder's entries, sorted by name as restic writes them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    pub fn get(&self, name: &OsStr) -> Option<&Node> {
        match self
            .nodes
            .binary_search_by(|n| n.name.as_os_str().cmp(name))
        {
            Ok(i) => Some(&self.nodes[i]),
            // Trees are sorted by restic, but don't rely on it.
            Err(_) => self.nodes.iter().find(|n| n.name == name),
        }
    }

    /// Rough memory use, for the tree cache's size cap.
    pub fn weight(&self) -> u64 {
        self.nodes
            .iter()
            .map(|n| 160 + n.name.len() as u64 + 32 * n.content.len() as u64)
            .sum::<u64>()
            + 32
    }
}

/// The start of a file, up to a limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBytes {
    pub data: Vec<u8>,
    /// The whole file's size; more than `data.len()` when cut at the limit.
    pub size: u64,
}

impl FileBytes {
    pub fn truncated(&self) -> bool {
        (self.data.len() as u64) < self.size
    }
}

/// Read-only access to a restic repository. restoric never writes to it.
pub trait Repo: Send + Sync {
    /// The repository id; names the on-disk cache.
    fn id(&self) -> Id;
    fn snapshots(&self) -> Result<Vec<SnapshotInfo>>;
    fn tree(&self, id: &TreeId) -> Result<Arc<Tree>>;
    /// The first `limit` bytes of a file node.
    fn read_file(&self, node: &Node, limit: u64) -> Result<FileBytes>;
}
