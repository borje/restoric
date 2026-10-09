//! Marks, visual mode, the yank register, restoring and copying
//! (PLAN.md §3.5, §3.11).

use std::path::PathBuf;

use super::cmdline::InputKind;
use super::{App, Effect, Row, View};
use crate::index::listing::Entry;
use crate::index::versions::Run;
use crate::repo::NodeKind;
use crate::repo::RestoreStep;
use crate::restore::{How, Progress, Target, planned};
use crate::ui::fmt;
use crate::worker::{Cancel, Request};

/// What the restore keys say on an entry that exists on disk, viewing the
/// `on disk` version: it isn't a version to restore.
const ON_DISK: &str = "That's the file on disk. Pick a version to restore.";

/// The restore dialog (`r`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    pub target: Target,
    /// 0 to 2: overwrite, next to it, restore folder; 3: tar, for a folder.
    pub sel: usize,
    /// Overwriting asks for ⏎ twice.
    pub confirm: bool,
}

impl Dialog {
    /// How many options: a folder has the tar archive as a fourth.
    pub fn options(&self) -> usize {
        if self.target.node.kind == NodeKind::Dir {
            4
        } else {
            3
        }
    }
}

/// A y/n popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// Before `P` overwrites.
    Overwrite { text: String, targets: Vec<Target> },
    /// `esc` while restoring.
    StopRestore,
    /// `q` while restoring.
    QuitRestore,
}

/// The restore that's running; there's at most one.
#[derive(Clone, Debug)]
pub struct Restoring {
    pub how: How,
    pub items: usize,
    /// `main.go`, `src/` or `2 items`, until the worker names the item.
    pub label: String,
    pub cancel: Cancel,
    /// The latest progress; `None` until the worker starts.
    pub progress: Option<Progress>,
    /// Asked to stop; waits for the item being copied.
    pub stopping: bool,
    /// Quit once it has stopped.
    pub quit_after: bool,
}

impl Restoring {
    /// The item being restored: `main.go`, `src/`.
    pub fn name(&self) -> String {
        self.progress
            .as_ref()
            .map_or_else(|| self.label.clone(), |p| p.name.clone())
    }

    /// How far the current item has got, 0 to 100, once it's copying.
    pub fn percent(&self) -> Option<u64> {
        match self.progress.as_ref()?.step {
            RestoreStep::Bytes { done, total } if total > 0 => {
                Some((done.min(total) * 100) / total)
            }
            RestoreStep::Bytes { .. } => Some(100),
            RestoreStep::Preparing => None,
        }
    }

    /// The popup's text for `StopRestore` and `QuitRestore`, below its title.
    pub fn stop_text(&self, quit: bool) -> String {
        let name = self.name();
        let state = match (self.percent(), self.items) {
            (Some(p), 1) => format!("{name} is {p}% done."),
            (Some(p), n) => format!(
                "Item {} of {n}, {name}, is {p}% done.",
                self.progress.as_ref().map_or(1, |p| p.item + 1)
            ),
            (None, _) => format!("{name} is being prepared."),
        };
        let what = match (&self.how, self.items) {
            (How::Overwrite, _) => "Nothing will be overwritten.",
            (_, 1) => "The partly restored copy is removed.",
            _ => "Finished items are kept; the partly restored copy is removed.",
        };
        if quit {
            format!("A restore is running. {state} {what}")
        } else {
            format!("{state} {what}")
        }
    }
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

    /// The picked listing rows: the selection, or what's under the cursor.
    fn picked_rows(&self) -> Vec<Row> {
        let rows = self.rows();
        let marked = self.marked_rows();
        if marked.is_empty() {
            rows.get(self.sel).copied().into_iter().collect()
        } else {
            marked.iter().map(|&k| rows[k]).collect()
        }
    }

    /// On the `on disk` version, the picked entry that exists on disk when
    /// it's the only one: the file itself, not a version to restore.
    fn disk_pick(&self) -> Option<(PathBuf, Entry)> {
        if !self.on_disk() || self.view != View::Folder {
            return None;
        }
        let [row] = self.picked_rows()[..] else {
            return None;
        };
        let e = self.entry(row).filter(|e| !e.is_gone())?;
        Some((self.folder.join(&e.node.name), e.clone()))
    }

