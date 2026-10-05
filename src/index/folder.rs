//! Comparing two versions of a folder: whether its content differs, and how
//! many items were added, changed and deleted under it. Both go only into
//! subtrees whose ids differ, and both are cached by tree ids.

use std::cmp::Ordering;

use anyhow::Result;
use serde::Serialize;

use super::{Index, Mode, NodeRef};
use crate::cache::Table;
use crate::repo::{Node, Tree, TreeId};

/// Items added, changed and deleted under a folder. An item is anything but
/// a folder, or an empty folder, so a change always counts as at least one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub added: u64,
    pub changed: u64,
    pub deleted: u64,
}

impl Counts {
    pub fn is_empty(&self) -> bool {
        *self == Counts::default()
    }

    fn encode(&self) -> Vec<u8> {
        [self.added, self.changed, self.deleted]
            .iter()
            .flat_map(|n| n.to_le_bytes())
            .collect()
    }

    fn decode(b: &[u8]) -> Option<Self> {
        let n = |i: usize| -> Option<u64> {
            Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
        };
        Some(Counts {
            added: n(0)?,
            changed: n(8)?,
            deleted: n(16)?,
        })
    }

    /// `+1 ~3 −1`, leaving out zeros.
    pub fn label(&self, sep: &str) -> String {
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("+{}", self.added));
        }
        if self.changed > 0 {
            parts.push(format!("~{}", self.changed));
        }
        if self.deleted > 0 {
            parts.push(format!("−{}", self.deleted));
        }
        parts.join(sep)
    }
}

impl std::ops::AddAssign for Counts {
    fn add_assign(&mut self, o: Self) {
        self.added += o.added;
        self.changed += o.changed;
        self.deleted += o.deleted;
    }
}

