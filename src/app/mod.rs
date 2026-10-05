//! App state (PLAN.md §4.2): what's on screen and what's loaded. The UI
//! draws from this; repository work goes out as [`Request`]s in `outbox`
//! and comes back through [`App::apply`].

pub mod keys;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::tz::TimeZone;

use crate::index::NodeRef;
use crate::index::folder::Counts;
use crate::index::listing::Entry;
use crate::index::timeline::{ChangeKind, ChangePoint};
use crate::repo::{SnapshotId, SnapshotInfo};
use crate::ui::fmt;
use crate::worker::{Request, Response};

/// What's known about one folder over time.
#[derive(Debug)]
pub struct FolderState {
    /// The timeline set: this machine's snapshots that cover the folder.
    pub set: Arc<Vec<SnapshotInfo>>,
    /// What's at the folder in each snapshot; `None` while indexing.
    pub refs: Option<Arc<Vec<NodeRef>>>,
    pub points: Vec<ChangePoint>,
    /// Counts at change points, by snapshot index.
    pub counts: HashMap<usize, Counts>,
    pub progress: Option<(usize, usize)>,
}

impl FolderState {
    pub fn loaded(&self) -> bool {
        self.refs.is_some()
    }

    /// Whether the folder exists at `set[i]` (assumed while indexing).
    pub fn exists(&self, i: usize) -> bool {
        self.refs.as_ref().is_none_or(|r| r[i].exists())
    }

    pub fn is_change(&self, i: usize) -> bool {
        self.points.iter().any(|p| p.index == i)
    }

    /// Change points where the folder exists: its versions, oldest first.
    pub fn versions(&self) -> Vec<usize> {
        self.points
            .iter()
            .filter(|p| p.kind != ChangeKind::Deleted)
            .map(|p| p.index)
            .collect()
    }

    /// The version `i` belongs to: the last version at or before it.
    pub fn version_at(&self, i: usize) -> Option<usize> {
        self.versions().into_iter().rev().find(|&v| v <= i)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// `..`
    Up,
    Entry(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Down(usize),
    Up(usize),
    Top,
    Bottom,
    HalfDown,
    HalfUp,
    Parent,
    Open,
    Root,
    OlderChange,
    NewerChange,
    OlderSnapshot,
    NewerSnapshot,
    OldestChange,
    NewestChange,
    GoSnapshot(usize),
    GoFolder(PathBuf),
    /// A click on a listing row: select it, or open it if it's selected.
    ClickRow(usize),
    Help,
    Prefix(char),
    Quit,
}

impl Action {
    /// Motions that take a count (`3H`, `5j`).
    pub fn repeats(&self) -> bool {
        matches!(
            self,
            Action::Down(_)
                | Action::Up(_)
                | Action::HalfDown
                | Action::HalfUp
                | Action::OlderChange
                | Action::NewerChange
                | Action::OlderSnapshot
                | Action::NewerSnapshot
        )
    }
}

/// Requests in flight, so each is sent once.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Pending {
    Folder(PathBuf),
    Listing(SnapshotId, PathBuf),
    Deleted(SnapshotId, PathBuf),
}

pub struct App {
    pub tz: TimeZone,
    /// The home folder, shown as `~`.
    pub home: Option<PathBuf>,
    /// This machine's snapshots (§2.4).
    all: Vec<SnapshotInfo>,
    /// The backup root: `h` stops here.
    pub root: PathBuf,
    pub folder: PathBuf,
    /// The snapshot being viewed.
    pub snap: SnapshotId,
    pub folders: HashMap<PathBuf, FolderState>,
    pub listings: HashMap<(SnapshotId, PathBuf), Option<Arc<Vec<Entry>>>>,
    /// How many items were deleted before the snapshot before.
    pub deleted: HashMap<(SnapshotId, PathBuf), usize>,
    pending: HashSet<Pending>,
    /// Index into [`App::rows`].
    pub sel: usize,
    sel_name: Option<OsString>,
    /// Digits typed before a motion.
    pub count: String,
    /// A prefix key waiting for the next key (`g`).
    pub prefix: Option<char>,
    pub help: bool,
    pub message: Option<String>,
    pub zoom: u32,
    /// Set once the user moves in time, so the start-up jump doesn't override it.
    moved: bool,
    pub quit: bool,
    pub outbox: Vec<Request>,
    /// Set when what's on screen changed, so stale requests can be skipped.
    pub bumped: bool,
    /// Rows in the listing pane, from the last draw (for half-page moves).
    pub page: usize,
}

impl App {
    /// `all` is this machine's snapshots; `folder` must be in some of them.
    pub fn new(
        all: Vec<SnapshotInfo>,
        folder: PathBuf,
        tz: TimeZone,
        home: Option<PathBuf>,
    ) -> Self {
        let root = all
            .iter()
            .flat_map(|s| &s.paths)
            .filter(|p| folder.starts_with(p))
            .min_by_key(|p| p.components().count())
            .cloned()
            .unwrap_or_else(|| folder.clone());
        let mut app = App {
            tz,
            home,
            all,
            root,
            folder: folder.clone(),
            snap: SnapshotId::default(),
            folders: HashMap::new(),
            listings: HashMap::new(),
            deleted: HashMap::new(),
            pending: HashSet::new(),
            sel: 0,
            sel_name: None,
            count: String::new(),
            prefix: None,
            help: false,
            message: None,
            zoom: 1,
            moved: false,
            quit: false,
            outbox: Vec::new(),
            bumped: false,
            page: 20,
        };
        let set = app.set_for(&folder);
        if let Some(last) = set.last() {
            app.snap = last.id;
        }
        app.sel = usize::from(app.folder != app.root);
        app.ensure();
        app
    }

