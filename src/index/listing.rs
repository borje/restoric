//! A folder's entries at one snapshot, with change markers against the
//! snapshot before (PLAN.md §3.1), and items deleted earlier.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;

use super::folder::Counts;
use super::{Index, NodeRef};
use crate::repo::{Node, SnapshotInfo, Tree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delta {
    /// Unchanged since the snapshot before.
    Same,
    /// A file or link that's new.
    Added,
    /// A file or link whose content, mode or owner changed.
    Changed,
    /// There in the snapshot before, gone in this one.
    Deleted,
    /// A folder: what changed under it (empty when nothing did).
    Counts(Counts),
    /// Deleted before the snapshot before; the last snapshot that had it.
    Gone(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The node in this snapshot, or the last one for a deleted entry.
    pub node: Node,
    pub delta: Delta,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.node.is_dir()
    }

    /// Deleted in this snapshot (`−`).
    pub fn is_deleted(&self) -> bool {
        self.delta == Delta::Deleted
    }

    /// Deleted in this snapshot or earlier: not in this snapshot at all.
    pub fn is_gone(&self) -> bool {
        matches!(self.delta, Delta::Deleted | Delta::Gone(_))
    }
}

/// Folders first, then by name, ignoring case.
pub fn cmp(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    b.is_dir().cmp(&a.is_dir()).then_with(|| {
        let (x, y) = (a.node.name.to_string_lossy(), b.node.name.to_string_lossy());
        x.to_lowercase()
            .cmp(&y.to_lowercase())
            .then_with(|| a.node.name.cmp(&b.node.name))
    })
}

pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(cmp);
}

impl Index {
    /// The entries of `folder` at `set[i]`, compared with `set[i - 1]`.
    /// `None` if the folder doesn't exist at `set[i]`.
    pub fn listing(
        &self,
        set: &[SnapshotInfo],
        i: usize,
        folder: &Path,
    ) -> Result<Option<Vec<Entry>>> {
        let NodeRef::Dir(cur) = self.node_ref(&set[i], folder)? else {
            return Ok(None);
        };
        let cur = self.tree(cur)?;
        let prev = match i.checked_sub(1) {
            Some(p) => match self.node_ref(&set[p], folder)? {
                NodeRef::Dir(t) => self.tree(t)?,
                _ => std::sync::Arc::new(Tree::default()),
            },
            None => std::sync::Arc::new(Tree::default()),
        };
        let mut out = Vec::with_capacity(cur.nodes.len());
        for n in &cur.nodes {
            let before = prev.get(&n.name);
            let delta = if n.is_dir() {
                let was = before.filter(|b| b.is_dir()).and_then(|b| b.subtree);
                Delta::Counts(self.tree_counts(was, n.subtree)?)
            } else {
                match before {
                    None => Delta::Added,
                    Some(b) if b.is_dir() => Delta::Added,
                    Some(b) if self.refs_differ(&NodeRef::of(b), &NodeRef::of(n))? => {
                        Delta::Changed
                    }
                    Some(_) => Delta::Same,
                }
            };
            out.push(Entry {
                node: n.clone(),
                delta,
            });
        }
        for b in &prev.nodes {
            if cur.get(&b.name).is_none() {
                out.push(Entry {
                    node: b.clone(),
                    delta: Delta::Deleted,
                });
            }
        }
        sort(&mut out);
        Ok(Some(out))
    }

    /// Items in `folder` at some snapshot before `set[i - 1]` that are in
    /// neither `set[i - 1]` nor `set[i]`: deleted earlier. Each comes with
    /// its node and the last snapshot that had it. Reads one tree per
    /// distinct earlier version of the folder.
    pub fn deleted_earlier(
        &self,
        set: &[SnapshotInfo],
        i: usize,
        folder: &Path,
    ) -> Result<Vec<Entry>> {
        let mut now = BTreeSet::new();
        for j in [i.checked_sub(1), Some(i)].into_iter().flatten() {
            now.extend(self.names_at(&set[j], folder)?);
        }
        self.deleted_before(set, i.saturating_sub(1), &now, folder)
    }

    /// The names in `folder` at `snap`; none if it isn't a folder there.
    pub fn names_at(&self, snap: &SnapshotInfo, folder: &Path) -> Result<BTreeSet<OsString>> {
        Ok(match self.node_ref(snap, folder)? {
            NodeRef::Dir(t) => self.tree(t)?.nodes.iter().map(|n| n.name.clone()).collect(),
            _ => BTreeSet::new(),
        })
    }

    /// Items in `folder` at some snapshot before `set[upto]` whose names
    /// aren't in `now`, with the last snapshot that had each.
    pub fn deleted_before(
        &self,
        set: &[SnapshotInfo],
        upto: usize,
        now: &BTreeSet<OsString>,
        folder: &Path,
    ) -> Result<Vec<Entry>> {
        // Name → (last snapshot with it, its node there).
        let mut last: BTreeMap<OsString, (usize, Node)> = BTreeMap::new();
        let mut prev = None;
        for (j, s) in set.iter().enumerate().take(upto) {
            let NodeRef::Dir(t) = self.node_ref(s, folder)? else {
                prev = None;
                continue;
            };
            if prev == Some(t) {
                // Same tree as the snapshot before: the same names, seen later.
                for v in last.values_mut() {
                    if v.0 + 1 == j {
                        v.0 = j;
                    }
                }
                continue;
            }
            prev = Some(t);
            for n in &self.tree(t)?.nodes {
                if !now.contains(&n.name) {
                    last.insert(n.name.clone(), (j, n.clone()));
                }
            }
        }
        let mut out: Vec<Entry> = last
            .into_values()
            .map(|(j, node)| Entry {
                node,
                delta: Delta::Gone(j),
            })
            .collect();
        sort(&mut out);
        Ok(out)
    }
}
