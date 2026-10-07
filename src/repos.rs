//! Which `[[repo]]` from the config holds the folder (PLAN.md §4.6).
//!
//! restoric reads each repository's snapshot list and picks the one whose
//! snapshots of this machine hold the folder. What it saw is remembered in
//! `repos.json`, so a usual start opens only the repository it needs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::index::timeline::{Filter, holds};
use crate::repo::SnapshotInfo;

/// A repository checked this recently isn't read again on a miss.
pub const RECHECK: SignedDuration = SignedDuration::from_mins(5);

/// What a snapshot says about where it came from, without its id or time.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Root {
    pub host: String,
    pub tags: Vec<String>,
    pub paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Seen {
    /// When the snapshot list was read, in seconds since the epoch.
    checked: i64,
    roots: BTreeSet<Root>,
}

/// The snapshot roots seen in each repository, by its location.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProbeCache {
    repos: BTreeMap<String, Seen>,
}

impl ProbeCache {
    pub fn default_path() -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "restoric")?;
        Some(dirs.cache_dir().join("repos.json"))
    }

    /// The file at `p`, or an empty cache if it's missing or unreadable:
    /// everything in it can be read again.
    pub fn load(p: &Path) -> Self {
        std::fs::read(p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, p: &Path) -> Result<()> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, serde_json::to_vec(self)?)
            .with_context(|| format!("writing {}", p.display()))
    }

    /// Remembers what `repo` holds as of `now`.
    pub fn record(&mut self, repo: &str, snaps: &[SnapshotInfo], now: Timestamp) {
        let roots = snaps
            .iter()
            .map(|s| Root {
                host: s.host.clone(),
                tags: s.tags.clone(),
                paths: s.paths.clone(),
            })
            .collect();
        self.repos.insert(
            repo.to_string(),
            Seen {
                checked: now.as_second(),
                roots,
            },
        );
    }

    fn fresh(&self, repo: &str, now: Timestamp) -> bool {
        self.repos
            .get(repo)
            .is_some_and(|s| now.as_second() - s.checked < RECHECK.as_secs())
    }

    /// How closely `c`'s remembered snapshots of ours hold `folder`.
    fn fit(&self, c: &Candidate, folder: &Path) -> Option<Fit> {
        self.repos
            .get(&c.repo)?
            .roots
            .iter()
            .filter(|r| c.filter.accepts(&r.host, &r.tags))
            .filter_map(|r| fit(&r.paths, folder))
            .max()
    }

    /// The candidate whose remembered snapshots hold `folder` most closely,
    /// leaving out `skip`. Earlier candidates win ties.
    fn best(&self, cands: &[Candidate], folder: &Path, skip: &BTreeSet<usize>) -> Option<usize> {
        let mut best: Option<(Fit, usize)> = None;
        for (i, c) in cands.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            if let Some(f) = self.fit(c, folder)
                && best.as_ref().is_none_or(|(b, _)| f > *b)
            {
                best = Some((f, i));
            }
        }
        best.map(|(_, i)| i)
    }
}

/// How closely backup paths hold a folder: a path at or above the folder
/// holds all of it and beats one below it, and a deeper path beats a
/// shallower one.
type Fit = (bool, usize);

fn fit(paths: &[PathBuf], folder: &Path) -> Option<Fit> {
    if !holds(paths, folder) {
        return None;
    }
    paths
        .iter()
        .filter_map(|p| {
            if folder.starts_with(p) {
                Some((true, p.components().count()))
            } else if p.starts_with(folder) {
                Some((false, 0))
            } else {
                None
            }
        })
        .max()
}

/// A `[[repo]]` to look in: its location and which snapshots are ours.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub repo: String,
    pub filter: Filter,
}

/// Why `connect` is called: `true` when looking through repositories after
/// a miss, `false` when opening the one the cache picked.
pub type Probing = bool;

