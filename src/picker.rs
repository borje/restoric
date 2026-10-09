//! The repository picker: a start-up screen for reaching
//! snapshots the folder-anchored flow can't. It lists (host, backup path)
//! groups and, with several `[[repo]]`, a repo level above them drawn from
//! `repos.json` without opening any repository.
//!
//! This is pure state apart from [`Pick::open_at`]: the driver (`tui::Term::pick`) opens repositories
//! when [`Step::Open`] or [`Step::Refresh`] asks and reports back with
//! [`Picker::opened`], [`Picker::refreshed`] or [`Picker::failed`].

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::index::{Index, fold};
use crate::repo::SnapshotInfo;
use crate::repos::{Candidate, ProbeCache, fit};

/// One (host, backup path) group of snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRow {
    pub host: String,
    pub path: PathBuf,
    pub snapshots: usize,
    pub latest: Timestamp,
    /// The host is one of this machine's.
    pub mine: bool,
}

/// One row per (host, backup path): this machine's hosts first, then by
/// newest snapshot. A snapshot with several paths counts once in each.
pub fn groups_of(snaps: &[SnapshotInfo], mine: &[String]) -> Vec<GroupRow> {
    let mut by: BTreeMap<(&str, &PathBuf), (usize, Timestamp)> = BTreeMap::new();
    for s in snaps {
        let mut paths: Vec<&PathBuf> = s.paths.iter().collect();
        paths.sort();
        paths.dedup();
        for p in paths {
            let e = by.entry((&s.host, p)).or_insert((0, s.time));
            e.0 += 1;
            e.1 = e.1.max(s.time);
        }
    }
    let mut rows: Vec<GroupRow> = by
        .into_iter()
        .map(|((host, path), (snapshots, latest))| GroupRow {
            host: host.to_string(),
            path: path.clone(),
            snapshots,
            latest,
            mine: mine.iter().any(|m| m == host),
        })
        .collect();
    rows.sort_by_key(|r| (!r.mine, Reverse(r.latest)));
    rows
}

/// The row whose path holds `path` most closely (at or above, deeper
/// first; then below), earlier rows winning ties; 0 if none holds it.
pub fn closest(rows: &[GroupRow], path: &Path) -> usize {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| fit(std::slice::from_ref(&r.path), path).map(|f| (f, Reverse(i))))
        .max()
        .map_or(0, |(_, Reverse(i))| i)
}

/// One `[[repo]]` at the repo level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoRow {
    /// Index into the config's `[[repo]]` list.
    pub index: usize,
    pub location: String,
    /// This machine's hosts for this repository (its `host` list).
    pub wanted: Vec<String>,
    /// Hosts seen the last time its snapshot list was read.
    pub hosts: Vec<String>,
    /// When the list was read; `None` if never.
    pub read: Option<Timestamp>,
    /// The cached hosts include this machine.
    pub mine: bool,
    /// A cached backup path holds the folder.
    pub holds: bool,
    /// Opening it failed.
    pub error: Option<String>,
}

/// The group level of one repository.
#[derive(Clone, Debug)]
pub struct Groups {
    /// Index of the repository in the config, when picked at the repo level.
    pub repo: Option<usize>,
    pub location: String,
    pub rows: Vec<GroupRow>,
    /// Index into [`Picker::visible`].
    pub sel: usize,
    pub filter: String,
    /// The filter being typed (`/`).
    pub input: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Repos,
    Groups,
}

/// What the user picked: a host and one of its backup paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub host: String,
    pub path: PathBuf,
    pub repo: Option<usize>,
}

impl Pick {
    /// Where the folder view opens after this pick, and the name to select
    /// there: `folder` when the picked path holds it, else the picked path.
    /// A backup path that is a file (`restic backup dir/a.log dir/b.csv`)
    /// opens its folder with the file selected, as `restoric FILE` does.
    /// Looks the path up in the newest snapshot that backed it up.
    pub fn open_at(
        &self,
        folder: &Path,
        snaps: &[SnapshotInfo],
        index: &Index,
    ) -> (PathBuf, Option<OsString>) {
        if folder.starts_with(&self.path) {
            return (folder.to_path_buf(), None);
        }
        let newest = snaps
            .iter()
            .filter(|s| s.host == self.host && s.paths.contains(&self.path))
            .max_by_key(|s| s.time);
        let file = newest.is_some_and(|s| match index.node_at(s, &self.path) {
            Ok(node) => node.is_some_and(|n| !n.is_dir()),
            Err(e) => {
                tracing::warn!("looking up {}: {e:#}", self.path.display());
                false
            }
        });
        match (file, self.path.parent(), self.path.file_name()) {
            (true, Some(parent), Some(name)) => (parent.to_path_buf(), Some(name.to_os_string())),
            _ => (self.path.clone(), None),
        }
    }
}

