//! What changed on disk since a snapshot (the `on disk` row).
//! A file counts as changed when its kind, size, modification time or
//! permissions differ, which is how restic decides to read it again.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use super::folder::Counts;
use super::listing::{self, Delta, Entry};
use super::{Index, NodeRef};
use crate::disk::{Disk, DiskEntry, DiskKind};
use crate::repo::{Id, Node, NodeKind, SnapshotInfo, Tree, TreeId};

impl Index {
    /// Items added, changed and deleted on disk under `path` since `snap`.
    pub fn live(&self, disk: &dyn Disk, snap: &SnapshotInfo, path: &Path) -> Result<Counts> {
        let on_disk = disk.stat(path);
        Ok(match (self.node_ref(snap, path)?, on_disk) {
            (NodeRef::Missing, None) => Counts::default(),
            (NodeRef::Missing, Some(_)) => Counts {
                added: disk_items(disk, path),
                ..Counts::default()
            },
            (NodeRef::Dir(t), Some(d)) if d.kind == DiskKind::Dir => {
                self.live_tree(disk, t, path)?
            }
            (r, None) => Counts {
                deleted: match r {
                    NodeRef::Dir(t) => self.tree_counts(None, Some(t))?.added,
                    _ => 1,
                },
                ..Counts::default()
            },
            (NodeRef::Dir(t), Some(_)) => Counts {
                deleted: self.tree_counts(None, Some(t))?.added,
                added: 1,
                changed: 0,
            },
            (NodeRef::Leaf { .. }, Some(d)) => {
                let node = self.node_at(snap, path)?;
                let changed = node.is_none_or(|n| d.differs_from(&n));
                Counts {
                    changed: u64::from(changed),
                    ..Counts::default()
                }
            }
        })
    }

    /// The folder's entries on disk, compared with its tree in `newest`:
    /// the `on disk` version, whose version before is the
    /// newest snapshot. With them, the folder's `on disk` counts from the
    /// same walk. `None` when the folder isn't a readable folder on disk.
    pub fn disk_listing(
        &self,
        disk: &dyn Disk,
        newest: Option<&SnapshotInfo>,
        folder: &Path,
    ) -> Result<Option<(Vec<Entry>, Counts)>> {
        let Some(mut on_disk) = disk.read_dir(folder) else {
            return Ok(None);
        };
        on_disk.sort_by(|a, b| a.0.cmp(&b.0));
        let prev = match newest.map(|s| self.node_ref(s, folder)).transpose()? {
            Some(NodeRef::Dir(t)) => self.tree(t)?,
            _ => Arc::new(Tree::default()),
        };
        let mut out = Vec::with_capacity(on_disk.len());
        let mut total = Counts::default();
        // What's in the snapshot but not on disk, or replaced by the other
        // kind of entry: `−` rows, restorable from the newest snapshot.
        let deleted = |b: &Node, out: &mut Vec<Entry>, total: &mut Counts| -> Result<()> {
            total.deleted += self.node_items(b)?;
            out.push(Entry {
                node: b.clone(),
                delta: Delta::Deleted,
            });
            Ok(())
        };
        for (name, d) in &on_disk {
            let child = folder.join(name);
            let before = prev.get(name);
            let delta = match (before, d.kind == DiskKind::Dir) {
                (Some(b), true) if b.is_dir() => {
                    let c = match b.subtree {
                        Some(t) => self.live_tree(disk, t, &child)?,
                        None => Counts::default(),
                    };
                    total += c;
                    Delta::Counts(c)
                }
                (b, true) => {
                    // New, or a file that became a folder.
                    if let Some(b) = b {
                        deleted(b, &mut out, &mut total)?;
                    }
                    let c = Counts {
                        added: disk_items(disk, &child),
                        ..Counts::default()
                    };
                    total += c;
                    Delta::Counts(c)
                }
                (None, false) => {
                    total.added += 1;
                    Delta::Added
                }
                (Some(b), false) if b.is_dir() => {
                    deleted(b, &mut out, &mut total)?;
                    total.added += 1;
                    Delta::Added
                }
                (Some(b), false) if d.differs_from(b) => {
                    total.changed += 1;
                    Delta::Changed
                }
                (Some(_), false) => Delta::Same,
            };
            out.push(Entry {
                node: disk_node(name, d),
                delta,
            });
        }
        for b in &prev.nodes {
            if on_disk
                .binary_search_by(|(name, _)| name.cmp(&b.name))
                .is_err()
            {
                deleted(b, &mut out, &mut total)?;
            }
        }
        listing::sort(&mut out);
        Ok(Some((out, total)))
    }

