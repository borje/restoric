//! Marks, visual mode, the yank register, restoring and copying
//! (PLAN.md §3.5, §3.11).

use std::path::PathBuf;

use super::{App, Effect, Row, View};
use crate::index::versions::Run;
use crate::repo::NodeKind;
use crate::restore::{How, Target, planned};
use crate::ui::fmt;
use crate::worker::Request;

/// The restore dialog (`r`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    pub target: Target,
    /// 0 to 3: overwrite, next to it, restore folder, pager or tar.
    pub sel: usize,
    /// Overwriting asks for ⏎ twice.
    pub confirm: bool,
}

/// The confirmation popup before `P` overwrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    pub text: String,
    pub targets: Vec<Target>,
}

/// `main.go`, `src/`, or `2 items`.
pub fn describe(targets: &[Target]) -> String {
    match targets {
        [t] => {
            let mut n = t.name();
            if t.node.kind == NodeKind::Dir {
                n.push('/');
            }
            n
        }
        _ => format!("{} items", targets.len()),
    }
}

impl App {
    /// Whether listing row `k` is selected (marked or in the visual range).
    pub fn is_marked(&self, k: usize, row: Row) -> bool {
        if row == Row::Up {
            return false;
        }
        if let Some(anchor) = self.visual {
            let (lo, hi) = (anchor.min(self.sel), anchor.max(self.sel));
            if (lo..=hi).contains(&k) {
                return true;
            }
        }
        self.entry(row)
            .is_some_and(|e| self.marks.contains(&e.node.name))
    }

    /// Selected rows in the folder view, by index.
    pub fn marked_rows(&self) -> Vec<usize> {
        self.rows()
            .iter()
            .enumerate()
            .filter(|(k, r)| self.is_marked(*k, **r))
            .map(|(k, _)| k)
            .collect()
    }

    pub(super) fn toggle_mark(&mut self) {
        if let Some(e) = self.selected() {
            let name = e.node.name.clone();
            if !self.marks.remove(&name) {
                self.marks.insert(name);
            }
        }
        self.act(super::Action::Down(1));
    }

    pub(super) fn toggle_visual(&mut self) {
        match self.visual.take() {
            None => self.visual = Some(self.sel),
            // Keep the range selected.
            Some(anchor) => {
                let rows = self.rows();
                let (lo, hi) = (anchor.min(self.sel), anchor.max(self.sel));
                for r in rows.iter().take(hi + 1).skip(lo) {
                    if let Some(e) = self.entry(*r) {
                        self.marks.insert(e.node.name.clone());
                    }
                }
            }
        }
    }

    /// The target for a listing row.
    fn row_target(&self, row: Row) -> Option<Target> {
        let e = self.entry(row)?;
        let set = self.set();
        Some(Target {
            snapshot: set.get(self.entry_snapshot(e))?.clone(),
            path: self.folder.join(&e.node.name),
            node: e.node.clone(),
        })
    }

    /// The selected version in the versions or diff view.
    fn version_target(&self, path: &PathBuf, run: usize) -> Result<Target, String> {
        let t = self.tracks.get(path).ok_or_else(String::new)?;
        let runs: Vec<Run> = t.runs.iter().rev().copied().collect();
        let r = runs.get(run).ok_or_else(String::new)?;
        if !r.exists {
            return Err(
                "The file did not exist in these snapshots, so there is nothing to restore.".into(),
            );
        }
        let node = self
            .nodes
            .get(&(path.clone(), r.from))
            .cloned()
            .flatten()
            .ok_or_else(String::new)?;
        Ok(Target {
            snapshot: t.set[r.from].clone(),
            path: path.clone(),
            node,
        })
    }

    /// What `y`, `r` and `cc` act on: the selection, or what's under the cursor.
    pub fn targets(&self) -> Result<Vec<Target>, String> {
        match &self.view {
            View::Folder => {
                let rows = self.rows();
                let marked = self.marked_rows();
                let picked: Vec<Row> = if marked.is_empty() {
                    rows.get(self.sel).copied().into_iter().collect()
                } else {
                    marked.iter().map(|&k| rows[k]).collect()
                };
                let t: Vec<Target> = picked
                    .into_iter()
                    .filter_map(|r| self.row_target(r))
                    .collect();
                if t.is_empty() {
                    Err("Select a file or folder first.".into())
                } else {
                    Ok(t)
                }
            }
            View::Versions(v) => self.version_target(&v.path, v.sel).map(|t| vec![t]),
            View::Diff(d) => self.version_target(&d.path, d.run).map(|t| vec![t]),
        }
    }

    pub(super) fn yank(&mut self) -> bool {
        match self.targets() {
            Ok(t) => {
                let when = fmt::time(t[0].snapshot.time, &self.tz);
                self.message = Some(format!(
                    "Yanked {} from {when}. p restores next to the original, P overwrites.",
                    describe(&t)
                ));
                self.yanked = Some(t);
                self.marks.clear();
                self.visual = None;
                true
            }
            Err(e) => {
                let e = if e.is_empty() {
                    "Nothing to yank here.".into()
                } else {
                    e
                };
                self.message = Some(e);
                false
            }
        }
    }

