//! App state (PLAN.md §4.2): what's on screen and what's loaded. The UI
//! draws from this; repository and disk work goes out as [`Request`]s in
//! `outbox` and comes back through [`App::apply`].

pub mod cmdline;
pub mod keys;
pub mod selection;

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::tz::TimeZone;

use crate::diff::{DIFF_LIMIT, DiffKey, FORCED_LIMIT, FileDiff, HunkLine, SideKey};
use crate::index::NodeRef;
use crate::index::find::Found;
use crate::index::fingerprint::{self, Fp};
use crate::index::folder::Counts;
use crate::index::listing::{self, Delta, Entry};
use crate::index::timeline::{ChangeKind, ChangePoint};
use crate::index::timeline::{Filter, covers, explain_empty};
use crate::index::versions::{Run, run_at};
use crate::index::{Mode, ModeSwitch};
use crate::repo::{FileBytes, Node, SnapshotId, SnapshotInfo};
use crate::restore::{How, Places, Target};
use crate::ui::fmt;
use crate::worker::{Cancel, Request, Response, Side};
use cmdline::{Input, InputKind};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
pub use selection::Restoring;
use selection::{Confirm, Dialog};

/// Things the event loop does outside the app: they need the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Clipboard(String),
    /// Show a file in `$PAGER`.
    Pager {
        name: String,
        bytes: Vec<u8>,
    },
}

/// What's known about one path over time: the current folder, or the
/// selected entry (its "item track").
#[derive(Debug)]
pub struct Track {
    /// The timeline set: this machine's snapshots that cover the path.
    pub set: Arc<Vec<SnapshotInfo>>,
    /// What's at the path in each snapshot; `None` while indexing.
    pub refs: Option<Arc<Vec<NodeRef>>>,
    pub points: Vec<ChangePoint>,
    /// Versions and missing periods, oldest first.
    pub runs: Vec<Run>,
    /// Counts at change points, by snapshot index (folders only).
    pub counts: HashMap<usize, Counts>,
    pub progress: Option<(usize, usize)>,
}

impl Track {
    fn new(set: Arc<Vec<SnapshotInfo>>) -> Self {
        Track {
            set,
            refs: None,
            points: Vec::new(),
            runs: Vec::new(),
            counts: HashMap::new(),
            progress: None,
        }
    }

    pub fn loaded(&self) -> bool {
        self.refs.is_some()
    }

    /// Whether the path exists at `set[i]` (assumed while indexing).
    pub fn exists(&self, i: usize) -> bool {
        self.refs
            .as_ref()
            .is_none_or(|r| r.get(i).is_some_and(|r| r.exists()))
    }

    pub fn is_change(&self, i: usize) -> bool {
        self.points.iter().any(|p| p.index == i)
    }

    /// Change points where the path exists: its versions, oldest first.
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

    /// Index of a snapshot in this track's set.
    pub fn index_of(&self, id: SnapshotId) -> Option<usize> {
        self.set.iter().position(|s| s.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// `..`
    Up,
    Entry(usize),
    /// An item deleted earlier, shown with `.`.
    Ghost(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    #[default]
    Content,
    /// An inline diff against the file on disk.
    Disk,
}

/// The versions of one file (PLAN.md §3.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionsView {
    pub path: PathBuf,
    /// Index into the runs, newest first.
    pub sel: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffMode {
    /// The selected version → the file on disk (`c`).
    Disk,
    /// The version before → the selected version (`p`).
    Previous,
}

/// The full-screen diff (PLAN.md §3.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffView {
    pub path: PathBuf,
    /// Index into the runs, newest first; always a version that exists.
    pub run: usize,
    pub mode: DiffMode,
    pub scroll: usize,
    /// Read past the size limit (asked for with ⏎).
    pub force: bool,
    /// Back to the versions view rather than the folder.
    pub from_versions: bool,
}

/// `:find` results (PLAN.md §3.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindView {
    pub query: String,
    /// Where it searched from, and the snapshots it searched.
    pub root: PathBuf,
    pub set: Arc<Vec<SnapshotInfo>>,
    /// Each with whether it's on disk now.
    pub results: Vec<(Found, bool)>,
    /// `(done, total)` while still searching.
    pub progress: Option<(usize, usize)>,
    pub sel: usize,
    /// Stops the search when the view is left.
    pub cancel: Cancel,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Folder,
    Versions(VersionsView),
    Diff(DiffView),
    Find(FindView),
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
    OlderItemChange,
    NewerItemChange,
    GoSnapshot(usize),
    GoFolder(PathBuf),
    /// A click on a listing row: select it, or open it if it's selected.
    ClickRow(usize),
    /// A click on a row of the versions view.
    ClickVersion(usize),
    TogglePreview,
    /// Scroll the preview by this many lines.
    Scroll(isize),
    ToggleDeleted,
    /// Full-screen diff against disk (`d`; `⏎` in the versions view).
    Diff,
    /// Full-screen diff against the previous version (`p` in versions and diff).
    DiffPrevious,
    NextHunk,
    PrevHunk,
    Back,
    ToggleMark,
    Visual,
    Yank,
    Paste,
    PasteOver,
    RestoreDialog,
    /// `cc` `cd` `cf`
    Copy(char),
    CommandLine,
    /// `esc`: leave visual mode, then clear the selection.
    Escape,
    /// `/`, `f`, `s`
    Search,
    FilterInput,
    Find,
    NextMatch,
    PrevMatch,
    ZoomIn,
    ZoomOut,
    /// A click on a find result: select it, or go there if selected.
    ClickFound(usize),
    /// A click on a restore dialog option: select it, or restore if selected.
    DialogOption(usize),
    /// `o`: show the file in `$PAGER`.
    Pager,
    DialogRestore,
    DialogCancel,
    ConfirmYes,
    ConfirmNo,
    Help,
    Prefix(char),
    Quit,
}

impl Action {
    /// Motions that take a count (`3H`, `5j`, `2J`).
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
                | Action::OlderItemChange
                | Action::NewerItemChange
                | Action::Scroll(_)
                | Action::NextHunk
                | Action::PrevHunk
                | Action::NextMatch
                | Action::PrevMatch
                | Action::ZoomIn
                | Action::ZoomOut
        )
    }
}

/// Lines added and removed going to the file on disk; `None` for binary.
pub type DiskStat = Option<(usize, usize)>;

/// Requests in flight, so each is sent once.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Pending {
    Track(PathBuf),
    Listing(SnapshotId, PathBuf),
    Deleted(SnapshotId, PathBuf),
    Live(PathBuf),
    File(Fp),
    Disk(PathBuf),
    Node(PathBuf, usize),
    /// Asked once per folder; never forgotten.
    Counts(PathBuf),
    Diff(DiffKey),
}

impl Pending {
    /// Kept when the view moves on: their requests are never skipped.
    fn kept(&self, folder: &Path) -> bool {
        match self {
            Pending::Track(p) | Pending::Live(p) => p == folder,
            Pending::Counts(_) => true,
            _ => false,
        }
    }
}