/// Picks the candidate that holds `folder` and returns its index and the
/// connection `connect` made to it. `connect(i, probing)` opens candidate
/// `i` and lists its snapshots; a failure goes to `warn` and that candidate
/// is skipped. Updates `cache` with every list read.
pub fn choose<T>(
    cands: &[Candidate],
    folder: &Path,
    cache: &mut ProbeCache,
    now: Timestamp,
    mut connect: impl FnMut(usize, Probing) -> Result<(T, Vec<SnapshotInfo>)>,
    mut warn: impl FnMut(usize, &anyhow::Error),
) -> Option<(usize, T)> {
    let mut tried = BTreeSet::new();
    // The cache's pick, checked against its snapshot list as it is now.
    if let Some(i) = cache.best(cands, folder, &tried) {
        tried.insert(i);
        match connect(i, false) {
            Ok((t, snaps)) => {
                cache.record(&cands[i].repo, &snaps, now);
                if cache.fit(&cands[i], folder).is_some() {
                    return Some((i, t));
                }
            }
            Err(e) => warn(i, &e),
        }
    }
    // A miss: read the lists the cache doesn't know well enough.
    let mut conns = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        if tried.contains(&i) || cache.fresh(&c.repo, now) {
            continue;
        }
        match connect(i, true) {
            Ok((t, snaps)) => {
                cache.record(&c.repo, &snaps, now);
                conns.insert(i, t);
            }
            Err(e) => warn(i, &e),
        }
    }
    let i = cache.best(cands, folder, &tried)?;
    match conns.remove(&i) {
        Some(t) => Some((i, t)),
        None => match connect(i, false) {
            Ok((t, snaps)) => {
                cache.record(&cands[i].repo, &snaps, now);
                Some((i, t))
            }
            Err(e) => {
                warn(i, &e);
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{SnapshotId, TreeId};
    use anyhow::anyhow;
    use std::cell::RefCell;

    fn snap(host: &str, tag: &str, path: &str) -> SnapshotInfo {
        SnapshotInfo {
            id: SnapshotId::default(),
            time: Timestamp::UNIX_EPOCH,
            host: host.into(),
            paths: vec![PathBuf::from(path)],
            tags: if tag.is_empty() {
                vec![]
            } else {
                vec![tag.into()]
            },
            tree: TreeId::default(),
        }
    }

    fn cand(repo: &str) -> Candidate {
        Candidate {
            repo: repo.into(),
            filter: Filter {
                hosts: vec!["me".into()],
                tag: None,
            },
        }
    }

    /// Repositories `name → snapshots`, plus a log of what was opened.
    struct World {
        repos: HashMap<&'static str, Vec<SnapshotInfo>>,
        opened: RefCell<Vec<(String, Probing)>>,
    }

    impl World {
        fn new(repos: &[(&'static str, Vec<SnapshotInfo>)]) -> Self {
            Self {
                repos: repos.iter().cloned().collect(),
                opened: RefCell::default(),
            }
        }

        fn choose(
            &self,
            cands: &[Candidate],
            folder: &str,
            cache: &mut ProbeCache,
            now: Timestamp,
        ) -> Option<String> {
            choose(
                cands,
                Path::new(folder),
                cache,
                now,
                |i, probing| {
                    let name = &cands[i].repo;
                    self.opened.borrow_mut().push((name.clone(), probing));
                    let snaps = self
                        .repos
                        .get(name.as_str())
                        .ok_or_else(|| anyhow!("{name} is down"))?;
                    Ok((name.clone(), snaps.clone()))
                },
                |_, _| {},
            )
            .map(|(_, t)| t)
        }

        fn opened(&self) -> Vec<(String, Probing)> {
            std::mem::take(&mut self.opened.borrow_mut())
        }
    }

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_second(secs).unwrap()
    }

    #[test]
    fn a_miss_reads_every_list_then_a_hit_opens_one() {
        let w = World::new(&[
            ("a", vec![snap("me", "", "/home/me/photos")]),
            ("b", vec![snap("me", "", "/home/me/dev")]),
        ]);
        let cands = [cand("a"), cand("b")];
        let mut cache = ProbeCache::default();
        assert_eq!(
            w.choose(&cands, "/home/me/dev/x", &mut cache, at(0)),
            Some("b".into())
        );
        assert_eq!(w.opened(), [("a".into(), true), ("b".into(), true)]);
        // Next start: only b, and not as a probe.
        assert_eq!(
            w.choose(&cands, "/home/me/dev/x", &mut cache, at(9999)),
            Some("b".into())
        );
        assert_eq!(w.opened(), [("b".into(), false)]);
    }

    #[test]
    fn the_closest_backup_path_wins_then_config_order() {
        let w = World::new(&[
            ("home", vec![snap("me", "", "/home/me")]),
            ("dev", vec![snap("me", "", "/home/me/dev")]),
            ("dev2", vec![snap("me", "", "/home/me/dev")]),
            ("below", vec![snap("me", "", "/home/me/dev/x/y")]),
        ]);
        let cands = [cand("home"), cand("below"), cand("dev2"), cand("dev")];
        let mut cache = ProbeCache::default();
        assert_eq!(
            w.choose(&cands, "/home/me/dev/x", &mut cache, at(0)),
            Some("dev2".into())
        );
        // A folder above every backup path: one held below still counts.
        assert_eq!(
            w.choose(&cands, "/home", &mut ProbeCache::default(), at(0)),
            Some("home".into())
        );
    }

    #[test]
    fn only_this_machines_snapshots_count() {
        let w = World::new(&[
            ("other", vec![snap("laptop", "", "/home/me/dev")]),
            ("tagged", vec![snap("me", "work", "/home/me/dev")]),
        ]);
        let mut tagged = cand("tagged");
        tagged.filter.tag = Some("work".into());
        let cands = [cand("other"), tagged];
        assert_eq!(
            w.choose(&cands, "/home/me/dev", &mut ProbeCache::default(), at(0)),
            Some("tagged".into())
        );
    }

    #[test]
    fn a_repository_that_fails_is_skipped() {
        let w = World::new(&[("up", vec![snap("me", "", "/home/me")])]);
        let cands = [cand("down"), cand("up")];
        assert_eq!(
            w.choose(&cands, "/home/me/dev", &mut ProbeCache::default(), at(0)),
            Some("up".into())
        );
    }

    #[test]
    fn a_stale_hit_reads_the_others() {
        let mut w = World::new(&[("a", vec![snap("me", "", "/home/me/dev")]), ("b", vec![])]);
        let cands = [cand("a"), cand("b")];
        let mut cache = ProbeCache::default();
        w.choose(&cands, "/home/me/dev", &mut cache, at(0));
        w.opened();
        // The folder's backups moved from a to b.
        w.repos.insert("a", vec![]);
        w.repos.insert("b", vec![snap("me", "", "/home/me/dev")]);
        assert_eq!(
            w.choose(&cands, "/home/me/dev", &mut cache, at(1000)),
            Some("b".into())
        );
        assert_eq!(w.opened(), [("a".into(), false), ("b".into(), true)]);
    }

    #[test]
    fn a_list_read_recently_isnt_read_again() {
        let w = World::new(&[("a", vec![snap("me", "", "/home/me/dev")])]);
        let cands = [cand("a")];
        let mut cache = ProbeCache::default();
        assert_eq!(w.choose(&cands, "/srv", &mut cache, at(0)), None);
        assert_eq!(w.opened(), [("a".into(), true)]);
        assert_eq!(w.choose(&cands, "/srv", &mut cache, at(60)), None);
        assert_eq!(w.opened(), []);
        assert_eq!(w.choose(&cands, "/srv", &mut cache, at(301)), None);
        assert_eq!(w.opened(), [("a".into(), true)]);
    }

    #[test]
    fn the_cache_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("restoric-repos-{}", std::process::id()));
        let p = dir.join("repos.json");
        let mut cache = ProbeCache::default();
        cache.record("a", &[snap("me", "t", "/x")], at(5));
        cache.save(&p).unwrap();
        let back = ProbeCache::load(&p);
        assert_eq!(back.repos["a"].checked, 5);
        assert_eq!(back.repos["a"].roots.len(), 1);
        std::fs::write(&p, "not json").unwrap();
        assert!(ProbeCache::load(&p).repos.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