    /// The selected version in the versions or diff view.
    fn version_target(&self, path: &PathBuf, run: usize, disk: bool) -> Result<Target, String> {
        if disk {
            return Err(ON_DISK.into());
        }
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
                let picked = self.picked_rows();
                // On disk, an entry that's there is the file itself; one
                // that's gone restores from the last snapshot that had it.
                if self.on_disk()
                    && picked
                        .iter()
                        .any(|&r| self.entry(r).is_some_and(|e| !e.is_gone()))
                {
                    return Err(ON_DISK.into());
                }
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
            View::Versions(v) => self.version_target(&v.path, v.sel, v.disk).map(|t| vec![t]),
            View::Diff(d) => self.version_target(&d.path, d.run, d.disk).map(|t| vec![t]),
            View::Find(_) => Err(String::new()),
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
        if self.restore_busy() {
            return false;
        }
        let Some(targets) = self.yanked.clone() else {
            self.message = Some("Nothing yanked. Press y on a file first.".into());
            return false;
        };
        if self.foreign() {
            if over {
                self.message = Some(
                    "Overwriting is off for another host's snapshots. p restores into a directory you choose."
                        .into(),
                );
                return false;
            }
            self.ask_dir(targets);
            return true;
        }
        if over {
            let when = fmt::time(targets[0].snapshot.time, &self.tz);
            self.confirm = Some(Confirm::Overwrite {
                text: format!(
                    "Replace {} on disk with the version from {when}? The current version is kept for :undo.",
                    describe(&targets)
                ),
                targets,
            });
        } else {
            self.start_restore(targets, How::NextTo);
        }
        true
    }

    /// Sends a restore to the worker and shows its progress.
    pub(super) fn start_restore(&mut self, targets: Vec<Target>, how: How) {
        let cancel = Cancel::default();
        self.restoring = Some(Restoring {
            how: how.clone(),
            items: targets.len(),
            label: describe(&targets),
            cancel: cancel.clone(),
            progress: None,
            stopping: false,
            quit_after: false,
        });
        self.outbox.push(Request::Restore {
            targets,
            how,
            cancel,
        });
    }

    /// Whether a restore is running; if so, says so. Another restore or an
    /// undo waits until it's done.
    pub(super) fn restore_busy(&mut self) -> bool {
        if self.restoring.is_some() {
            self.message = Some("A restore is running · esc to stop".into());
        }
        self.restoring.is_some()
    }

    /// The restore is over: closes its popups, and quits if asked to.
    pub(super) fn restore_ended(&mut self) -> Option<Restoring> {
        if matches!(
            self.confirm,
            Some(Confirm::StopRestore | Confirm::QuitRestore)
        ) {
            self.confirm = None;
        }
        let r = self.restoring.take();
        if r.as_ref().is_some_and(|r| r.quit_after) {
            self.quit = true;
        }
        r
    }

    /// Stops the running restore (`:cancel`, or `y` in the popup).
    pub(super) fn stop_restore(&mut self, quit: bool) {
        match &mut self.restoring {
            Some(r) => {
                r.cancel.cancel();
                r.stopping = true;
                r.quit_after |= quit;
            }
            None if quit => self.quit = true,
            None => self.message = Some("No restore is running.".into()),
        }
    }

    /// On a foreign host (§3.18): asks where the restore goes, with the
    /// last directory used, else the folder restoric started in.
    pub(super) fn ask_dir(&mut self, targets: Vec<Target>) {
        let dir = self
            .last_dir
            .clone()
            .unwrap_or_else(|| self.start_dir.clone());
        let text = fmt::path(&dir, self.home.as_deref());
        self.pending_targets = Some(targets);
        self.start_input(InputKind::Dir, &text);
    }