/// What the driver does after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Stay,
    /// Open config repository `i` and list its snapshots.
    Open(usize),
    /// List config repository `i`'s snapshots again.
    Refresh(usize),
    Picked(Pick),
    Quit,
}

#[derive(Debug)]
pub struct Picker {
    pub level: Level,
    pub repos: Vec<RepoRow>,
    pub repo_sel: usize,
    pub groups: Option<Groups>,
    /// The folder restoric started in.
    pub path: PathBuf,
    pub home: Option<PathBuf>,
    pub tz: TimeZone,
    pub now: Timestamp,
    /// Why the picker opened, or the last error; above the rows.
    pub message: Option<String>,
    /// What the driver is doing (`opening …`), in the status bar.
    pub busy: Option<String>,
    /// `g` waiting for `gg`.
    prefix_g: bool,
}

impl Picker {
    pub fn new(path: PathBuf, home: Option<PathBuf>, tz: TimeZone, now: Timestamp) -> Self {
        Picker {
            level: Level::Groups,
            repos: Vec::new(),
            repo_sel: 0,
            groups: None,
            path,
            home,
            tz,
            now,
            message: None,
            busy: None,
            prefix_g: false,
        }
    }

    /// The repo level: one row per candidate, from what the cache
    /// remembers. Repositories with this machine's snapshots first, then
    /// those holding the folder, then config order.
    pub fn with_repos(mut self, cands: &[Candidate], cache: &ProbeCache) -> Self {
        let mut rows: Vec<RepoRow> = cands
            .iter()
            .enumerate()
            .map(|(index, c)| {
                let seen = cache.seen(&c.repo);
                RepoRow {
                    index,
                    location: c.repo.clone(),
                    wanted: c.filter.hosts.clone(),
                    hosts: seen.as_ref().map(|s| s.hosts.clone()).unwrap_or_default(),
                    read: seen.as_ref().map(|s| s.checked),
                    mine: seen.as_ref().is_some_and(|s| s.mine(&c.filter)),
                    holds: seen.as_ref().is_some_and(|s| s.holds(&self.path)),
                    error: None,
                }
            })
            .collect();
        rows.sort_by_key(|r| (!r.mine, !r.holds));
        self.repos = rows;
        self.level = Level::Repos;
        self
    }

    /// The group level of a repository opened before the picker, with the
    /// dead-end message when there is one.
    pub fn with_groups(
        mut self,
        location: &str,
        snaps: &[SnapshotInfo],
        mine: &[String],
        message: Option<String>,
    ) -> Self {
        self.enter_groups(None, location, snaps, mine);
        self.message = message;
        self
    }

    fn enter_groups(
        &mut self,
        repo: Option<usize>,
        location: &str,
        snaps: &[SnapshotInfo],
        mine: &[String],
    ) {
        let rows = groups_of(snaps, mine);
        let sel = closest(&rows, &self.path);
        self.groups = Some(Groups {
            repo,
            location: location.to_string(),
            rows,
            sel,
            filter: String::new(),
            input: None,
        });
        self.level = Level::Groups;
        self.message = None;
    }

