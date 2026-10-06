//! What changed on disk since a snapshot (PLAN.md §3.1, the `on disk` row).
//! A file counts as changed when its kind, size, modification time or
//! permissions differ, which is how restic decides to read it again.

use std::path::Path;

use anyhow::Result;

use super::folder::Counts;
use super::{Index, NodeRef};
use crate::disk::{Disk, DiskKind};
use crate::repo::{SnapshotInfo, TreeId};

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
                        c.deleted += match n.subtree {
                            Some(t) if n.is_dir() => self.tree_counts(None, Some(t))?.added,
                            _ => 1,
                        };
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
                c.deleted += match n.subtree {
                    Some(t) if n.is_dir() => self.tree_counts(None, Some(t))?.added,
                    _ => 1,
                };
            }
        }
        Ok(c)
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
