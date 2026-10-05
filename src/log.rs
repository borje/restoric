//! `restoric log PATH`: the change points of a path, printed (no TUI).

use std::path::Path;

use anyhow::{Result, bail};
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::index::folder::Counts;
use crate::index::timeline::{ChangeKind, Filter, explain_empty, timeline_set};
use crate::index::{Index, NodeRef};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub snapshot: String,
    pub time: String,
    pub kind: ChangeKind,
    /// Items added, changed and deleted (folders only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Counts>,
    /// Snapshots after this one, before the next change, where nothing changed.
    pub unchanged: usize,
}

pub struct Log {
    pub entries: Vec<Entry>,
    /// Snapshots in the timeline set.
    pub snapshots: usize,
    pub is_dir: bool,
}

/// Change points of `path`, newest first, with counts.
pub fn log(
    index: &Index,
    filter: &Filter,
    path: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Log> {
    let snaps = index.repo().snapshots()?;
    let set = timeline_set(&snaps, filter, path);
    if set.is_empty() {
        bail!("{}", explain_empty(&snaps, filter, path));
    }
    let refs = index.refs(&set, path, progress)?;
    let points = index.change_points(&refs)?;
    let is_dir = refs
        .iter()
        .rev()
        .find(|r| r.exists())
        .is_some_and(|r| matches!(r, NodeRef::Dir(_)));

    let mut entries = Vec::new();
    for (k, p) in points.iter().enumerate() {
        let prev = p
            .index
            .checked_sub(1)
            .map(|i| refs[i])
            .unwrap_or(NodeRef::Missing);
        let cur = refs[p.index];
        let counts = if matches!(prev, NodeRef::Dir(_)) || matches!(cur, NodeRef::Dir(_)) {
            Some(index.ref_counts(&prev, &cur)?)
        } else {
            None
        };
        let next = points.get(k + 1).map(|n| n.index).unwrap_or(set.len());
        // After a deletion the path is missing, not unchanged.
        let unchanged = if p.kind == ChangeKind::Deleted {
            0
        } else {
            next - p.index - 1
        };
        let s = &set[p.index];
        entries.push(Entry {
            snapshot: s.id.to_string(),
            time: s.time.to_string(),
            kind: p.kind,
            counts,
            unchanged,
        });
    }
    index.flush()?;
    entries.reverse();
    if !refs.iter().any(|r| r.exists()) {
        bail!(
            "{} isn't in any of the {} snapshots that cover it.",
            path.display(),
            set.len()
        );
    }
    Ok(Log {
        entries,
        snapshots: set.len(),
        is_dir,
    })
}

/// The log as text, times in `tz`.
pub fn render(log: &Log, path: &Path, tz: &TimeZone) -> String {
    let versions = log
        .entries
        .iter()
        .filter(|e| e.kind != ChangeKind::Deleted)
        .count();
    let mut out = format!(
        "{}{} · {} version{} in {} snapshot{}\n\n",
        path.display(),
        if log.is_dir { "/" } else { "" },
        versions,
        if versions == 1 { "" } else { "s" },
        log.snapshots,
        if log.snapshots == 1 { "" } else { "s" },
    );
    for e in &log.entries {
        let time = e
            .time
            .parse::<jiff::Timestamp>()
            .map(|t| {
                t.to_zoned(tz.clone())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|_| e.time.clone());
        if e.unchanged > 0 {
            out.push_str(&format!(
                "                  ┄ {} unchanged ┄\n",
                e.unchanged
            ));
        }
        let what = match (e.kind, e.counts) {
            (ChangeKind::Added, Some(c)) => format!("new  {}", c.label(" ")),
            (ChangeKind::Deleted, Some(c)) => format!("deleted  {}", c.label(" ")),
            (ChangeKind::Changed, Some(c)) => c.label(" "),
            (ChangeKind::Added, None) => "new".to_string(),
            (ChangeKind::Deleted, None) => "deleted".to_string(),
            (ChangeKind::Changed, None) => "changed".to_string(),
        };
        out.push_str(&format!("{time}  {}  {what}\n", &e.snapshot[..8]));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::cache::Cache;
    use crate::index::Mode;
    use crate::repo::fake::FakeRepo;

    const HISTORY: &str = "
host laptop
root /home/me/proj
snapshot 2026-07-24 12:16
  write src/main.go v1\\n
  write src/util.go u
  write src/legacy.go old
  write README.md hi
snapshot 2026-07-25 12:00
  touch src/util.go                # metadata only
snapshot 2026-07-26 12:00
  append src/main.go v2\\n
  rm src/legacy.go
  write src/server.go s
snapshot 2026-07-27 12:00
  write README.md hello            # outside src
snapshot 2026-07-28 12:00
  chmod src/util.go 755            # permissions count
snapshot 2026-07-29 12:00
  rm src
snapshot 2026-07-30 12:00
  write src/main.go back
snapshot 2026-07-31 12:00 host=other
  write src/main.go other machine
";

    fn run(mode: Mode, path: &str) -> String {
        let repo = Arc::new(FakeRepo::parse(HISTORY).unwrap());
        let index = Index::new(repo, Cache::in_memory(), mode, 1 << 20);
        let filter = Filter {
            hosts: vec!["laptop".into()],
            tag: None,
        };
        let path = Path::new(path);
        let log = log(&index, &filter, path, &mut |_, _| {}).unwrap();
        render(&log, path, &TimeZone::UTC)
    }

    #[test]
    fn folder_log() {
        insta::assert_snapshot!(run(Mode::Content, "/home/me/proj/src"), @"
        /home/me/proj/src/ · 4 versions in 7 snapshots

        2026-07-30 12:00  c162e2d7  new  +1
        2026-07-29 12:00  e26edccf  deleted  −3
        2026-07-28 12:00  1a66857e  ~1
                          ┄ 1 unchanged ┄
        2026-07-26 12:00  bd65d7c5  +1 ~1 −1
                          ┄ 1 unchanged ┄
        2026-07-24 12:16  fe3121f1  new  +3
        ");
    }

    #[test]
    fn strict_shows_metadata_only_changes() {
        insta::assert_snapshot!(run(Mode::Strict, "/home/me/proj/src"), @"
        /home/me/proj/src/ · 5 versions in 7 snapshots

        2026-07-30 12:00  c162e2d7  new  +1
        2026-07-29 12:00  e26edccf  deleted  −3
        2026-07-28 12:00  1a66857e  ~1
                          ┄ 1 unchanged ┄
        2026-07-26 12:00  bd65d7c5  +1 ~1 −1
        2026-07-25 12:00  10f6460d  ~1
        2026-07-24 12:16  fe3121f1  new  +3
        ");
    }

    #[test]
    fn file_log() {
        insta::assert_snapshot!(run(Mode::Content, "/home/me/proj/src/util.go"), @"
        /home/me/proj/src/util.go · 2 versions in 7 snapshots

        2026-07-29 12:00  e26edccf  deleted
        2026-07-28 12:00  1a66857e  changed
                          ┄ 3 unchanged ┄
        2026-07-24 12:16  fe3121f1  new
        ");
    }
}