/// Walks two sorted trees side by side.
fn merge<'a>(a: &'a Tree, b: &'a Tree) -> Vec<(Option<&'a Node>, Option<&'a Node>)> {
    let mut a_sorted: Vec<&Node> = a.nodes.iter().collect();
    let mut b_sorted: Vec<&Node> = b.nodes.iter().collect();
    a_sorted.sort_by(|x, y| x.name.cmp(&y.name));
    b_sorted.sort_by(|x, y| x.name.cmp(&y.name));
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a_sorted.len() || j < b_sorted.len() {
        match (a_sorted.get(i), b_sorted.get(j)) {
            (Some(x), Some(y)) => match x.name.cmp(&y.name) {
                Ordering::Less => {
                    out.push((Some(*x), None));
                    i += 1;
                }
                Ordering::Greater => {
                    out.push((None, Some(*y)));
                    j += 1;
                }
                Ordering::Equal => {
                    out.push((Some(*x), Some(*y)));
                    i += 1;
                    j += 1;
                }
            },
            (Some(x), None) => {
                out.push((Some(*x), None));
                i += 1;
            }
            (None, Some(y)) => {
                out.push((None, Some(*y)));
                j += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    out
}

fn key(mode: Mode, a: Option<TreeId>, b: Option<TreeId>) -> Vec<u8> {
    let mut k = vec![mode.byte()];
    k.extend(a.unwrap_or_default().0.0);
    k.extend(b.unwrap_or_default().0.0);
    k
}

impl Index {
    /// Whether two versions of a path differ, under the current mode.
    pub fn refs_differ(&self, a: &NodeRef, b: &NodeRef) -> Result<bool> {
        Ok(match (a, b) {
            (NodeRef::Dir(x), NodeRef::Dir(y)) => self.trees_differ(*x, *y)?,
            (NodeRef::Leaf { fp: f1, raw: r1 }, NodeRef::Leaf { fp: f2, raw: r2 }) => {
                match self.mode {
                    Mode::Content => f1 != f2,
                    Mode::Strict => r1 != r2,
                }
            }
            (x, y) => x != y,
        })
    }

    /// Whether two folder trees differ. Stops at the first difference.
    pub fn trees_differ(&self, a: TreeId, b: TreeId) -> Result<bool> {
        if a == b {
            return Ok(false);
        }
        if self.mode == Mode::Strict {
            return Ok(true);
        }
        let k = key(self.mode, Some(a.min(b)), Some(a.max(b)));
        if let Some(v) = self.cache.get(Table::Differs, &k)? {
            return Ok(v.first() == Some(&1));
        }
        let (ta, tb) = (self.tree(a)?, self.tree(b)?);
        let mut differs = false;
        for pair in merge(&ta, &tb) {
            differs = match pair {
                (Some(x), Some(y)) => match (x.is_dir(), y.is_dir()) {
                    (true, true) => match (x.subtree, y.subtree) {
                        (Some(sx), Some(sy)) => self.trees_differ(sx, sy)?,
                        (sx, sy) => sx != sy,
                    },
                    (false, false) => self.refs_differ(&NodeRef::of(x), &NodeRef::of(y))?,
                    _ => true,
                },
                _ => true,
            };
            if differs {
                break;
            }
        }
        self.cache.put(Table::Differs, k, vec![u8::from(differs)]);
        Ok(differs)
    }

    /// Counts between two versions of a path.
    pub fn ref_counts(&self, a: &NodeRef, b: &NodeRef) -> Result<Counts> {
        let dir = |r: &NodeRef| match r {
            NodeRef::Dir(t) => Some(*t),
            _ => None,
        };
        let items = |r: &NodeRef| -> Result<u64> {
            Ok(match r {
                NodeRef::Missing => 0,
                NodeRef::Dir(t) => self.items(*t)?,
                NodeRef::Leaf { .. } => 1,
            })
        };
        Ok(match (a, b) {
            (NodeRef::Dir(_), NodeRef::Dir(_)) => self.tree_counts(dir(a), dir(b))?,
            _ if !self.refs_differ(a, b)? => Counts::default(),
            (NodeRef::Leaf { .. }, NodeRef::Leaf { .. }) => Counts {
                changed: 1,
                ..Counts::default()
            },
            _ => Counts {
                added: items(b)?,
                deleted: items(a)?,
                changed: 0,
            },
        })
    }

    /// Items under a folder tree (an empty folder is one item).
    fn items(&self, t: TreeId) -> Result<u64> {
        Ok(self.tree_counts(None, Some(t))?.added)
    }

    /// Counts between two folder trees; `None` is a missing folder.
    pub fn tree_counts(&self, a: Option<TreeId>, b: Option<TreeId>) -> Result<Counts> {
        if a == b {
            return Ok(Counts::default());
        }
        let k = key(self.mode, a, b);
        if let Some(c) = self
            .cache
            .get(Table::Counts, &k)?
            .and_then(|v| Counts::decode(&v))
        {
            return Ok(c);
        }
        let empty = std::sync::Arc::new(Tree::default());
        let ta = a
            .map(|t| self.tree(t))
            .transpose()?
            .unwrap_or(empty.clone());
        let tb = b.map(|t| self.tree(t)).transpose()?.unwrap_or(empty);
        let mut c = Counts::default();
        let item = |n: &Node| -> Result<u64> {
            match (n.is_dir(), n.subtree) {
                (true, Some(s)) => Ok(self.tree_counts(None, Some(s))?.added),
                _ => Ok(1),
            }
        };
        for pair in merge(&ta, &tb) {
            match pair {
                (Some(x), None) => c.deleted += item(x)?,
                (None, Some(y)) => c.added += item(y)?,
                (Some(x), Some(y)) => match (x.is_dir(), y.is_dir()) {
                    (true, true) => {
                        c += self.tree_counts(x.subtree, y.subtree)?;
                        if self.mode == Mode::Strict && x.subtree == y.subtree && x.raw != y.raw {
                            c.changed += 1;
                        }
                    }
                    (false, false) => {
                        if self.refs_differ(&NodeRef::of(x), &NodeRef::of(y))? {
                            c.changed += 1;
                        }
                    }
                    _ => {
                        c.deleted += item(x)?;
                        c.added += item(y)?;
                    }
                },
                (None, None) => {}
            }
        }
        // An empty folder is an item of its own.
        if a.is_none() && c.added == 0 {
            c.added = 1;
        }
        if b.is_none() && c.deleted == 0 {
            c.deleted = 1;
        }
        self.cache.put(Table::Counts, k, c.encode());
        Ok(c)
    }
}