pub struct App {
    pub tz: TimeZone,
    /// The home folder, shown as `~`.
    pub home: Option<PathBuf>,
    /// Every snapshot in the repository.
    everything: Vec<SnapshotInfo>,
    /// Which of them are this machine's (§2.4); `:host`, `:tag`.
    pub filter: Filter,
    /// This machine's hosts. A filter host outside them is a foreign host
    /// (§3.18): restores ask for a directory and `P` is off.
    pub mine: Vec<String>,
    /// The host picked in the picker, for the title bar.
    pub shown_host: Option<String>,
    /// The folder restoric started in: the default restore directory on a
    /// foreign host.
    pub start_dir: PathBuf,
    /// The last directory a foreign-host restore went to this session.
    pub last_dir: Option<PathBuf>,
    /// What the `restore to:` prompt will restore.
    pub(crate) pending_targets: Option<Vec<Target>>,
    /// The snapshots that pass the filter.
    all: Vec<SnapshotInfo>,
    /// Switches the worker's change detection (`:set strict`).
    pub mode_switch: Option<ModeSwitch>,
    pub strict: bool,
    /// "Today", for `:yesterday`; the clock when `None` (tests set it).
    pub now: Option<jiff::Timestamp>,
    /// `/`: matches are highlighted, `n` `N` step through them.
    pub search: String,
    /// `f`: only names containing it are listed.
    pub name_filter: String,
    /// Keys from the config's `[keys]`, before the built-in ones.
    pub keymap: HashMap<(KeyCode, KeyModifiers), Action>,
    /// Nerd Font icons (or the plain set).
    pub icons: bool,
    /// Files larger than this are diffed only on request.
    pub diff_limit: u64,
    /// `--at DATE`: where to start instead of the newest change.
    pub start_at: Option<String>,
    /// The backup root: `h` stops here.
    pub root: PathBuf,
    pub folder: PathBuf,
    /// The snapshot being viewed.
    pub snap: SnapshotId,
    pub view: View,
    pub tracks: HashMap<PathBuf, Track>,
    pub listings: HashMap<(SnapshotId, PathBuf), Option<Arc<Vec<Entry>>>>,
    /// Items deleted before the snapshot before, by snapshot and folder.
    pub deleted: HashMap<(SnapshotId, PathBuf), Arc<Vec<Entry>>>,
    /// What changed on disk since the newest snapshot, by path.
    pub live: HashMap<PathBuf, Counts>,
    /// File contents from the repository, by content.
    pub files: HashMap<Fp, Arc<FileBytes>>,
    /// File contents on disk (`None`: missing or not a file).
    pub disk_files: HashMap<PathBuf, Option<Arc<FileBytes>>>,
    /// Lines added and removed against disk, by (content, path).
    pub disk_stats: RefCell<HashMap<(Fp, PathBuf), DiskStat>>,
    /// Nodes at (path, snapshot index in the path's set).
    pub nodes: HashMap<(PathBuf, usize), Option<Node>>,
    /// Full-screen diffs.
    pub diffs: HashMap<DiffKey, Arc<FileDiff>>,
    pending: HashSet<Pending>,
    /// Show items deleted earlier (`.`).
    pub ghosts: bool,
    pub preview: PreviewMode,
    /// Preview scroll; `None` until the preview picks its first line.
    pub scroll: Option<usize>,
    /// The first line the preview showed at the last draw.
    pub scroll_base: Cell<usize>,
    scroll_key: String,
    /// Index into [`App::rows`].
    pub sel: usize,
    sel_name: Option<OsString>,
    /// Digits typed before a motion.
    pub count: String,
    /// A prefix key waiting for the next key (`g`, `z`).
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
    /// Selected names in the current folder (`Space`, `v`).
    pub marks: HashSet<OsString>,
    /// Visual mode's anchor row.
    pub visual: Option<usize>,
    /// What `y` yanked, for `p` and `P`.
    pub yanked: Option<Vec<Target>>,
    pub dialog: Option<Dialog>,
    pub confirm: Option<Confirm>,
    /// The restore that's running.
    pub restoring: Option<Restoring>,
    /// The `:` line being typed.
    pub input: Option<Input>,
    pub effects: Vec<Effect>,
    pub places: Places,
    /// Whether paths exist on disk (for the restore dialog).
    pub exists: HashMap<PathBuf, bool>,
}

impl App {
    /// `everything` is every snapshot, of which `filter` picks this
    /// machine's; `folder` must be in some of those.
    pub fn new(
        everything: Vec<SnapshotInfo>,
        filter: Filter,
        folder: PathBuf,
        tz: TimeZone,
        home: Option<PathBuf>,
    ) -> Self {
        let all: Vec<SnapshotInfo> = everything
            .iter()
            .filter(|s| filter.matches(s))
            .cloned()
            .collect();
        let root = all
            .iter()
            .flat_map(|s| &s.paths)
            .filter(|p| folder.starts_with(p))
            .min_by_key(|p| p.components().count())
            .cloned()
            .unwrap_or_else(|| folder.clone());
        let mut app = App {
            tz: tz.clone(),
            home,
            everything,
            filter,
            mine: Vec::new(),
            shown_host: None,
            start_dir: folder.clone(),
            last_dir: None,
            pending_targets: None,
            all,
            mode_switch: None,
            strict: false,
            now: None,
            search: String::new(),
            name_filter: String::new(),
            keymap: HashMap::new(),
            icons: false,
            diff_limit: DIFF_LIMIT,
            start_at: None,
            root,
            folder: folder.clone(),
            snap: SnapshotId::default(),
            view: View::Folder,
            tracks: HashMap::new(),
            listings: HashMap::new(),
            deleted: HashMap::new(),
            live: HashMap::new(),
            files: HashMap::new(),
            disk_files: HashMap::new(),
            disk_stats: RefCell::new(HashMap::new()),
            nodes: HashMap::new(),
            diffs: HashMap::new(),
            pending: HashSet::new(),
            ghosts: false,
            preview: PreviewMode::Content,
            scroll: None,
            scroll_base: Cell::new(0),
            scroll_key: String::new(),
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
            marks: HashSet::new(),
            visual: None,
            yanked: None,
            dialog: None,
            confirm: None,
            restoring: None,
            input: None,
            effects: Vec::new(),
            places: Places::default_for(tz.clone()),
            exists: HashMap::new(),
        };
        let set = app.set_for(&folder);
        if let Some(last) = set.last() {
            app.snap = last.id;
        }
        app.sel = usize::from(app.folder != app.root);
        app.ensure();
        app
    }

    pub fn set_for(&self, path: &Path) -> Arc<Vec<SnapshotInfo>> {
        if let Some(t) = self.tracks.get(path) {
            return t.set.clone();
        }
        let mut set: Vec<SnapshotInfo> = self
            .all
            .iter()
            .filter(|s| covers(s, path))
            .cloned()
            .collect();
        set.sort_by_key(|s| s.time);
        Arc::new(set)
    }

    /// The current folder's track.
    pub fn state(&self) -> Option<&Track> {
        self.tracks.get(&self.folder)
    }