    fn set_for(&self, folder: &Path) -> Arc<Vec<SnapshotInfo>> {
        let mut set: Vec<SnapshotInfo> = self
            .all
            .iter()
            .filter(|s| s.paths.iter().any(|p| folder.starts_with(p)))
            .cloned()
            .collect();
        set.sort_by_key(|s| s.time);
        Arc::new(set)
    }

    pub fn state(&self) -> Option<&FolderState> {
        self.folders.get(&self.folder)
    }

    pub fn set(&self) -> Arc<Vec<SnapshotInfo>> {
        self.state()
            .map(|s| s.set.clone())
            .unwrap_or_else(|| self.set_for(&self.folder))
    }

    /// Index of the viewed snapshot in the folder's timeline set.
    pub fn idx(&self) -> usize {
        let set = self.set();
        set.iter()
            .position(|s| s.id == self.snap)
            .unwrap_or(set.len().saturating_sub(1))
    }

    pub fn listing(&self) -> Option<&Option<Arc<Vec<Entry>>>> {
        self.listings.get(&(self.snap, self.folder.clone()))
    }

    /// Rows of the listing: `..` (below the root), then entries.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.folder != self.root {
            rows.push(Row::Up);
        }
        if let Some(Some(entries)) = self.listing() {
            rows.extend((0..entries.len()).map(Row::Entry));
        }
        rows
    }

    pub fn entry(&self, row: Row) -> Option<&Entry> {
        match row {
            Row::Up => None,
            Row::Entry(i) => self.listing()?.as_ref()?.get(i),
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.rows().get(self.sel).and_then(|r| self.entry(*r))
    }

    /// Sends the requests for what's on screen and not yet loaded.
    fn ensure(&mut self) {
        let folder = self.folder.clone();
        if !self.folders.contains_key(&folder) {
            let set = self.set_for(&folder);
            self.folders.insert(
                folder.clone(),
                FolderState {
                    set: set.clone(),
                    refs: None,
                    points: Vec::new(),
                    counts: HashMap::new(),
                    progress: None,
                },
            );
            if self.pending.insert(Pending::Folder(folder.clone())) {
                self.outbox.push(Request::ChangePoints {
                    set,
                    folder: folder.clone(),
                });
            }
        }
        let set = self.set();
        if set.is_empty() {
            return;
        }
        let snap = self.idx();
        let key = (self.snap, folder.clone());
        if !self.listings.contains_key(&key)
            && self
                .pending
                .insert(Pending::Listing(self.snap, folder.clone()))
        {
            self.outbox.push(Request::Listing {
                set: set.clone(),
                snap,
                folder: folder.clone(),
            });
        }
        if !self.deleted.contains_key(&key)
            && self
                .pending
                .insert(Pending::Deleted(self.snap, folder.clone()))
        {
            self.outbox
                .push(Request::DeletedEarlier { set, snap, folder });
        }
    }

    /// The view moved: older on-screen requests may be skipped by the worker,
    /// so forget them and ask again for what's needed now.
    fn moved_on(&mut self) {
        self.bumped = true;
        self.pending.retain(|p| matches!(p, Pending::Folder(_)));
        self.ensure();
    }

    pub fn apply(&mut self, r: Response) {
        match r {
            Response::Progress {
                folder,
                done,
                total,
            } => {
                if let Some(f) = self.folders.get_mut(&folder) {
                    f.progress = Some((done, total));
                }
            }
            Response::ChangePoints {
                folder,
                refs,
                points,
            } => {
                self.pending.remove(&Pending::Folder(folder.clone()));
                let Some(f) = self.folders.get_mut(&folder) else {
                    return;
                };
                f.refs = Some(refs.clone());
                f.points = points;
                f.progress = None;
                // Counts, newest first, in batches so the top of the Versions column fills first.
                let newest_first: Vec<usize> = f.points.iter().rev().map(|p| p.index).collect();
                for chunk in newest_first.chunks(64) {
                    self.outbox.push(Request::PointCounts {
                        folder: folder.clone(),
                        refs: refs.clone(),
                        points: chunk.to_vec(),
                    });
                }
                // Start at the newest snapshot that changed the folder (§3.1).
                if folder == self.folder && !self.moved {
                    let f = &self.folders[&folder];
                    if let Some(&v) = f.versions().last() {
                        let id = f.set[v].id;
                        self.go_snapshot_id(id);
                        self.moved = false;
                    }
                }
            }
            Response::PointCounts { folder, counts } => {
                if let Some(f) = self.folders.get_mut(&folder) {
                    f.counts.extend(counts);
                }
            }
            Response::Listing {
                snapshot,
                folder,
                entries,
            } => {
                self.pending
                    .remove(&Pending::Listing(snapshot, folder.clone()));
                self.listings.insert((snapshot, folder), entries);
                self.restore_selection();
            }
            Response::DeletedEarlier {
                snapshot,
                folder,
                names,
            } => {
                self.pending
                    .remove(&Pending::Deleted(snapshot, folder.clone()));
                self.deleted.insert((snapshot, folder), names.len());
            }
            Response::Error(e) => self.message = Some(e),
        }
    }

    /// Keeps the same name selected when the listing changes.
    fn restore_selection(&mut self) {
        // Keep the place until the listing is there.
        if !matches!(self.listing(), Some(Some(_))) {
            return;
        }
        let rows = self.rows();
        if let Some(name) = &self.sel_name
            && let Some(i) = rows
                .iter()
                .position(|r| self.entry(*r).is_some_and(|e| &e.node.name == name))
        {
            self.sel = i;
            return;
        }
        self.sel = self.sel.min(rows.len().saturating_sub(1));
    }

    fn select(&mut self, i: usize) {
        let rows = self.rows();
        self.sel = i.min(rows.len().saturating_sub(1));
        self.sel_name = rows
            .get(self.sel)
            .and_then(|r| self.entry(*r))
            .map(|e| e.node.name.clone());
    }

    fn go_snapshot_id(&mut self, id: SnapshotId) {
        if id != self.snap {
            self.snap = id;
            self.moved = true;
            self.moved_on();
            self.restore_selection();
        }
    }

    fn go_index(&mut self, i: usize) {
        let set = self.set();
        if let Some(s) = set.get(i) {
            self.go_snapshot_id(s.id);
        }
    }

    fn go_folder(&mut self, folder: PathBuf, select: Option<OsString>) {
        let time = self.set().get(self.idx()).map(|s| s.time);
        self.folder = folder;
        let set = self.set_for(&self.folder);
        // Same snapshot if the new folder's set has it, else the last one before.
        if !set.iter().any(|s| s.id == self.snap)
            && let Some(t) = time
            && let Some(s) = set.iter().rev().find(|s| s.time <= t).or(set.first())
        {
            self.snap = s.id;
        }
        self.sel_name = select;
        self.sel = usize::from(self.folder != self.root);
        self.moved_on();
        self.restore_selection();
    }

    fn display(&self, p: &Path) -> String {
        fmt::path(p, self.home.as_deref())
    }

    fn time(&self, i: usize) -> String {
        fmt::time(self.set()[i].time, &self.tz)
    }

    /// Does an action. Returns false if it stopped at a boundary (with a message).
    pub fn act(&mut self, a: Action) -> bool {
        let rows = self.rows().len();
        let i = self.idx();
        let set = self.set();
        let state = self.state();
        let loaded = state.is_some_and(|s| s.loaded());
        let versions = state.map(|s| s.versions()).unwrap_or_default();
        let fail = |app: &mut App, msg: String| {
            app.message = Some(msg);
            false
        };
        match a {
            Action::Down(n) => self.select(self.sel + n),
            Action::Up(n) => self.select(self.sel.saturating_sub(n)),
            Action::Top => self.select(0),
            Action::Bottom => self.select(rows.saturating_sub(1)),
            Action::HalfDown => self.select(self.sel + (self.page / 2).max(1)),
            Action::HalfUp => self.select(self.sel.saturating_sub((self.page / 2).max(1))),
            Action::Parent => {
                if self.folder == self.root {
                    return fail(self, "Already at the top of the backup.".into());
                }
                let from = self.folder.file_name().map(|n| n.to_os_string());
                let parent = self.folder.parent().unwrap_or(&self.root).to_path_buf();
                self.go_folder(parent, from);
            }
            Action::Root => {
                if self.folder != self.root {
                    self.go_folder(self.root.clone(), None);
                }
            }
            Action::Open => match self.rows().get(self.sel).copied() {
                Some(Row::Up) => return self.act(Action::Parent),
                Some(row) => {
                    let Some(e) = self.entry(row).cloned() else {
                        return true;
                    };
                    if e.is_dir() {
                        if e.is_deleted() && i > 0 {
                            self.go_index(i - 1);
                            let name = e.node.name.to_string_lossy();
                            self.message = Some(format!(
                                "Jumped to {}, the last snapshot that has {name}/",
                                self.time(i - 1)
                            ));
                        }
                        self.go_folder(self.folder.join(&e.node.name), None);
                    } else {
                        return fail(self, "File versions aren't available yet.".into());
                    }
                }
                None => {}
            },
            Action::ClickRow(r) => {
                if r == self.sel {
                    return self.act(Action::Open);
                }
                self.select(r);
            }
            Action::OlderChange => {
                if !loaded {
                    return true;
                }
                match versions.iter().rev().find(|&&v| v < i) {
                    Some(&v) => self.go_index(v),
                    None => return fail(self, "This is the oldest version of this folder.".into()),
                }
            }
            Action::NewerChange => {
                if !loaded {
                    return true;
                }
                match versions.iter().find(|&&v| v > i) {
                    Some(&v) => self.go_index(v),
                    None if i + 1 < set.len()
                        && self.state().is_some_and(|s| s.exists(set.len() - 1)) =>
                    {
                        self.go_index(set.len() - 1)
                    }
                    None => {
                        return fail(
                            self,
                            "Newest snapshot. Nothing changed on disk since.".into(),
                        );
                    }
                }
            }
            Action::OlderSnapshot => {
                if i == 0 || !self.state().is_some_and(|s| s.exists(i - 1)) {
                    return fail(self, "No older snapshot of this folder.".into());
                }
                self.go_index(i - 1);
            }
            Action::NewerSnapshot => {
                if i + 1 >= set.len() {
                    return fail(self, "This is the newest snapshot.".into());
                }
                if !self.state().is_some_and(|s| s.exists(i + 1)) {
                    let msg = format!(
                        "{}/ did not exist then. Go up a folder and try again.",
                        self.display(&self.folder)
                    );
                    return fail(self, msg);
                }
                self.go_index(i + 1);
            }
            Action::OldestChange => {
                if let Some(&v) = versions.first() {
                    self.go_index(v);
                }
            }
            Action::NewestChange => {
                if let Some(&v) = versions.last() {
                    self.go_index(v);
                }
            }
            Action::GoSnapshot(j) => {
                if self.state().is_some_and(|s| s.exists(j)) {
                    self.go_index(j);
                } else {
                    let msg = format!(
                        "{}/ did not exist then. Go up a folder and try again.",
                        self.display(&self.folder)
                    );
                    return fail(self, msg);
                }
            }
            Action::GoFolder(p) => {
                if p != self.folder {
                    self.go_folder(p, None);
                }
            }
            Action::Help => self.help = true,
            Action::Prefix(c) => self.prefix = Some(c),
            Action::Quit => self.quit = true,
        }
        true
    }

    /// Does an action `n` times, stopping at the first boundary message.
    pub fn act_n(&mut self, a: Action, n: usize) {
        let n = if a.repeats() { n.max(1) } else { 1 };
        for _ in 0..n {
            if !self.act(a.clone()) {
                break;
            }
        }
    }
}
