//! The timeline set and change points (PLAN.md §2.3, §2.4).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::{Index, NodeRef};
use crate::repo::SnapshotInfo;

/// Which snapshots belong to this machine (§2.4).
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// Accepted hostnames; several when the machine was renamed.
    pub hosts: Vec<String>,
    /// Only snapshots with this tag.
    pub tag: Option<String>,
}

impl Filter {
    pub fn matches(&self, s: &SnapshotInfo) -> bool {
        self.hosts.contains(&s.host) && self.tag.as_ref().is_none_or(|t| s.tags.contains(t))
    }
}

/// This machine's snapshots whose backup paths include `path`, oldest first.
pub fn timeline_set(snaps: &[SnapshotInfo], filter: &Filter, path: &Path) -> Vec<SnapshotInfo> {
    let mut set: Vec<SnapshotInfo> = snaps
        .iter()
        .filter(|s| filter.matches(s) && s.paths.iter().any(|p| path.starts_with(p)))
        .cloned()
        .collect();
    set.sort_by_key(|s| s.time);
    set
}

/// Why the timeline set is empty, in words for the user (§4.6 steps 4–5).
pub fn explain_empty(snaps: &[SnapshotInfo], filter: &Filter, path: &Path) -> String {
    let mine: Vec<&SnapshotInfo> = snaps.iter().filter(|s| filter.matches(s)).collect();
    if mine.is_empty() {
        let mut hosts: Vec<&str> = snaps.iter().map(|s| s.host.as_str()).collect();
        hosts.sort();
        hosts.dedup();
        let wanted = filter.hosts.join(", ");
        let tag = filter
            .tag
            .as_ref()
            .map(|t| format!(" with tag {t}"))
            .unwrap_or_default();
        if hosts.is_empty() {
            return "The repository has no snapshots.".to_string();
        }
        return format!(
            "No snapshots from this machine ({wanted}){tag}.\n\
             Hosts with snapshots: {}.\n\
             If one of them is this machine, pass --host NAME.",
            hosts.join(", ")
        );
    }
    let mut paths: Vec<&PathBuf> = mine.iter().flat_map(|s| &s.paths).collect();
    paths.sort();
    paths.dedup();
    let list: Vec<String> = paths.iter().map(|p| format!("  {}", p.display())).collect();
    format!(
        "{} isn't in any of this machine's snapshots.\nThis machine backs up:\n{}",
        path.display(),
        list.join("\n")
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// The path appears (the first time, or again after being deleted).
    Added,
    Changed,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangePoint {
    /// Index into the timeline set.
    pub index: usize,
    pub kind: ChangeKind,
}

impl Index {
    /// What's at `path` in every snapshot of `set`, newest first, reporting
    /// progress as `(done, total)`.
    pub fn refs(
        &self,
        set: &[SnapshotInfo],
        path: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Vec<NodeRef>> {
        let mut refs = vec![NodeRef::Missing; set.len()];
        for (done, i) in (0..set.len()).rev().enumerate() {
            refs[i] = self.node_ref(&set[i], path)?;
            progress(done + 1, set.len());
        }
        Ok(refs)
    }

    /// Where the path changed from the snapshot before, given its `refs`
    /// (oldest first). The first snapshot that has the path counts as added.
    pub fn change_points(&self, refs: &[NodeRef]) -> Result<Vec<ChangePoint>> {
        let mut out = Vec::new();
        let mut prev = NodeRef::Missing;
        for (index, r) in refs.iter().enumerate() {
            let kind = match (prev.exists(), r.exists()) {
                (false, true) => Some(ChangeKind::Added),
                (true, false) => Some(ChangeKind::Deleted),
                (true, true) if self.refs_differ(&prev, r)? => Some(ChangeKind::Changed),
                _ => None,
            };
            if let Some(kind) = kind {
                out.push(ChangePoint { index, kind });
            }
            prev = *r;
        }
        Ok(out)
    }
}
