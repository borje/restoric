//! The files on disk, for "what changed since the newest snapshot" and
//! "vs disk" (PLAN.md §3.1, §3.3). A trait, so tests can use a fake disk.
//! Symlinks are never followed.

use std::ffi::OsString;
use std::path::Path;

use jiff::Timestamp;

use crate::repo::{Node, NodeKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskEntry {
    pub kind: DiskKind,
    pub size: u64,
    pub mtime: Option<Timestamp>,
    /// Permission bits (0o777) only.
    pub mode: Option<u32>,
}

impl DiskEntry {
    /// Whether the file on disk differs from `node`, the way restic decides
    /// whether to read a file again: kind, size, modification time and mode.
    pub fn differs_from(&self, node: &Node) -> bool {
        let kind = match node.kind {
            NodeKind::File => DiskKind::File,
            NodeKind::Dir => DiskKind::Dir,
            NodeKind::Symlink { .. } => DiskKind::Symlink,
            NodeKind::Other(_) => DiskKind::Other,
        };
        if kind != self.kind {
            return true;
        }
        if kind != DiskKind::File {
            return false;
        }
        let same_mode = match (self.mode, node.mode) {
            (Some(a), Some(b)) => a & 0o777 == b & 0o777,
            _ => true,
        };
        let same_time = match (self.mtime, node.mtime) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        self.size != node.size || !same_time || !same_mode
    }
}

pub trait Disk: Send + Sync {
    fn stat(&self, path: &Path) -> Option<DiskEntry>;
    /// A folder's entries, or `None` if it isn't a readable folder.
    fn read_dir(&self, path: &Path) -> Option<Vec<(OsString, DiskEntry)>>;
    /// The first `limit` bytes of a file.
    fn read(&self, path: &Path, limit: u64) -> Option<Vec<u8>>;
}

/// The real file system.
pub struct RealDisk;

fn entry(m: &std::fs::Metadata) -> DiskEntry {
    let ft = m.file_type();
    let kind = if ft.is_symlink() {
        DiskKind::Symlink
    } else if ft.is_dir() {
        DiskKind::Dir
    } else if ft.is_file() {
        DiskKind::File
    } else {
        DiskKind::Other
    };
    let mtime = m.modified().ok().and_then(|t| Timestamp::try_from(t).ok());
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(m.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let mode = None;
    DiskEntry {
        kind,
        size: if ft.is_file() { m.len() } else { 0 },
        mtime,
        mode,
    }
}

impl Disk for RealDisk {
    fn stat(&self, path: &Path) -> Option<DiskEntry> {
        std::fs::symlink_metadata(path).ok().map(|m| entry(&m))
    }

    fn read_dir(&self, path: &Path) -> Option<Vec<(OsString, DiskEntry)>> {
        let rd = std::fs::read_dir(path).ok()?;
        Some(
            rd.filter_map(Result::ok)
                .filter_map(|e| Some((e.file_name(), entry(&e.path().symlink_metadata().ok()?))))
                .collect(),
        )
    }

    fn read(&self, path: &Path, limit: u64) -> Option<Vec<u8>> {
        use std::io::Read;
        if self.stat(path)?.kind != DiskKind::File {
            return None;
        }
        let mut out = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(limit)
            .read_to_end(&mut out)
            .ok()?;
        Some(out)
    }
}
