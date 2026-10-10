//! The repository as the rest of restoric sees it: the [`Repo`] trait and our
//! own types. rustic_core types stay inside `rustic.rs`.

pub mod fake;
pub mod rustic;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Component, Path, PathBuf};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub id: SnapshotId,
    pub time: Timestamp,
    pub host: String,
    pub paths: Vec<PathBuf>,
    pub tags: Vec<String>,
    pub tree: TreeId,
    /// rustic's `--label`; empty when unset.
    pub label: String,
    pub description: Option<String>,
    /// What made it: "restic 0.17.3", "rustic v0.9.5".
    pub program: String,
    pub parent: Option<SnapshotId>,
    pub summary: Option<Summary>,
}

/// What the backup recorded about its run (restic 0.17 and later, rustic).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files new, changed and unchanged since the parent snapshot.
    pub files: (u64, u64, u64),
    /// Folders new, changed and unchanged.
    pub dirs: (u64, u64, u64),
    /// What the backup read.
    pub total_files: u64,
    pub total_bytes: u64,
    /// Bytes it added to the repository, before and after compression.
    pub added: u64,
    pub added_packed: u64,
    pub duration: jiff::SignedDuration,
    /// The command line; rustic records it, restic doesn't.
    pub command: String,
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

/// Whether `name` is one plain path component, safe to join onto a folder:
/// not empty, `.` or `..`, and without a separator, root, prefix or NUL.
/// A file system never makes anything else; a crafted repository can, and
/// joining such a name would reach outside the folder.
pub fn is_plain_name(name: &OsStr) -> bool {
    let mut parts = Path::new(name).components();
    matches!(parts.next(), Some(Component::Normal(n)) if n == name)
        && parts.next().is_none()
        && !name.as_encoded_bytes().contains(&0)
}

impl Tree {
    /// A tree as a repository stored it, without the entries whose name
    /// isn't [plain](is_plain_name): they're logged and never shown,
    /// previewed or restored.
    pub fn from_stored(nodes: Vec<Node>) -> Self {
        let nodes = nodes
            .into_iter()
            .filter(|n| {
                let plain = is_plain_name(&n.name);
                if !plain {
                    tracing::warn!("leaving out a tree entry named {:?}", n.name);
                }
                plain
            })
            .collect();
        Self { nodes }
    }

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

/// How far a restore has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStep {
    /// Finding out what to restore; the size isn't known yet.
    Preparing,
    /// Copying file contents.
    Bytes { done: u64, total: u64 },
}

/// The error of a restore that was stopped on request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for Stopped {}

/// Whether an error is a [`Stopped`].
pub fn is_stopped(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Stopped>().is_some()
}

/// The start of a file, up to a limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBytes {
    pub data: Vec<u8>,
    /// The whole file's size; more than `data.len()` when cut at the limit.
    pub size: u64,
    /// A file on disk's permission bits; from the repository, the node has them.
    pub mode: Option<u32>,
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
    /// Up to `len` bytes of a file node, from `offset`.
    fn read_at(&self, node: &Node, offset: u64, len: u64) -> Result<Vec<u8>>;
    /// Restores what's at `path` in `snap` (a file, link or folder) to
    /// `dest`, which must not exist yet; its parent must. Keeps mode,
    /// modification time and symlinks; owner and group only as root.
    ///
    /// Reports how far it got through `progress`. `cancelled` is asked once
    /// the restore is planned, before file contents are copied; if it says
    /// yes, whatever was created at `dest` is removed and the error is
    /// [`Stopped`]. Once copying has started it runs to the end.
    fn restore(
        &self,
        snap: &SnapshotInfo,
        path: &Path,
        dest: &Path,
        progress: &mut dyn FnMut(RestoreStep),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()>;

    /// The first `limit` bytes of a file node.
    fn read_file(&self, node: &Node, limit: u64) -> Result<FileBytes> {
        Ok(FileBytes {
            data: self.read_at(node, 0, limit.min(node.size))?,
            size: node.size,
            mode: None,
        })
    }
}

/// Whether restoric runs as root, so restores may set owner and group.
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and can't fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &[u8]) -> Node {
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            OsStr::from_bytes(name).to_os_string()
        };
        #[cfg(not(unix))]
        let name = OsString::from(String::from_utf8_lossy(name).into_owned());
        Node {
            name,
            kind: NodeKind::File,
            size: 0,
            mode: None,
            uid: None,
            gid: None,
            mtime: None,
            content: Vec::new(),
            subtree: None,
            raw: Id::default(),
        }
    }

    #[test]
    fn plain_names_only() {
        let plain = |s: &str| is_plain_name(OsStr::new(s));
        for ok in ["main.go", ".bashrc", "...", "a b", "x."] {
            assert!(plain(ok), "{ok:?} is plain");
        }
        // A backslash is a separator only on Windows.
        assert_eq!(plain("back\\slash"), cfg!(unix));
        for bad in [
            "", ".", "..", "/", "/etc", "a/b", "../x", "a/", "x/..", "a\0b",
        ] {
            assert!(!plain(bad), "{bad:?} isn't plain");
        }
    }

    #[test]
    fn a_stored_tree_leaves_out_names_that_reach_outside() {
        let t = Tree::from_stored(
            [
                &b"a"[..],
                b"..",
                b"../../.bashrc",
                b"/etc/passwd",
                b"b",
                b"c/d",
            ]
            .into_iter()
            .map(node)
            .collect(),
        );
        let names: Vec<&OsStr> = t.nodes.iter().map(|n| n.name.as_os_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }
}