    /// `⏎` on the `restore to:` prompt.
    pub(super) fn restore_into(&mut self, text: &str) {
        let Some(targets) = self.pending_targets.take() else {
            return;
        };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let dir = fmt::expand_home(text, self.home.as_deref());
        let dir = if dir.is_absolute() {
            dir
        } else {
            self.start_dir.join(dir)
        };
        self.last_dir = Some(dir.clone());
        self.start_restore(targets, How::Into(dir));
    }

    pub(super) fn open_dialog(&mut self) -> bool {
        if self.restore_busy() {
            return false;
        }
        match self.targets() {
            Ok(t) if self.foreign() => {
                self.ask_dir(t);
                true
            }
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
    pub fn exists_on_disk(&self, t: &Target) -> bool {
        self.exists.get(&t.path).copied().unwrap_or(true)
    }

    /// The dialog's options: label and where it goes.
    pub fn dialog_options(&self, d: &Dialog) -> Vec<(String, String)> {
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
        let first = if self.exists_on_disk(t) {
            ("Overwrite original".to_string(), show(&t.path))
        } else {
            (
                "Restore to original location".to_string(),
                format!("{} (missing now)", show(&t.path)),
            )
        };
        let next = if self.exists_on_disk(t) {
            format!(
                "→ {}{slash}",
                file_name(planned(t, &How::NextTo, &self.places))
            )
        } else {
            format!("→ {}{slash}", show(&t.path))
        };
        let dir = planned(t, &How::RestoreDir, &self.places);
        let mut options = vec![
            first,
            ("Restore next to it".to_string(), next),
            (
                format!("Restore to {}/", show(&self.places.restore_dir)),
                format!("→ {}{slash}", show(&dir)),
            ),
        ];
        if is_dir {
            options.push((
                "Write a tar archive".to_string(),
                format!("→ {}", file_name(planned(t, &How::Tar, &self.places))),
            ));
        }
        options
    }

    /// `o`: show the selected file, as it was in that snapshot, in `$PAGER`.
    pub(super) fn pager(&mut self) -> bool {
        if let Some((path, e)) = self.disk_pick() {
            if e.is_dir() {
                self.message = Some("Select a file to show. o shows a file in $PAGER.".into());
                return false;
            }
            let name = e.node.name.to_string_lossy().into_owned();
            self.outbox.push(Request::ReadAllDisk { path, name });
            return true;
        }
        let t = match self.targets() {
            Ok(t) if t.len() == 1 => t[0].clone(),
            Ok(_) => {
                self.message = Some("Select one file to show.".into());
                return false;
            }
            Err(e) => {
                if !e.is_empty() {
                    self.message = Some(e);
                }
                return false;
            }
        };
        if t.node.kind == NodeKind::Dir {
            self.message = Some("Select a file to show. o shows a file in $PAGER.".into());
            return false;
        }
        let name = t.name();
        self.outbox.push(Request::ReadAll { node: t.node, name });
        true
    }

    /// ⏎ in the dialog.
    pub(super) fn dialog_enter(&mut self) {
        let Some(d) = self.dialog.clone() else { return };
        let t = d.target.clone();
        match d.sel {
            0 if self.exists_on_disk(&t) && !d.confirm => {
                self.dialog = Some(Dialog { confirm: true, ..d });
                return;
            }
            0 => self.start_restore(vec![t], How::Overwrite),
            1 => self.start_restore(vec![t], How::NextTo),
            2 => self.start_restore(vec![t], How::RestoreDir),
            _ => self.start_restore(vec![t], How::Tar),
        }
        self.dialog = None;
    }

    /// `cc` `cd` `cf`: snapshot:path (the path alone on disk), the folder,
    /// the name.
    pub(super) fn copy(&mut self, what: char) -> bool {
        if let Some((path, e)) = self.disk_pick() {
            let text = match what {
                'c' => path.display().to_string(),
                'd' => self.folder.display().to_string(),
                _ => e.node.name.to_string_lossy().into_owned(),
            };
            self.message = Some(format!("Copied {text}"));
            self.effects.push(Effect::Clipboard(text));
            return true;
        }
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
        self.forget_disk(None);
        self.ensure();
    }
}