    pub(super) fn paste(&mut self, over: bool) -> bool {
        let Some(targets) = self.yanked.clone() else {
            self.message = Some("Nothing yanked. Press y on a file first.".into());
            return false;
        };
        if over {
            let when = fmt::time(targets[0].snapshot.time, &self.tz);
            self.confirm = Some(Confirm {
                text: format!(
                    "Replace {} on disk with the version from {when}? The current version is kept for :undo.",
                    describe(&targets)
                ),
                targets,
            });
        } else {
            self.outbox.push(Request::Restore {
                targets,
                how: How::NextTo,
            });
        }
        true
    }

    pub(super) fn open_dialog(&mut self) -> bool {
        match self.targets() {
            Ok(t) => {
                let target = t[0].clone();
                let on_disk = self.exists.get(&target.path).copied();
                if on_disk.is_none() {
                    self.outbox.push(Request::Exists {
                        paths: vec![target.path.clone()],
                    });
                }
                // Next to it, unless there's nothing on disk to be next to.
                let sel = if on_disk == Some(false) { 0 } else { 1 };
                self.dialog = Some(Dialog {
                    target,
                    sel,
                    confirm: false,
                });
                true
            }
            Err(e) => {
                if !e.is_empty() {
                    self.message = Some(e);
                }
                false
            }
        }
    }

    /// Whether the dialog's target exists on disk (assumed while unknown).
    pub fn on_disk(&self, t: &Target) -> bool {
        self.exists.get(&t.path).copied().unwrap_or(true)
    }

    /// The dialog's four options: label and where it goes.
    pub fn dialog_options(&self, d: &Dialog) -> [(String, String); 4] {
        let t = &d.target;
        let home = self.home.as_deref();
        let is_dir = t.node.kind == NodeKind::Dir;
        let slash = if is_dir { "/" } else { "" };
        let show = |p: &std::path::Path| fmt::path(p, home);
        let file_name = |p: PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let first = if self.on_disk(t) {
            ("Overwrite original".to_string(), show(&t.path))
        } else {
            (
                "Restore to original location".to_string(),
                format!("{} (missing now)", show(&t.path)),
            )
        };
        let next = if self.on_disk(t) {
            format!(
                "→ {}{slash}",
                file_name(planned(t, How::NextTo, &self.places))
            )
        } else {
            format!("→ {}{slash}", show(&t.path))
        };
        let dir = planned(t, How::RestoreDir, &self.places);
        let fourth = if is_dir {
            (
                "Write a tar archive".to_string(),
                format!("→ {}", file_name(planned(t, How::Tar, &self.places))),
            )
        } else {
            (
                "Show in $PAGER".to_string(),
                "read only, writes nothing".to_string(),
            )
        };
        [
            first,
            ("Restore next to it".to_string(), next),
            (
                format!("Restore to {}/", show(&self.places.restore_dir)),
                format!("→ {}{slash}", show(&dir)),
            ),
            fourth,
        ]
    }

    /// ⏎ in the dialog.
    pub(super) fn dialog_enter(&mut self) {
        let Some(d) = self.dialog.clone() else { return };
        let t = d.target.clone();
        match d.sel {
            0 if self.on_disk(&t) && !d.confirm => {
                self.dialog = Some(Dialog { confirm: true, ..d });
                return;
            }
            0 => self.outbox.push(Request::Restore {
                targets: vec![t],
                how: How::Overwrite,
            }),
            1 => self.outbox.push(Request::Restore {
                targets: vec![t],
                how: How::NextTo,
            }),
            2 => self.outbox.push(Request::Restore {
                targets: vec![t],
                how: How::RestoreDir,
            }),
            _ if t.node.kind == NodeKind::Dir => self.outbox.push(Request::Restore {
                targets: vec![t],
                how: How::Tar,
            }),
            _ => {
                let name = t.name();
                self.outbox.push(Request::ReadAll { node: t.node, name });
            }
        }
        self.dialog = None;
    }

    /// `cc` `cd` `cf`: snapshot:path, the folder, the name.
    pub(super) fn copy(&mut self, what: char) -> bool {
        let t = match self.targets() {
            Ok(t) => t[0].clone(),
            Err(_) => {
                self.message = Some("Select a file or folder first.".into());
                return false;
            }
        };
        let text = match what {
            'c' => format!("{}:{}", t.snapshot.id.0.short(), t.path.display()),
            'd' => match &self.view {
                View::Folder => self.folder.display().to_string(),
                _ => t
                    .path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            },
            _ => t.name(),
        };
        self.message = Some(format!("Copied {text}"));
        self.effects.push(Effect::Clipboard(text));
        true
    }

    /// After a restore or undo, what's on disk has changed.
    pub(super) fn disk_changed(&mut self) {
        self.live.clear();
        self.disk_files.clear();
        self.disk_stats.borrow_mut().clear();
        self.exists.clear();
        self.diffs.retain(|k, _| {
            !matches!(k.old, crate::diff::SideKey::Disk(_))
                && !matches!(k.new, crate::diff::SideKey::Disk(_))
        });
        self.pending
            .retain(|p| !matches!(p, super::Pending::Live(_) | super::Pending::Disk(_)));
        self.ensure();
    }
}