    /// Group rows that pass the filter, as indices into `rows`.
    pub fn visible(&self) -> Vec<usize> {
        let Some(g) = &self.groups else {
            return Vec::new();
        };
        let f = fold(&g.filter);
        g.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                f.is_empty()
                    || fold(&r.host).contains(&f)
                    || fold(&r.path.to_string_lossy()).contains(&f)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The selected repo row.
    pub fn repo(&self) -> Option<&RepoRow> {
        self.repos.get(self.repo_sel)
    }

    fn row_mut(&mut self, index: usize) -> Option<&mut RepoRow> {
        self.repos.iter_mut().find(|r| r.index == index)
    }

    /// The driver opened config repository `index`: its snapshots.
    pub fn opened(&mut self, index: usize, snaps: &[SnapshotInfo]) {
        self.refreshed(index, snaps);
        let Some(row) = self.repos.iter().find(|r| r.index == index) else {
            return;
        };
        let (location, mine) = (row.location.clone(), row.wanted.clone());
        self.enter_groups(Some(index), &location, snaps, &mine);
    }

    /// The driver read config repository `index`'s snapshot list again.
    pub fn refreshed(&mut self, index: usize, snaps: &[SnapshotInfo]) {
        let (now, path) = (self.now, self.path.clone());
        if let Some(row) = self.row_mut(index) {
            let mut hosts: Vec<String> = snaps.iter().map(|s| s.host.clone()).collect();
            hosts.sort();
            hosts.dedup();
            row.mine = snaps.iter().any(|s| row.wanted.contains(&s.host));
            row.holds = snaps
                .iter()
                .any(|s| crate::index::timeline::holds(&s.paths, &path));
            row.hosts = hosts;
            row.read = Some(now);
            row.error = None;
        }
        self.message = None;
    }

    /// Opening or listing config repository `index` failed.
    pub fn failed(&mut self, index: usize, err: &str) {
        let location = self
            .row_mut(index)
            .map(|r| {
                r.error = Some(err.to_string());
                r.location.clone()
            })
            .unwrap_or_default();
        self.message = Some(format!("Can't open {location}: {err}"));
    }

    /// Typing the group filter.
    fn filter_key(&mut self, k: KeyEvent) {
        let Some(g) = &mut self.groups else { return };
        let Some(text) = &mut g.input else { return };
        match k.code {
            KeyCode::Esc => {
                g.input = None;
                g.filter.clear();
                g.sel = 0;
            }
            KeyCode::Enter => g.input = None,
            KeyCode::Backspace => {
                if text.pop().is_none() {
                    g.input = None;
                }
                g.filter = g.input.clone().unwrap_or_default();
                g.sel = 0;
            }
            KeyCode::Char(c) => {
                text.push(c);
                g.filter = text.clone();
                g.sel = 0;
            }
            _ => {}
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Step {
        if k.kind == KeyEventKind::Release {
            return Step::Stay;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return Step::Quit;
        }
        if self.groups.as_ref().is_some_and(|g| g.input.is_some()) {
            self.filter_key(k);
            return Step::Stay;
        }
        let g = std::mem::take(&mut self.prefix_g);
        let (len, sel) = match self.level {
            Level::Repos => (self.repos.len(), self.repo_sel),
            Level::Groups => (
                self.visible().len(),
                self.groups.as_ref().map_or(0, |g| g.sel),
            ),
        };
        let last = len.saturating_sub(1);
        let set = |p: &mut Picker, i: usize| match p.level {
            Level::Repos => p.repo_sel = i.min(last),
            Level::Groups => {
                if let Some(g) = &mut p.groups {
                    g.sel = i.min(last);
                }
            }
        };
        match (g, k.code) {
            (true, KeyCode::Char('g')) => set(self, 0),
            (true, _) => {}
            (_, KeyCode::Char('g')) => self.prefix_g = true,
            (_, KeyCode::Char('j') | KeyCode::Down) => set(self, sel + 1),
            (_, KeyCode::Char('k') | KeyCode::Up) => set(self, sel.saturating_sub(1)),
            (_, KeyCode::Char('G') | KeyCode::End) => set(self, last),
            (_, KeyCode::Home) => set(self, 0),
            (_, KeyCode::PageDown) => set(self, sel + 10),
            (_, KeyCode::PageUp) => set(self, sel.saturating_sub(10)),
            (_, KeyCode::Char('q')) if self.level == Level::Groups && !self.repos.is_empty() => {
                self.level = Level::Repos;
                self.message = None;
            }
            (_, KeyCode::Char('q')) => return Step::Quit,
            (_, KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right) => match self.level {
                Level::Repos => {
                    if let Some(r) = self.repo() {
                        return Step::Open(r.index);
                    }
                }
                Level::Groups => {
                    if let Some(g) = &self.groups
                        && let Some(&i) = self.visible().get(g.sel)
                    {
                        let r = &g.rows[i];
                        return Step::Picked(Pick {
                            host: r.host.clone(),
                            path: r.path.clone(),
                            repo: g.repo,
                        });
                    }
                }
            },
            (_, KeyCode::Char('r')) if self.level == Level::Repos => {
                if let Some(r) = self.repo() {
                    return Step::Refresh(r.index);
                }
            }
            (_, KeyCode::Char('/')) if self.level == Level::Groups => {
                if let Some(g) = &mut self.groups {
                    g.input = Some(g.filter.clone());
                }
            }
            (_, KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace)
                if self.level == Level::Groups =>
            {
                let filtered = self.groups.as_ref().is_some_and(|g| !g.filter.is_empty());
                if filtered {
                    if let Some(g) = &mut self.groups {
                        g.filter.clear();
                        g.sel = 0;
                    }
                } else if !self.repos.is_empty() {
                    self.level = Level::Repos;
                    self.message = None;
                }
            }
            _ => {}
        }
        Step::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::timeline::Filter;
    use crate::repo::{SnapshotId, TreeId};

    fn snap(host: &str, when: &str, paths: &[&str]) -> SnapshotInfo {
        SnapshotInfo {
            id: SnapshotId::default(),
            time: format!("{when}T12:00:00Z").parse().unwrap(),
            host: host.into(),
            paths: paths.iter().map(PathBuf::from).collect(),
            tags: vec![],
            tree: TreeId::default(),
        }
    }

    fn snaps() -> Vec<SnapshotInfo> {
        vec![
            snap("old", "2026-03-01", &["/home/me"]),
            snap("vm", "2026-08-01", &["/srv/data", "/etc"]),
            snap("vm", "2026-09-01", &["/srv/data"]),
            snap("me", "2026-07-14", &["/home/me/dev/project"]),
            snap("me", "2026-09-06", &["/home/me/dev/project"]),
        ]
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn picker() -> Picker {
        Picker::new(
            PathBuf::from("/home/me/dev/project/src"),
            Some(PathBuf::from("/home/me")),
            TimeZone::UTC,
            "2026-10-05T12:00:00Z".parse().unwrap(),
        )
    }

    #[test]
    fn groups_this_machine_first_then_newest_and_counts_each_path() {
        let rows = groups_of(&snaps(), &["me".into()]);
        let short: Vec<(&str, &str, usize)> = rows
            .iter()
            .map(|r| (r.host.as_str(), r.path.to_str().unwrap(), r.snapshots))
            .collect();
        assert_eq!(
            short,
            [
                ("me", "/home/me/dev/project", 2),
                ("vm", "/srv/data", 2),
                ("vm", "/etc", 1),
                ("old", "/home/me", 1),
            ]
        );
        assert!(rows[0].mine && !rows[1].mine);
    }

    #[test]
    fn the_closest_row_is_preselected() {
        let rows = groups_of(&snaps(), &[]);
        // Newest first without `mine`: me, vm /srv/data, vm /etc, old.
        assert_eq!(closest(&rows, Path::new("/home/me/dev/project/src")), 0);
        assert_eq!(closest(&rows, Path::new("/home/me/music")), 3);
        // Two rows below it: the earlier one.
        assert_eq!(closest(&rows, Path::new("/home")), 0);
        assert_eq!(closest(&rows, Path::new("/nowhere")), 0);
    }

    #[test]
    fn keys_move_filter_and_pick() {
        let mut p = picker().with_groups("repo", &snaps(), &["me".into()], None);
        assert_eq!(p.groups.as_ref().unwrap().sel, 0);
        p.key(key('j'));
        p.key(key('/'));
        for c in "vm".chars() {
            p.key(key(c));
        }
        assert_eq!(p.visible(), [1, 2]);
        p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(p.groups.as_ref().unwrap().input.is_none());
        p.key(key('G'));
        let step = p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            step,
            Step::Picked(Pick {
                host: "vm".into(),
                path: PathBuf::from("/etc"),
                repo: None
            })
        );
        // esc clears the filter; a second does nothing without a repo level.
        p.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(p.visible().len(), 4);
        p.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(p.level, Level::Groups);
        assert_eq!(p.key(key('q')), Step::Quit);
    }

    #[test]
    fn repo_level_orders_opens_and_comes_back() {
        let cand = |r: &str, h: &str| Candidate {
            repo: r.into(),
            filter: Filter {
                hosts: vec![h.into()],
                tag: None,
            },
        };
        let cands = [cand("a", "me"), cand("b", "me"), cand("c", "me")];
        let mut cache = ProbeCache::default();
        let at = |s| Timestamp::from_second(s).unwrap();
        cache.record("a", &[snap("vm", "2026-01-01", &["/srv"])], at(0));
        cache.record("c", &[snap("me", "2026-01-01", &["/home/me"])], at(0));
        let mut p = picker().with_repos(&cands, &cache);
        let order: Vec<&str> = p.repos.iter().map(|r| r.location.as_str()).collect();
        assert_eq!(order, ["c", "a", "b"]);
        assert_eq!(p.repos[2].read, None);
        assert_eq!(p.key(key('j')), Step::Stay);
        assert_eq!(p.key(key('r')), Step::Refresh(0));
        p.refreshed(0, &snaps());
        assert_eq!(p.repos[1].hosts, ["me", "old", "vm"]);
        assert!(p.repos[1].mine && p.repos[1].holds);
        assert_eq!(p.key(key('j')), Step::Stay);
        p.failed(1, "no such host");
        assert_eq!(p.message.as_deref(), Some("Can't open b: no such host"));
        assert_eq!(
            p.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Step::Open(1)
        );
        p.opened(1, &snaps());
        assert_eq!(p.level, Level::Groups);
        assert_eq!(p.message, None);
        assert_eq!(p.groups.as_ref().unwrap().repo, Some(1));
        p.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(p.level, Level::Repos);
        p.opened(1, &snaps());
        assert_eq!(p.key(key('q')), Step::Stay);
        assert_eq!(p.level, Level::Repos);
        assert_eq!(p.key(key('q')), Step::Quit);
    }
}