    pub fn set(&self) -> Arc<Vec<SnapshotInfo>> {
        self.set_for(&self.folder)
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

    fn ghost_entries(&self) -> Option<&Arc<Vec<Entry>>> {
        self.deleted.get(&(self.snap, self.folder.clone()))
    }

    /// How many items deleted earlier are hidden.
    pub fn hidden(&self) -> usize {
        if self.ghosts {
            0
        } else {
            self.ghost_entries().map_or(0, |g| g.len())
        }
    }

    /// Rows of the listing: `..` (below the root), then entries, with the
    /// items deleted earlier among them when shown.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.folder != self.root {
            rows.push(Row::Up);
        }
        let Some(Some(entries)) = self.listing() else {
            return rows;
        };
        let shown = |e: &Entry| {
            self.name_filter.is_empty()
                || e.node
                    .name
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&self.name_filter.to_lowercase())
        };
        let ghosts: &[Entry] = match (self.ghosts, self.ghost_entries()) {
            (true, Some(g)) => g,
            _ => &[],
        };
        let (mut i, mut j) = (0, 0);
        while i < entries.len() || j < ghosts.len() {
            let take_entry = match (entries.get(i), ghosts.get(j)) {
                (Some(e), Some(g)) => listing::cmp(e, g) != Ordering::Greater,
                (Some(_), None) => true,
                _ => false,
            };
            if take_entry {
                if shown(&entries[i]) {
                    rows.push(Row::Entry(i));
                }
                i += 1;
            } else {
                if shown(&ghosts[j]) {
                    rows.push(Row::Ghost(j));
                }
                j += 1;
            }
        }
        rows
    }

    pub fn entry(&self, row: Row) -> Option<&Entry> {
        match row {
            Row::Up => None,
            Row::Entry(i) => self.listing()?.as_ref()?.get(i),
            Row::Ghost(i) => self.ghost_entries()?.get(i),
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.rows().get(self.sel).and_then(|r| self.entry(*r))
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected().map(|e| self.folder.join(&e.node.name))
    }

    /// The snapshot (index in the folder's set) where the selected entry is
    /// shown: this one, or the last one that had it if it's gone.
    pub fn entry_snapshot(&self, e: &Entry) -> usize {
        let i = self.idx();
        match e.delta {
            Delta::Deleted => i.saturating_sub(1),
            Delta::Gone(last) => last,
            _ => i,
        }
    }

    /// The file node shown in the preview of the versions view.
    pub fn version_node(&self, v: &VersionsView) -> Option<(Run, Option<&Node>)> {
        let t = self.tracks.get(&v.path)?;
        let run = *t.runs.iter().rev().nth(v.sel)?;
        let node = self
            .nodes
            .get(&(v.path.clone(), run.from))
            .and_then(Option::as_ref);
        Some((run, node))
    }

    fn request(&mut self, p: Pending, r: Request) {
        if self.pending.insert(p) {
            self.outbox.push(r);
        }
    }

    fn want_track(&mut self, path: &Path, item: bool) {
        if !self.tracks.contains_key(path) {
            let set = self.set_for(path);
            self.tracks
                .insert(path.to_path_buf(), Track::new(set.clone()));
            self.request(
                Pending::Track(path.to_path_buf()),
                Request::ChangePoints {
                    set,
                    path: path.to_path_buf(),
                    item,
                },
            );
        }
    }

    /// Counts at the folder's change points, newest first, in batches so
    /// the top of the Versions column fills first.
    fn want_counts(&mut self, folder: &Path) {
        let Some(t) = self.tracks.get(folder).filter(|t| t.loaded()) else {
            return;
        };
        let refs = t.refs.clone().unwrap_or_default();
        if !refs.iter().any(|r| matches!(r, NodeRef::Dir(_)))
            || !self.pending.insert(Pending::Counts(folder.to_path_buf()))
        {
            return;
        }
        let newest_first: Vec<usize> = t.points.iter().rev().map(|p| p.index).collect();
        for chunk in newest_first.chunks(64) {
            self.outbox.push(Request::PointCounts {
                path: folder.to_path_buf(),
                refs: refs.clone(),
                points: chunk.to_vec(),
            });
        }
    }

    fn want_live(&mut self, path: &Path, item: bool) {
        if !self.live.contains_key(path) {
            let set = self.set_for(path);
            self.request(
                Pending::Live(path.to_path_buf()),
                Request::Live {
                    set,
                    path: path.to_path_buf(),
                    item,
                },
            );
        }
    }

    fn want_listing(&mut self, folder: &Path, snap: SnapshotId) {
        let key = (snap, folder.to_path_buf());
        if self.listings.contains_key(&key) {
            return;
        }
        let set = self.set_for(folder);
        if let Some(i) = set.iter().position(|s| s.id == snap) {
            self.request(
                Pending::Listing(snap, folder.to_path_buf()),
                Request::Listing {
                    set,
                    snap: i,
                    folder: folder.to_path_buf(),
                },
            );
        }
    }

    fn want_file(&mut self, node: &Node) {
        let key = fingerprint::content(node);
        if !self.files.contains_key(&key) {
            self.request(Pending::File(key), Request::ReadFile { node: node.clone() });
        }
    }

    fn want_disk(&mut self, path: &Path) {
        if !self.disk_files.contains_key(path) {
            self.request(
                Pending::Disk(path.to_path_buf()),
                Request::ReadDisk {
                    path: path.to_path_buf(),
                },
            );
        }
    }

    fn want_node(&mut self, path: &Path, i: usize) {
        if !self.nodes.contains_key(&(path.to_path_buf(), i)) {
            let set = self.set_for(path);
            self.request(
                Pending::Node(path.to_path_buf(), i),
                Request::Nodes {
                    set,
                    path: path.to_path_buf(),
                    snaps: vec![i],
                },
            );
        }
    }

    /// The node of the version before the one holding `s` (an index into
    /// the path's own set), once known.
    pub fn previous_version(&self, path: &Path, s: usize) -> Option<Option<&Node>> {
        let t = self.tracks.get(path).filter(|t| t.loaded())?;
        let (k, _) = run_at(&t.runs, s)?;
        let prev = t.runs[..k].iter().rev().find(|r| r.exists);
        match prev {
            None => Some(None),
            Some(r) => self
                .nodes
                .get(&(path.to_path_buf(), r.from))
                .map(Option::as_ref),
        }
    }

    /// Sends the requests for what's on screen and not yet loaded.
    fn ensure(&mut self) {
        let folder = self.folder.clone();
        self.want_track(&folder, false);
        self.want_live(&folder, false);
        self.want_counts(&folder);
        if self.set().is_empty() {
            return;
        }
        self.want_listing(&folder, self.snap);
        let key = (self.snap, folder.clone());
        if !self.deleted.contains_key(&key) {
            let set = self.set();
            let snap = self.idx();
            self.request(
                Pending::Deleted(self.snap, folder.clone()),
                Request::DeletedEarlier {
                    set,
                    snap,
                    folder: folder.clone(),
                },
            );
        }

        match self.view.clone() {
            View::Folder => self.ensure_selected(),
            View::Versions(v) => self.ensure_versions(&v),
            View::Diff(d) => self.ensure_diff(&d),
            View::Find(_) => {}
        }

        let key = self.preview_key();
        if key != self.scroll_key {
            self.scroll_key = key;
            self.scroll = None;
        }
    }

    fn ensure_selected(&mut self) {
        let Some(e) = self.selected().cloned() else {
            return;
        };
        let path = self.folder.join(&e.node.name);
        self.want_track(&path, true);
        self.want_live(&path, true);
        let s = self.entry_snapshot(&e);
        let snap_id = self.set().get(s).map(|x| x.id);
        if e.is_dir() {
            if let Some(id) = snap_id {
                self.want_listing(&path, id);
            }
            return;
        }
        if e.node.kind != crate::repo::NodeKind::File {
            return;
        }
        match self.preview {
            PreviewMode::Content => {
                self.want_file(&e.node);
                // The version before, for the margin marks.
                let t = self.tracks.get(&path).filter(|t| t.loaded());
                let prev = t.and_then(|t| {
                    let s = t.index_of(snap_id?)?;
                    let (k, _) = run_at(&t.runs, s)?;
                    t.runs[..k].iter().rev().find(|r| r.exists).map(|r| r.from)
                });
                if let Some(p) = prev {
                    self.want_node(&path, p);
                    if let Some(Some(n)) = self.nodes.get(&(path.clone(), p)).cloned() {
                        self.want_file(&n);
                    }
                }
            }
            PreviewMode::Disk => {
                self.want_file(&e.node);
                self.want_disk(&path);
            }
        }
    }

    fn ensure_versions(&mut self, v: &VersionsView) {
        let path = v.path.clone();
        self.want_track(&path, false);
        self.want_live(&path, true);
        self.want_disk(&path);
        let Some(t) = self.tracks.get(&path).filter(|t| t.loaded()) else {
            return;
        };
        let starts: Vec<usize> = t
            .runs
            .iter()
            .rev()
            .filter(|r| r.exists)
            .map(|r| r.from)
            .take(200)
            .collect();
        for s in starts {
            self.want_node(&path, s);
            if let Some(Some(n)) = self.nodes.get(&(path.clone(), s)).cloned() {
                self.want_file(&n);
            }
        }
    }

    /// The two sides of a diff and their labels, once their nodes are known.
    pub fn diff_sides(&self, d: &DiffView) -> Option<(DiffKey, Side, Side, String, String)> {
        let t = self.tracks.get(&d.path).filter(|t| t.loaded())?;
        let runs: Vec<Run> = t.runs.iter().rev().copied().collect();
        let r0 = runs.get(d.run)?;
        let node = |r: &Run| -> Option<Node> { self.nodes.get(&(d.path.clone(), r.from))?.clone() };
        let label = |r: &Run| fmt::time(t.set[r.from].time, &self.tz);
        let repo_side = |n: Node| (SideKey::Content(fingerprint::content(&n)), Side::Repo(n));
        let ((ok, os, ol), (nk, ns, nl)) = match d.mode {
            DiffMode::Disk => {
                let (k, s) = repo_side(node(r0)?);
                (
                    (k, s, label(r0)),
                    (
                        SideKey::Disk(d.path.clone()),
                        Side::Disk(d.path.clone()),
                        "on disk".to_string(),
                    ),
                )
            }
            DiffMode::Previous => {
                let older = runs[d.run + 1..].iter().find(|r| r.exists);
                let old = match older {
                    Some(r) => {
                        let (k, s) = repo_side(node(r)?);
                        (k, s, label(r))
                    }
                    None => (SideKey::Nothing, Side::Nothing, "(nothing)".to_string()),
                };
                let (k, s) = repo_side(node(r0)?);
                (old, (k, s, label(r0)))
            }
        };
        let limit = if d.force {
            FORCED_LIMIT
        } else {
            self.diff_limit
        };
        let key = DiffKey {
            old: ok,
            new: nk,
            limit,
        };
        Some((key, os, ns, ol, nl))
    }

    fn ensure_diff(&mut self, d: &DiffView) {
        let path = d.path.clone();
        self.want_track(&path, false);
        let Some(t) = self.tracks.get(&path).filter(|t| t.loaded()) else {
            return;
        };
        let runs: Vec<Run> = t.runs.iter().rev().copied().collect();
        let mut wanted = Vec::new();
        if let Some(r0) = runs.get(d.run) {
            wanted.push(r0.from);
        }
        if let Some(r) = runs
            .get(d.run + 1..)
            .and_then(|r| r.iter().find(|r| r.exists))
        {
            wanted.push(r.from);
        }
        for s in wanted {
            self.want_node(&path, s);
        }
        if let Some((key, old, new, _, _)) = self.diff_sides(d)
            && !self.diffs.contains_key(&key)
        {
            let (old, new) = (Box::new(old), Box::new(new));
            self.request(Pending::Diff(key.clone()), Request::Diff { key, old, new });
        }
    }

    /// Opens the full-screen diff at the version `run` (newest first) or
    /// the next older one that exists.
    fn open_diff(
        &mut self,
        path: PathBuf,
        run: usize,
        mode: DiffMode,
        from_versions: bool,
    ) -> bool {
        let runs: Vec<Run> = self
            .tracks
            .get(&path)
            .map(|t| t.runs.iter().rev().copied().collect())
            .unwrap_or_default();
        let Some(k) = (run..runs.len()).find(|&k| runs[k].exists) else {
            self.message = Some("No saved version of this file to compare.".into());
            return false;
        };
        // Look at the file on disk afresh.
        let disk = SideKey::Disk(path.clone());
        self.diffs
            .retain(|key, _| key.old != disk && key.new != disk);
        self.disk_files.remove(&path);
        self.disk_stats.borrow_mut().retain(|(_, p), _| *p != path);
        self.view = View::Diff(DiffView {
            path,
            run: k,
            mode,
            scroll: 0,
            force: false,
            from_versions,
        });
        self.moved_on();
        true
    }

    fn preview_key(&self) -> String {
        match &self.view {
            View::Find(_) => "find".into(),
            View::Folder => format!(
                "f|{}|{:?}|{}|{:?}",
                self.folder.display(),
                self.sel_name,
                self.snap,
                self.preview
            ),
            View::Versions(v) => format!("v|{}|{}|{:?}", v.path.display(), v.sel, self.preview),
            View::Diff(d) => format!("d|{}|{}|{:?}", d.path.display(), d.run, d.mode),
        }
    }

    /// The view moved: older on-screen requests may be skipped by the
    /// worker, so forget them and ask again for what's needed now.
    fn moved_on(&mut self) {
        self.bumped = true;
        let folder = self.folder.clone();
        self.pending.retain(|p| p.kept(&folder));
        // Item tracks that were skipped are asked for again.
        self.tracks.retain(|p, t| t.loaded() || *p == folder);
        self.ensure();
    }

    pub fn apply(&mut self, r: Response) {
        match r {
            Response::Progress { path, done, total } => {
                if let Some(t) = self.tracks.get_mut(&path) {
                    t.progress = Some((done, total));
                }
            }
            Response::ChangePoints {
                path,
                refs,
                points,
                runs,
            } => {
                self.pending.remove(&Pending::Track(path.clone()));
                let Some(t) = self.tracks.get_mut(&path) else {
                    return;
                };
                t.refs = Some(refs.clone());
                t.points = points;
                t.runs = runs;
                t.progress = None;
                // Start at the newest snapshot that changed the folder (§3.1),
                // or at `--at`.
                if path == self.folder && !self.moved {
                    if let Some(at) = self.start_at.take() {
                        self.run_command(&at);
                    } else {
                        let t = &self.tracks[&path];
                        if let Some(&v) = t.versions().last() {
                            let id = t.set[v].id;
                            self.go_snapshot_id(id);
                            self.moved = false;
                        }
                    }
                }
                self.ensure();
            }
            Response::PointCounts { path, counts } => {
                if let Some(t) = self.tracks.get_mut(&path) {
                    t.counts.extend(counts);
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
                self.ensure();
            }
            Response::DeletedEarlier {
                snapshot,
                folder,
                entries,
            } => {
                self.pending
                    .remove(&Pending::Deleted(snapshot, folder.clone()));
                self.deleted.insert((snapshot, folder), entries);
                self.restore_selection();
            }
            Response::Live { path, counts } => {
                self.pending.remove(&Pending::Live(path.clone()));
                self.live.insert(path, counts);
            }
            Response::File { key, bytes } => {
                self.pending.remove(&Pending::File(key));
                self.files.insert(key, bytes);
            }
            Response::DiskFile { path, bytes } => {
                self.pending.remove(&Pending::Disk(path.clone()));
                self.disk_stats.borrow_mut().retain(|(_, p), _| *p != path);
                self.disk_files.insert(path, bytes);
            }
            Response::Nodes { path, nodes } => {
                for (i, n) in nodes {
                    self.pending.remove(&Pending::Node(path.clone(), i));
                    self.nodes.insert((path.clone(), i), n);
                }
                self.ensure();
            }
            Response::Diff { key, diff } => {
                self.pending.remove(&Pending::Diff(key.clone()));
                self.diffs.insert(key, diff);
            }
            Response::RestoreProgress(p) => {
                if let Some(r) = &mut self.restoring {
                    r.progress = Some(p);
                }
            }
            Response::RestoreFailed(e) => {
                self.restore_ended();
                self.message = Some(e);
                self.disk_changed();
            }
            Response::Restored {
                how,
                done,
                stopped: true,
            } => {
                let items = self.restore_ended().map_or(done.len(), |r| r.items);
                self.message = Some(match (&how, done.len()) {
                    (How::Overwrite, _) => "Stopped · nothing was overwritten".into(),
                    (_, 0) => "Stopped · nothing was restored".into(),
                    (_, n) => format!("Stopped · restored {n} of {items}"),
                });
                self.disk_changed();
            }
            Response::Restored { how, done, .. } => {
                self.restore_ended();
                let home = self.home.clone();
                let show = |p: &Path| fmt::path(p, home.as_deref());
                let base = |p: &Path| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                self.message = Some(match (&how, done.as_slice()) {
                    (How::Overwrite, [d]) => {
                        format!(
                            "Overwrote {}. :undo puts the old version back.",
                            base(&d.target)
                        )
                    }
                    (How::Overwrite, ds) => {
                        format!(
                            "Overwrote {} items. :undo puts the old versions back.",
                            ds.len()
                        )
                    }
                    (How::NextTo, [d]) if d.dest == d.target => {
                        format!("Restored {} to {}", base(&d.target), show(&d.target))
                    }
                    (How::NextTo, [d]) => format!("Restored as {}", base(&d.dest)),
                    (How::NextTo, ds) => {
                        format!("Restored {} items next to the originals", ds.len())
                    }
                    (How::RestoreDir | How::Into(_), [d]) => {
                        format!("Restored to {}", show(&d.dest))
                    }
                    (How::RestoreDir | How::Into(_), ds) => format!(
                        "Restored {} items to {}",
                        ds.len(),
                        show(ds[0].dest.parent().unwrap_or(&ds[0].dest))
                    ),
                    (How::Tar, ds) => {
                        let names: Vec<String> = ds.iter().map(|d| base(&d.dest)).collect();
                        format!("Wrote {}", names.join(", "))
                    }
                });
                self.disk_changed();
            }
            Response::Undone(paths) => {
                self.message = Some(match paths.as_slice() {
                    [p] => format!(
                        "Put back {}.",
                        p.file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default()
                    ),
                    ps => format!("Put back {} items.", ps.len()),
                });
                self.disk_changed();
            }
            Response::Exists(found) => {
                self.exists.extend(found);
                // The dialog's default depends on it.
                if let Some(d) = &mut self.dialog
                    && self.exists.get(&d.target.path) == Some(&false)
                    && d.sel == 1
                {
                    d.sel = 0;
                }
            }
            Response::Pager { name, bytes } => self.effects.push(Effect::Pager { name, bytes }),
            Response::Snapshots { snaps, quiet } => {
                let ids = |v: &[SnapshotInfo]| v.iter().map(|s| s.id).collect::<HashSet<_>>();
                if quiet && ids(&snaps) == ids(&self.everything) {
                    return;
                }
                let before = self.all.len();
                self.everything = snaps;
                let f = self.filter.clone();
                self.apply_filter(f);
                let n = self.all.len();
                self.message = Some(match n.cmp(&before) {
                    Ordering::Greater => format!("{} new snapshots.", n - before),
                    Ordering::Less => format!("{} snapshots were removed.", before - n),
                    Ordering::Equal => "No new snapshots.".into(),
                });
            }
            Response::Found {
                query,
                results,
                progress,
            } => {
                if let View::Find(f) = &mut self.view
                    && f.query == query
                {
                    f.results = results;
                    f.progress = progress;
                }
            }
            Response::Error(e) => self.message = Some(e),
        }
    }

    /// Selects an entry by name once the listing is there (`--select`).
    pub fn select_name(&mut self, name: OsString) {
        self.sel_name = Some(name);
        self.restore_selection();
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
        let before = self.sel_name.clone();
        self.sel = i.min(rows.len().saturating_sub(1));
        self.sel_name = rows
            .get(self.sel)
            .and_then(|r| self.entry(*r))
            .map(|e| e.node.name.clone());
        if self.sel_name != before {
            self.moved_on();
        }
    }

    fn go_snapshot_id(&mut self, id: SnapshotId) {
        if id != self.snap {
            self.snap = id;
            self.moved = true;
            self.restore_selection();
            self.moved_on();
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
        self.marks.clear();
        self.visual = None;
        self.name_filter.clear();
        self.restore_selection();
        self.moved_on();
    }

    fn display(&self, p: &Path) -> String {
        fmt::path(p, self.home.as_deref())
    }

    fn time(&self, i: usize) -> String {
        fmt::time(self.set()[i].time, &self.tz)
    }

    /// Whether the snapshots shown are another machine's (§3.18).
    pub fn foreign(&self) -> bool {
        !self.filter.hosts.is_empty() && self.filter.hosts.iter().any(|h| !self.mine.contains(h))
    }

    /// Whether files changed on disk since the newest snapshot of the folder.
    pub fn changed_on_disk(&self) -> bool {
        self.live.get(&self.folder).is_some_and(|c| !c.is_empty())
    }

    fn not_there(&self) -> String {
        format!(
            "{}/ did not exist then. Go up a folder and try again.",
            self.display(&self.folder)
        )
    }

    /// Does an action. Returns false if it stopped at a boundary (with a message).
    pub fn act(&mut self, a: Action) -> bool {
        match a {
            Action::Quit => {
                if self.restoring.is_some() {
                    self.confirm = Some(Confirm::QuitRestore);
                } else {
                    self.quit = true;
                }
                return true;
            }
            Action::Yank => return self.yank(),
            Action::Paste => return self.paste(false),
            Action::PasteOver => return self.paste(true),
            Action::RestoreDialog => return self.open_dialog(),
            Action::Pager => return self.pager(),
            Action::Copy(c) => return self.copy(c),
            Action::CommandLine => {
                self.start_input(InputKind::Command, "");
                return true;
            }
            Action::Find => {
                self.start_input(InputKind::Command, "find ");
                return true;
            }
            Action::ZoomIn | Action::ZoomOut => return self.zoom_by(a == Action::ZoomIn),
            Action::DialogOption(k) => {
                if let Some(d) = &mut self.dialog {
                    if d.sel == k {
                        self.dialog_enter();
                    } else {
                        d.sel = k;
                        d.confirm = false;
                    }
                }
                return true;
            }
            Action::DialogRestore => {
                self.dialog_enter();
                return true;
            }
            Action::DialogCancel => {
                self.dialog = None;
                return true;
            }
            Action::ConfirmYes => {
                match self.confirm.take() {
                    Some(Confirm::Overwrite { targets, .. }) => {
                        if !self.restore_busy() {
                            self.start_restore(targets, How::Overwrite);
                        }
                    }
                    Some(Confirm::StopRestore) => self.stop_restore(false),
                    Some(Confirm::QuitRestore) => self.stop_restore(true),
                    None => {}
                }
                return true;
            }
            Action::ConfirmNo => {
                self.confirm = None;
                return true;
            }
            _ => {}
        }
        match self.view.clone() {
            View::Versions(v) => return self.act_versions(v, a),
            View::Diff(d) => return self.act_diff(d, a),
            View::Find(f) => return self.act_find(f, a),
            View::Folder => {}
        }
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
                    let path = self.folder.join(&e.node.name);
                    if e.is_dir() {
                        if e.is_gone() {
                            let last = self.entry_snapshot(&e);
                            self.go_index(last);
                            let name = e.node.name.to_string_lossy();
                            self.message = Some(format!(
                                "Jumped to {}, the last snapshot that has {name}/",
                                self.time(last)
                            ));
                        }
                        self.go_folder(path, None);
                    } else {
                        let s = self.entry_snapshot(&e);
                        let id = self.set()[s].id;
                        self.want_track(&path, false);
                        let sel = self
                            .tracks
                            .get(&path)
                            .filter(|t| t.loaded())
                            .and_then(|t| {
                                let k = run_at(&t.runs, t.index_of(id)?)?.0;
                                Some(t.runs.len() - 1 - k)
                            })
                            .unwrap_or(0);
                        self.view = View::Versions(VersionsView { path, sel });
                        self.moved_on();
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
            Action::ClickVersion(_) | Action::Back => {}
            Action::OlderChange => {
                if !loaded {
                    return true;
                }
                match versions.iter().rev().find(|&&v| v < i) {
                    Some(&v) => self.go_index(v),
                    None => {
                        return fail(self, "This is the oldest version of this folder.".into());
                    }
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
                        let msg = if self.changed_on_disk() {
                            "Newest snapshot. Newer changes exist only on disk."
                        } else {
                            "Newest snapshot. Nothing changed on disk since."
                        };
                        return fail(self, msg.into());
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
                    let msg = self.not_there();
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
            Action::OlderItemChange | Action::NewerItemChange => {
                // By name: mid-count, the next listing may not be loaded yet.
                let Some(n) = self
                    .selected()
                    .map(|e| e.node.name.clone())
                    .or(self.sel_name.clone())
                else {
                    return fail(self, "Select a file or folder first.".into());
                };
                let path = self.folder.join(&n);
                let mut name = n.to_string_lossy().into_owned();
                if self
                    .tracks
                    .get(&path)
                    .and_then(|t| t.refs.as_ref())
                    .is_some_and(|r| r.iter().any(|r| matches!(r, NodeRef::Dir(_))))
                {
                    name.push('/');
                }
                let Some(t) = self.tracks.get(&path).filter(|t| t.loaded()) else {
                    return true;
                };
                let older = a == Action::OlderItemChange;
                let folder = self.state();
                // Snapshots of the folder's set where the item changed.
                let changed = |j: usize| {
                    t.index_of(set[j].id).is_some_and(|k| t.is_change(k))
                        && folder.is_some_and(|f| f.exists(j))
                };
                let target = if older {
                    (0..i).rev().find(|&j| changed(j))
                } else {
                    (i + 1..set.len()).find(|&j| changed(j))
                };
                match target {
                    Some(j) => self.go_index(j),
                    None => {
                        let which = if older { "older" } else { "newer" };
                        return fail(self, format!("No {which} change to {name}."));
                    }
                }
            }
            Action::GoSnapshot(j) => {
                if self.state().is_some_and(|s| s.exists(j)) {
                    self.go_index(j);
                } else {
                    let msg = self.not_there();
                    return fail(self, msg);
                }
            }
            Action::GoFolder(p) => {
                if p != self.folder {
                    self.go_folder(p, None);
                }
            }
            Action::ToggleDeleted => {
                self.ghosts = !self.ghosts;
                self.restore_selection();
                self.moved_on();
            }
            Action::Diff => {
                let Some(e) = self.selected().cloned().filter(|e| !e.is_dir()) else {
                    return fail(self, "Select a file to diff.".into());
                };
                let path = self.folder.join(&e.node.name);
                let id = self.set()[self.entry_snapshot(&e)].id;
                self.want_track(&path, false);
                let Some(t) = self.tracks.get(&path).filter(|t| t.loaded()) else {
                    return true;
                };
                let run = t
                    .index_of(id)
                    .and_then(|s| run_at(&t.runs, s))
                    .map_or(0, |(k, _)| t.runs.len() - 1 - k);
                return self.open_diff(path, run, DiffMode::Disk, false);
            }
            Action::DiffPrevious | Action::NextHunk | Action::PrevHunk => {}
            Action::ToggleMark => self.toggle_mark(),
            Action::Visual => self.toggle_visual(),
            Action::Escape => {
                if self.visual.take().is_some() {
                } else if !self.marks.is_empty() {
                    self.marks.clear();
                } else if self.search.is_empty()
                    && self.name_filter.is_empty()
                    && self.restoring.as_ref().is_some_and(|r| !r.stopping)
                {
                    self.confirm = Some(Confirm::StopRestore);
                } else {
                    self.search.clear();
                    self.name_filter.clear();
                    self.restore_selection();
                }
            }
            Action::Search => self.start_input(InputKind::Search, ""),
            Action::FilterInput => {
                let f = self.name_filter.clone();
                self.start_input(InputKind::Filter, &f);
            }
            Action::NextMatch => return self.search_step(true),
            Action::PrevMatch => return self.search_step(false),
            Action::ClickFound(_) => {}
            Action::Yank
            | Action::Paste
            | Action::PasteOver
            | Action::RestoreDialog
            | Action::Pager
            | Action::Copy(_)
            | Action::CommandLine
            | Action::DialogOption(_)
            | Action::DialogRestore
            | Action::DialogCancel
            | Action::ConfirmYes
            | Action::ConfirmNo
            | Action::Find
            | Action::ZoomIn
            | Action::ZoomOut => unreachable!("handled above"),
            Action::TogglePreview => self.toggle_preview(),
            Action::Scroll(n) => self.scroll_by(n),
            Action::Help => self.help = true,
            Action::Prefix(c) => self.prefix = Some(c),
            Action::Quit => self.quit = true,
        }
        true
    }

    fn toggle_preview(&mut self) {
        self.preview = match self.preview {
            PreviewMode::Content => PreviewMode::Disk,
            PreviewMode::Disk => PreviewMode::Content,
        };
        self.moved_on();
    }

    fn scroll_by(&mut self, n: isize) {
        let s = self.scroll.unwrap_or(self.scroll_base.get()) as isize + n;
        self.scroll = Some(s.max(0) as usize);
    }

    fn act_versions(&mut self, v: VersionsView, a: Action) -> bool {
        let runs = self.tracks.get(&v.path).map_or(0, |t| t.runs.len());
        let fail = |app: &mut App, msg: &str| {
            app.message = Some(msg.into());
            false
        };
        let mut sel = v.sel;
        match a {
            Action::Down(n) => {
                if sel + 1 >= runs {
                    return fail(self, "This is the oldest version.");
                }
                sel = (sel + n).min(runs - 1);
            }
            Action::OlderChange | Action::OlderItemChange => {
                if sel + 1 >= runs {
                    return fail(self, "This is the oldest version.");
                }
                sel += 1;
            }
            Action::Up(n) => {
                if sel == 0 {
                    return fail(self, "This is the newest version.");
                }
                sel = sel.saturating_sub(n);
            }
            Action::NewerChange | Action::NewerItemChange => {
                if sel == 0 {
                    return fail(self, "This is the newest version.");
                }
                sel -= 1;
            }
            Action::Top => sel = 0,
            Action::Bottom => sel = runs.saturating_sub(1),
            Action::HalfDown => sel = (sel + 10).min(runs.saturating_sub(1)),
            Action::HalfUp => sel = sel.saturating_sub(10),
            Action::ClickVersion(k) => {
                if k == sel {
                    return self.open_diff(v.path, sel, DiffMode::Disk, true);
                }
                sel = k.min(runs.saturating_sub(1));
            }
            Action::GoSnapshot(j) => {
                // A click on the timeline selects the version holding it.
                if let Some(t) = self.tracks.get(&v.path)
                    && let Some((k, _)) = run_at(&t.runs, j)
                {
                    sel = t.runs.len() - 1 - k;
                }
            }
            Action::Open | Action::Diff => {
                return self.open_diff(v.path, sel, DiffMode::Disk, true);
            }
            Action::DiffPrevious => {
                return self.open_diff(v.path, sel, DiffMode::Previous, true);
            }
            Action::Back | Action::Parent | Action::Escape => {
                self.view = View::Folder;
                self.moved_on();
                return true;
            }
            Action::TogglePreview => self.toggle_preview(),
            Action::Scroll(n) => self.scroll_by(n),
            Action::Help => self.help = true,
            Action::Prefix(c) => self.prefix = Some(c),
            _ => {}
        }
        if sel != v.sel {
            self.view = View::Versions(VersionsView { sel, ..v });
            self.moved_on();
        }
        true
    }

    /// Lines in the diff being shown, and where its hunks start.
    fn diff_lines(&self, d: &DiffView) -> Option<(usize, Vec<usize>)> {
        let (key, ..) = self.diff_sides(d)?;
        match self.diffs.get(&key)?.as_ref() {
            FileDiff::Text { lines, .. } => Some((
                lines.len(),
                lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| matches!(l, HunkLine::Header(_)))
                    .map(|(i, _)| i)
                    .collect(),
            )),
            _ => Some((0, Vec::new())),
        }
    }

    fn act_diff(&mut self, d: DiffView, a: Action) -> bool {
        let (len, headers) = self.diff_lines(&d).unwrap_or_default();
        let page = self.page.max(2);
        let max = len.saturating_sub(page);
        let runs: Vec<Run> = self
            .tracks
            .get(&d.path)
            .map(|t| t.runs.iter().rev().copied().collect())
            .unwrap_or_default();
        let fail = |app: &mut App, msg: &str| {
            app.message = Some(msg.into());
            false
        };
        let mut next = d.clone();
        match a {
            Action::Down(n) => next.scroll = (d.scroll + n).min(max),
            Action::Up(n) => next.scroll = d.scroll.saturating_sub(n),
            Action::HalfDown => next.scroll = (d.scroll + 15).min(max),
            Action::HalfUp => next.scroll = d.scroll.saturating_sub(15),
            Action::Top => next.scroll = 0,
            Action::Bottom => next.scroll = max,
            // A hunk that's already on screen below can't be scrolled to.
            Action::NextHunk => match headers.iter().find(|&&h| h > d.scroll && d.scroll < max) {
                Some(&h) => next.scroll = h.min(max),
                None => return fail(self, "No more changes below."),
            },
            Action::PrevHunk => match headers.iter().rev().find(|&&h| h < d.scroll) {
                Some(&h) => next.scroll = h,
                None => return fail(self, "No more changes above."),
            },
            Action::OlderChange | Action::OlderItemChange => {
                match (d.run + 1..runs.len()).find(|&k| runs[k].exists) {
                    Some(k) => {
                        next.run = k;
                        next.scroll = 0;
                    }
                    None => return fail(self, "This is the oldest version."),
                }
            }
            Action::NewerChange | Action::NewerItemChange => {
                match (0..d.run).rev().find(|&k| runs[k].exists) {
                    Some(k) => {
                        next.run = k;
                        next.scroll = 0;
                    }
                    None => return fail(self, "This is the newest saved version."),
                }
            }
            Action::Diff => {
                next.mode = DiffMode::Disk;
                next.scroll = 0;
            }
            Action::DiffPrevious => {
                next.mode = DiffMode::Previous;
                next.scroll = 0;
            }
            Action::Open => next.force = true,
            Action::Back | Action::Parent | Action::Escape => {
                self.view = if d.from_versions {
                    View::Versions(VersionsView {
                        path: d.path,
                        sel: d.run,
                    })
                } else {
                    View::Folder
                };
                self.moved_on();
                return true;
            }
            Action::Help => self.help = true,
            Action::Prefix(c) => self.prefix = Some(c),
            _ => {}
        }
        if next != d {
            let reload = next.run != d.run || next.mode != d.mode || next.force != d.force;
            self.view = View::Diff(next);
            if reload {
                self.moved_on();
            }
        }
        true
    }

    /// Whether a row matches the search.
    pub fn matches(&self, row: Row) -> bool {
        !self.search.is_empty()
            && self.entry(row).is_some_and(|e| {
                e.node
                    .name
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&self.search.to_lowercase())
            })
    }

    fn select_row(&mut self, k: usize) {
        self.select(k);
    }

    /// The first entry (below `..`).
    fn select_first(&mut self) {
        self.select(usize::from(self.folder != self.root));
    }

    /// `:host`, `:tag`: a new filter, unless it leaves this folder with no snapshots.
    fn set_filter(&mut self, f: Filter) {
        let mine: Vec<SnapshotInfo> = self
            .everything
            .iter()
            .filter(|s| f.matches(s))
            .cloned()
            .collect();
        if !mine.iter().any(|s| covers(s, &self.folder)) {
            self.message = Some(explain_empty(&self.everything, &f, &self.folder));
            return;
        }
        self.apply_filter(f);
        let hosts = if self.filter.hosts.is_empty() {
            "any host".to_string()
        } else {
            self.filter.hosts.join(", ")
        };
        let tag = self
            .filter
            .tag
            .as_ref()
            .map(|t| format!(", tag {t}"))
            .unwrap_or_default();
        self.message = Some(format!("Showing snapshots from {hosts}{tag}."));
    }

    /// Recomputes everything that depends on which snapshots are shown.
    fn apply_filter(&mut self, f: Filter) {
        let time = self.set().get(self.idx()).map(|s| s.time);
        self.filter = f;
        self.all = self
            .everything
            .iter()
            .filter(|s| self.filter.matches(s))
            .cloned()
            .collect();
        self.forget_history();
        let set = self.set_for(&self.folder);
        if !set.iter().any(|s| s.id == self.snap)
            && let Some(s) = time
                .and_then(|t| set.iter().rev().find(|s| s.time <= t))
                .or(set.last())
        {
            self.snap = s.id;
        }
        self.moved_on();
    }

    /// Forgets what was computed over the snapshots, to compute it again.
    fn forget_history(&mut self) {
        self.tracks.clear();
        self.listings.clear();
        self.deleted.clear();
        self.nodes.clear();
        self.pending.clear();
    }

    /// `:set strict` / `:set nostrict`.
    fn set_mode(&mut self, mode: Mode) {
        self.strict = mode == Mode::Strict;
        if let Some(m) = &self.mode_switch {
            m.set(mode);
        }
        self.forget_history();
        self.moved_on();
        self.message = Some(
            if self.strict {
                "Counting every change restic stored (strict)."
            } else {
                "Counting changes to content, permissions and owner."
            }
            .into(),
        );
    }

    /// The zoom levels: 1×, 2×, 4×, … until no two snapshots share a column.
    fn max_zoom(&self) -> u32 {
        let set = self.set();
        let secs: Vec<i64> = set.iter().map(|s| s.time.as_second()).collect();
        let span = secs.last().zip(secs.first()).map_or(0, |(b, a)| b - a);
        let gap = secs
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|&g| g > 0)
            .min();
        let width = 74i64;
        let need = match gap {
            Some(g) if span > 0 => (span / (g * width)).max(1),
            _ => 1,
        };
        (need as u32).next_power_of_two().clamp(8, 4096)
    }

    fn zoom_by(&mut self, zoom_in: bool) -> bool {
        let next = if zoom_in {
            self.zoom * 2
        } else {
            self.zoom / 2
        };
        if next < 1 || next > self.max_zoom() {
            self.message = Some(
                if zoom_in {
                    "Fully zoomed in."
                } else {
                    "Fully zoomed out."
                }
                .into(),
            );
            return false;
        }
        self.zoom = next;
        true
    }

    /// `:find NAME`: search every snapshot from the backup root.
    pub(super) fn open_find(&mut self, query: &str) {
        let root = self.root.clone();
        let set = self.set_for(&root);
        let cancel = Cancel::default();
        self.outbox.push(Request::Find {
            set: set.clone(),
            root: root.clone(),
            query: query.to_string(),
            cancel: cancel.clone(),
        });
        self.view = View::Find(FindView {
            query: query.to_string(),
            root,
            set,
            results: Vec::new(),
            progress: Some((0, 0)),
            sel: 0,
            cancel,
        });
    }

    fn act_find(&mut self, f: FindView, a: Action) -> bool {
        let n = f.results.len();
        let mut sel = f.sel;
        match a {
            Action::Down(k) => sel = (sel + k).min(n.saturating_sub(1)),
            Action::Up(k) => sel = sel.saturating_sub(k),
            Action::Top => sel = 0,
            Action::Bottom => sel = n.saturating_sub(1),
            Action::HalfDown => sel = (sel + 10).min(n.saturating_sub(1)),
            Action::HalfUp => sel = sel.saturating_sub(10),
            Action::ClickFound(k) if k != sel => sel = k.min(n.saturating_sub(1)),
            Action::Open | Action::ClickFound(_) => {
                if let Some((found, _)) = f.results.get(sel).cloned() {
                    f.cancel.cancel();
                    self.go_found(&f, &found);
                }
                return true;
            }
            Action::Back | Action::Parent | Action::Escape => {
                f.cancel.cancel();
                self.view = View::Folder;
                self.moved_on();
                return true;
            }
            Action::Help => self.help = true,
            Action::Prefix(c) => self.prefix = Some(c),
            _ => {}
        }
        self.view = View::Find(FindView { sel, ..f });
        true
    }

    /// Opens a find result's folder at the last snapshot that had it, with it selected.
    fn go_found(&mut self, f: &FindView, found: &Found) {
        let path = f.root.join(&found.path);
        let parent = path.parent().unwrap_or(&f.root).to_path_buf();
        let snap = f.set[found.last].clone();
        self.view = View::Folder;
        self.snap = snap.id;
        self.moved = true;
        let name = path.file_name().map(|n| n.to_os_string());
        self.go_folder(parent, name.clone());
        let mut shown = name
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if found.is_dir {
            shown.push('/');
        }
        self.message = Some(if found.last + 1 == f.set.len() {
            format!("Showing {shown} in the latest snapshot.")
        } else {
            format!(
                "Jumped to {}, the last snapshot that has {shown}",
                fmt::time(snap.time, &self.tz)
            )
        });
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
