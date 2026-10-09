//! `:find NAME`: where something with NAME in its path was,
//! across every snapshot. Matches are memoised by (folder path, tree id),
//! so a folder that didn't change is read once; the cost follows the number
//! of distinct trees, not snapshots × files.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::{Index, NodeRef};
use crate::repo::{SnapshotInfo, TreeId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Relative to the folder searched from.
    pub path: PathBuf,
    pub is_dir: bool,
    /// First and last snapshot with it (indexes into the set).
    pub first: usize,
    pub last: usize,
}

/// Matches under one tree: (path relative to the search root, is a folder).
type Matches = Arc<Vec<(PathBuf, bool)>>;

impl Index {
    /// Everything under `root` whose path (relative to `root`) contains
    /// `query`, ignoring case. A match's contents aren't listed again.
    /// `progress(done, total, partial results)` is called as it goes; when
    /// it returns false, the search stops there.
    pub fn find(
        &self,
        set: &[SnapshotInfo],
        root: &Path,
        query: &str,
        progress: &mut dyn FnMut(usize, usize, Vec<Found>) -> bool,
    ) -> Result<Vec<Found>> {
        let q = super::fold(query);
        let mut memo: HashMap<(PathBuf, TreeId), Matches> = HashMap::new();
        let mut found: BTreeMap<PathBuf, Found> = BTreeMap::new();
        for (i, s) in set.iter().enumerate() {
            if let NodeRef::Dir(t) = self.node_ref(s, root)? {
                for (p, is_dir) in self.find_in(&mut memo, Path::new(""), t, &q)?.iter() {
                    found
                        .entry(p.clone())
                        .and_modify(|f| f.last = i)
                        .or_insert(Found {
                            path: p.clone(),
                            is_dir: *is_dir,
                            first: i,
                            last: i,
                        });
                }
            }
            if (i + 1).is_multiple_of(64)
                && i + 1 < set.len()
                && !progress(i + 1, set.len(), found.values().cloned().collect())
            {
                break;
            }
        }
        Ok(found.into_values().collect())
    }

    fn find_in(
        &self,
        memo: &mut HashMap<(PathBuf, TreeId), Matches>,
        rel: &Path,
        tree: TreeId,
        q: &str,
    ) -> Result<Matches> {
        let key = (rel.to_path_buf(), tree);
        if let Some(m) = memo.get(&key) {
            return Ok(m.clone());
        }
        let mut out = Vec::new();
        for n in &self.tree(tree)?.nodes {
            let p = rel.join(&n.name);
            if super::fold(&p.to_string_lossy()).contains(q) {
                out.push((p, n.is_dir()));
            } else if let (true, Some(sub)) = (n.is_dir(), n.subtree) {
                out.extend(self.find_in(memo, &p, sub, q)?.iter().cloned());
            }
        }
        let m = Arc::new(out);
        memo.insert(key, m.clone());
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;
    use crate::index::Mode;
    use crate::repo::Repo;
    use crate::repo::fake::FakeRepo;

    #[test]
    fn finds_across_snapshots() {
        let repo = Arc::new(
            FakeRepo::parse(
                "root /p
snapshot 2026-01-01 00:00
  write src/legacy.go a
  write src/main.go m
  write old/legacy/x.go x
snapshot 2026-01-02 00:00
  rm src/legacy.go
snapshot 2026-01-03 00:00
  rm old
  write src/LEGACY.md new
",
            )
            .unwrap(),
        );
        let set = repo.snapshots().unwrap();
        let index = Index::new(repo, Cache::in_memory(), Mode::Content, 1 << 20);
        let found = index
            .find(&set, Path::new("/p"), "legacy", &mut |_, _, _| true)
            .unwrap();
        let got: Vec<(String, bool, usize, usize)> = found
            .iter()
            .map(|f| (f.path.display().to_string(), f.is_dir, f.first, f.last))
            .collect();
        assert_eq!(
            got,
            [
                ("old/legacy".into(), true, 0, 1),
                ("src/LEGACY.md".into(), false, 2, 2),
                ("src/legacy.go".into(), false, 0, 0),
            ]
        );
    }
}