    /// Items deleted from `folder` before the newest snapshot of `set`
    /// that aren't on disk either: the `gone` rows of the `on disk` version.
    pub fn disk_deleted_earlier(
        &self,
        disk: &dyn Disk,
        set: &[SnapshotInfo],
        folder: &Path,
    ) -> Result<Vec<Entry>> {
        let Some(n) = set.len().checked_sub(1) else {
            return Ok(Vec::new());
        };
        let mut now = self.names_at(&set[n], folder)?;
        now.extend(
            disk.read_dir(folder)
                .unwrap_or_default()
                .into_iter()
                .map(|(name, _)| name),
        );
        self.deleted_before(set, n, &now, folder)
    }

    /// Items a node stands for: those under a folder, else one.
    fn node_items(&self, n: &Node) -> Result<u64> {
        Ok(match n.subtree {
            Some(t) if n.is_dir() => self.tree_counts(None, Some(t))?.added,
            _ => 1,
        })
    }

    fn live_tree(&self, disk: &dyn Disk, tree: TreeId, path: &Path) -> Result<Counts> {
        let tree = self.tree(tree)?;
        let mut on_disk = disk.read_dir(path).unwrap_or_default();
        on_disk.sort_by(|a, b| a.0.cmp(&b.0));
        let mut c = Counts::default();
        for (name, d) in &on_disk {
            let child = path.join(name);
            match tree.get(name) {
                None => c.added += disk_items(disk, &child),
                Some(n) => match (n.is_dir(), d.kind == DiskKind::Dir, n.subtree) {
                    (true, true, Some(t)) => c += self.live_tree(disk, t, &child)?,
                    (false, false, _) => c.changed += u64::from(d.differs_from(n)),
                    _ => {
                        c.deleted += self.node_items(n)?;
                        c.added += disk_items(disk, &child);
                    }
                },
            }
        }
        for n in &tree.nodes {
            if on_disk
                .binary_search_by(|(name, _)| name.cmp(&n.name))
                .is_err()
            {
                c.deleted += self.node_items(n)?;
            }
        }
        Ok(c)
    }
}

/// A node for an entry on disk, for the listing: what the UI shows and
/// nothing more. It has no content, so it must never be read from the
/// repository; the `on disk` version reads its files from disk.
fn disk_node(name: &OsStr, d: &DiskEntry) -> Node {
    Node {
        name: name.to_os_string(),
        kind: match d.kind {
            DiskKind::File => NodeKind::File,
            DiskKind::Dir => NodeKind::Dir,
            DiskKind::Symlink => NodeKind::Symlink {
                target: OsString::new(),
            },
            DiskKind::Other => NodeKind::Other(String::new()),
        },
        size: d.size,
        mode: d.mode,
        uid: None,
        gid: None,
        mtime: d.mtime,
        content: Vec::new(),
        subtree: None,
        raw: Id::default(),
    }
}

/// Items under a path on disk (an empty folder is one item).
fn disk_items(disk: &dyn Disk, path: &Path) -> u64 {
    match disk.stat(path) {
        Some(d) if d.kind == DiskKind::Dir => {
            let n: u64 = disk
                .read_dir(path)
                .unwrap_or_default()
                .iter()
                .map(|(name, _)| disk_items(disk, &path.join(name)))
                .sum();
            n.max(1)
        }
        Some(_) => 1,
        None => 0,
    }
}
