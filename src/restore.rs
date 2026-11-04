//! Restoring: next to the original, over it (with an
//! undo log), into the restore folder, or as a tar archive. Writes only to
//! the destination it picks, and to the undo folder. Never to the repository.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::index::folder::Counts;
use crate::repo::{
    Node, NodeKind, Repo, RestoreStep, SnapshotId, SnapshotInfo, Stopped, is_stopped,
};
use crate::worker::Cancel;

/// Something to restore: what was at `path` in `snapshot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub snapshot: SnapshotInfo,
    /// The absolute path, in the snapshot and on disk.
    pub path: PathBuf,
    pub node: Node,
}

/// Which targets a [`Plan`] is for: their snapshots and paths.
pub type PlanFor = Vec<(SnapshotId, PathBuf)>;

/// What an overwrite would change, worked out before it's confirmed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Disk against the versions restored, by size and time as the on
    /// disk version compares: `added` is only on disk and goes to `:undo`,
    /// `deleted` is only in the versions and comes back.
    pub differs: Counts,
    /// What the restore writes.
    pub files: u64,
    pub bytes: u64,
}

impl Plan {
    /// `On disk: 3 changed, 2 missing come back, 1 not in that version
    /// goes to :undo. Writes 4812 files, 1.2G.`
    pub fn describe(&self) -> String {
        let c = self.differs;
        let mut parts = Vec::new();
        if c.changed > 0 {
            parts.push(format!("{} changed", c.changed));
        }
        if c.deleted > 0 {
            parts.push(format!("{} missing come back", c.deleted));
        }
        if c.added > 0 {
            parts.push(format!("{} not in that version go to :undo", c.added));
        }
        let differs = if parts.is_empty() {
            "Nothing on disk differs from it, by size and time.".to_string()
        } else {
            format!("On disk: {}.", parts.join(", "))
        };
        let s = if self.files == 1 { "" } else { "s" };
        let size = crate::ui::fmt::size(self.bytes);
        format!("{differs} Writes {} file{s}, {size}.", self.files)
    }
}

impl Target {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum How {
    /// Replace what's on disk (or put it back where it was).
    Overwrite,
    /// `name.2026-09-09_1923` next to the original, or the original place
    /// if nothing is there.
    NextTo,
    /// `<restore dir>/2026-09-09_1923/name`
    RestoreDir,
    /// A folder as `name-2026-09-09_1923.tar` next to the original.
    Tar,
    /// `<dir>/name`: a directory the user named (restores from another
    /// host).
    Into(PathBuf),
}

/// Where restores go and where undo data is kept.
#[derive(Clone, Debug)]
pub struct Places {
    /// `~/Restored`; `None` without a home folder to put it in (and no
    /// `restore_dir` in the config), and then nothing is restored there.
    pub restore_dir: Option<PathBuf>,
    /// `~/.local/share/restoric/undo`; `None` without a home folder or
    /// `XDG_DATA_HOME`, and then nothing is overwritten. Never the shared
    /// temporary folder: it would hold the user's overwritten files.
    pub undo_dir: Option<PathBuf>,
    pub tz: TimeZone,
}

/// What gives a restore folder when there is none: shown in the restore
/// dialog in place of the destination, and in the error of a restore
/// into it.
pub const NO_RESTORE_DIR: &str = "set HOME, or restore_dir in config";

impl Places {
    pub fn default_for(tz: TimeZone) -> Self {
        Places {
            restore_dir: crate::dirs::home().map(|h| h.join("Restored")),
            undo_dir: crate::dirs::data().map(|d| d.join("undo")),
            tz,
        }
    }
}

/// `2026-09-09_1923`, the snapshot time in local time.
pub fn stamp(t: Timestamp, tz: &TimeZone) -> String {
    t.to_zoned(tz.clone()).strftime("%Y-%m-%d_%H%M").to_string()
}

fn exists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

/// `base`, or `base-2`, `base-3`, … whichever doesn't exist yet. `ext` goes
/// after the number (`name-2026-09-09_1923-2.tar`).
fn free(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{base}{ext}"));
    if !exists(&first) {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{base}-{n}{ext}")))
        .find(|p| !exists(p))
        .expect("some name is free")
}

/// Where a restore of `t` would go if something is on disk at its path,
/// without the `-2` a clash adds. Doesn't look at the disk. `None` for
/// the restore folder when there is none.
pub fn planned(t: &Target, how: &How, places: &Places) -> Option<PathBuf> {
    let st = stamp(t.snapshot.time, &places.tz);
    let parent = t.path.parent().unwrap_or(Path::new("/"));
    let name = t.name();
    Some(match how {
        How::Overwrite => t.path.clone(),
        How::NextTo => parent.join(format!("{name}.{st}")),
        How::RestoreDir => places.restore_dir.as_ref()?.join(&st).join(&name),
        How::Tar => parent.join(format!("{name}-{st}.tar")),
        How::Into(dir) => dir.join(&name),
    })
}

/// What a restore did, for the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub target: PathBuf,
    pub dest: PathBuf,
}

/// How a restore ended. A stopped overwrite has put everything back, so
/// `done` is empty; stopped copies keep the items that were finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub done: Vec<Done>,
    pub stopped: bool,
    /// Old copies a stopped overwrite put back but couldn't remove from
    /// the undo step afterwards, with why (see [`Undone::left`]).
    pub left: Vec<(PathBuf, String)>,
}

impl Outcome {
    fn finished(done: Vec<Done>) -> Self {
        Outcome {
            done,
            stopped: false,
            left: Vec::new(),
        }
    }

    fn stopped(done: Vec<Done>) -> Self {
        Outcome {
            done,
            stopped: true,
            left: Vec::new(),
        }
    }
}

/// How far a restore has got, for the status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Which item, from 0, of `items`.
    pub item: usize,
    pub items: usize,
    /// `main.go` or `src/`.
    pub name: String,
    pub step: RestoreStep,
}

/// Passes progress on, at most every 100 ms within one item and step.
struct Reporter<'a> {
    out: &'a mut dyn FnMut(Progress),
    items: usize,
    last: Option<(usize, bool, Instant)>,
}

impl Reporter<'_> {
    fn send(&mut self, item: usize, t: &Target, step: RestoreStep) {
        let copying = matches!(step, RestoreStep::Bytes { .. });
        let finished = matches!(step, RestoreStep::Bytes { done, total } if done >= total);
        let now = Instant::now();
        if let Some((i, c, at)) = self.last
            && i == item
            && c == copying
            && !finished
            && now.duration_since(at) < Duration::from_millis(100)
        {
            return;
        }
        self.last = Some((item, copying, now));
        let mut name = t.name();
        if t.node.kind == NodeKind::Dir {
            name.push('/');
        }
        (self.out)(Progress {
            item,
            items: self.items,
            name,
            step,
        });
    }
}

/// Restores every target; an overwrite of several items is one undo step.
/// `cancel` stops it between items, or before an item's contents are
/// copied (a tar archive stops anywhere).
pub fn run(
    repo: &dyn Repo,
    targets: &[Target],
    how: &How,
    places: &Places,
    progress: &mut dyn FnMut(Progress),
    cancel: &Cancel,
) -> Result<Outcome> {
    let mut rep = Reporter {
        out: progress,
        items: targets.len(),
        last: None,
    };
    if *how == How::Overwrite {
        return overwrite(repo, targets, places, &mut rep, cancel);
    }
    if *how == How::RestoreDir && places.restore_dir.is_none() {
        bail!("no restore folder: {NO_RESTORE_DIR}");
    }
    let mut done = Vec::new();
    for (i, t) in targets.iter().enumerate() {
        if cancel.cancelled() {
            return Ok(Outcome::stopped(done));
        }
        let mut report = |step| rep.send(i, t, step);
        let st = stamp(t.snapshot.time, &places.tz);
        let name = t.name();
        let parent = t.path.parent().context("can't restore /")?;
        let dest = match how {
            How::NextTo if !exists(&t.path) => {
                fs::create_dir_all(parent)?;
                t.path.clone()
            }
            How::NextTo => free(parent, &format!("{name}.{st}"), ""),
            How::RestoreDir => {
                let base = places.restore_dir.as_deref().expect("checked above");
                let dir = base.join(&st);
                fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
                free(&dir, &name, "")
            }
            How::Tar => free(parent, &format!("{name}-{st}"), ".tar"),
            How::Into(dir) => {
                fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
                free(dir, &name, "")
            }
            How::Overwrite => unreachable!(),
        };
        let res = if *how == How::Tar {
            write_tar(repo, t, &dest, &mut report, cancel)
        } else {
            match crate::repo::claim(&t.node, &dest) {
                Ok(()) => repo.restore(&t.snapshot, &t.path, &dest, &mut report, &|| {
                    cancel.cancelled()
                }),
                // Nothing of ours is at `dest`: whatever is there now was
                // put there by something else, and stays.
                Err(e) => {
                    let e = e.context(format!("restoring {}", t.path.display()));
                    return Err(with_earlier(e, done.len(), how));
                }
            }
        };
        // A tar archive removes itself when it fails (and nothing is at
        // `dest` if it couldn't be created). Otherwise what's there is the
        // claimed name, or the partial copy: nothing was there before.
        let clean = |dest: &Path| {
            if *how != How::Tar {
                let _ = unclaim(&Claimed::of(&t.node), dest);
            }
        };
        match res {
            Ok(()) => done.push(Done {
                target: t.path.clone(),
                dest,
            }),
            Err(e) if is_stopped(&e) => {
                clean(&dest);
                return Ok(Outcome::stopped(done));
            }
            Err(e) => {
                clean(&dest);
                let e = e.context(format!("restoring {}", t.path.display()));
                return Err(with_earlier(e, done.len(), how));
            }
        }
    }
    Ok(Outcome::finished(done))
}

/// One undo step: what an overwrite replaced.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    entries: Vec<UndoEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct UndoEntry {
    /// The path that was overwritten (or created).
    path: PathBuf,
    /// Where the old version is kept; `None` if nothing was there.
    saved: Option<PathBuf>,
    /// What the restore has claimed the name for, when nothing was there:
    /// set once it has, so that undo removes just that (see [`unclaim`]).
    /// Until then (the process died first) whatever is at the path was
    /// put there by something else, or is at most the claimed empty name,
    /// and stays. With a copy, the copy's being there is what says this.
    #[serde(default)]
    claimed: Option<Claimed>,
}

/// What a restore claimed a free name for, and so what's its to remove
/// there (see [`unclaim`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Claimed {
    File,
    Dir,
    /// A link to this target.
    Link(PathBuf),
    /// Something restoric doesn't restore: the name was only found free.
    Nothing,
}

impl Claimed {
    fn of(node: &Node) -> Self {
        match &node.kind {
            NodeKind::File => Claimed::File,
            NodeKind::Dir => Claimed::Dir,
            NodeKind::Symlink { target } => Claimed::Link(PathBuf::from(target)),
            NodeKind::Other(_) => Claimed::Nothing,
        }
    }
}

const MANIFEST: &str = "manifest.json";

/// Moves what's on disk into a new undo folder, then restores over it.
/// If a restore fails, what was moved away is put back; if that fails too,
/// the step keeps it for undo. If it's stopped, every item of this step is
/// put back.
///
/// Each entry is in the manifest before its original is moved, and the
/// copy is at its recorded place only once it's whole (see [`move_aside`]);
/// an entry for a free name is marked claimed only once the name is:
/// whatever the process dies in the middle of, [`rollback`] either finds a
/// whole copy to put back, something of the restore's to remove, or
/// leaves the path alone.
fn overwrite(
    repo: &dyn Repo,
    targets: &[Target],
    places: &Places,
    rep: &mut Reporter,
    cancel: &Cancel,
) -> Result<Outcome> {
    let stopped = |session: &Path, manifest: &Manifest| -> Result<Outcome> {
        let undone = rollback(session, manifest)?;
        // Everything is back: a stop, whatever copies stay.
        Ok(Outcome {
            done: Vec::new(),
            stopped: true,
            left: undone.left,
        })
    };
    if targets.is_empty() {
        return Ok(Outcome::finished(Vec::new()));
    }
    let mut done = Vec::new();
    let session = new_session(places.undo_dir.as_deref())?;
    let mut manifest = Manifest::default();
    for (i, t) in targets.iter().enumerate() {
        if cancel.cancelled() {
            return stopped(&session, &manifest);
        }
        let saved = exists(&t.path).then(|| saved_at(&session, i, &t.name()));
        manifest.entries.push(UndoEntry {
            path: t.path.clone(),
            saved: saved.clone(),
            claimed: None,
        });
        if let Err(e) = write_manifest(&session, &manifest) {
            // Nothing has been touched for this entry, and the manifest on
            // disk is still the one without it.
            manifest.entries.pop();
            if manifest.entries.is_empty() {
                remove_session(&session);
            }
            let e = e.context(format!("restoring {}", t.path.display()));
            return Err(with_earlier(e, done.len(), &How::Overwrite));
        }
        let moved = match &saved {
            Some(to) => fs::create_dir_all(to.parent().expect("has a parent"))
                .and_then(|()| move_aside(&t.path, to))
                .with_context(|| format!("moving {} aside", t.path.display())),
            None => match t.path.parent() {
                Some(p) => fs::create_dir_all(p).map_err(Into::into),
                None => Ok(()),
            },
        };
        let claim = Claimed::of(&t.node);
        let mut claimed = false;
        let res = moved.and_then(|()| {
            crate::repo::claim(&t.node, &t.path)?;
            claimed = true;
            if saved.is_none() {
                // From here on what's at the path is ours.
                manifest.entries.last_mut().expect("pushed above").claimed = Some(claim.clone());
                write_manifest(&session, &manifest)?;
            }
            let mut report = |step| rep.send(i, t, step);
            repo.restore(&t.snapshot, &t.path, &t.path, &mut report, &|| {
                cancel.cancelled()
            })
        });
        if let Err(e) = res {
            if is_stopped(&e) {
                return stopped(&session, &manifest);
            }
            let e = e.context(format!("restoring {}", t.path.display()));
            let e = match &saved {
                // Nothing of the user's was at the path. What's there now
                // is ours if the name was claimed (the claimed name, or a
                // partial copy); otherwise something else put it there,
                // and it stays. Either way there's nothing to put back.
                None => {
                    if claimed {
                        let _ = unclaim(&claim, &t.path);
                    }
                    match drop_last(&session, &mut manifest) {
                        Ok(()) => e,
                        Err(d) => anyhow::anyhow!(
                            "{e:#}; and the undo step still lists {}: {d:#}",
                            t.path.display()
                        ),
                    }
                }
                // Put the old version back. The entry's copy is whole if
                // it's there, so this is safe whichever step failed.
                Some(_) => {
                    let mut left = Vec::new();
                    match put_back_last(&session, &mut manifest, &mut left) {
                        Ok(_) => match left.first() {
                            None => e,
                            Some((copy, why)) => anyhow::anyhow!(
                                "{e:#}; the old version is back, but its copy couldn't be removed and stays at {}: {why}",
                                copy.display()
                            ),
                        },
                        Err(back) => anyhow::anyhow!(
                            "{e:#}; the old {} is kept for undo, but putting it back failed: {back:#}",
                            t.path.display()
                        ),
                    }
                }
            };
            return Err(with_earlier(e, done.len(), &How::Overwrite));
        }
        done.push(Done {
            target: t.path.clone(),
            dest: t.path.clone(),
        });
    }
    // Stopped while the last item was being copied: still all or nothing.
    if cancel.cancelled() {
        return stopped(&session, &manifest);
    }
    Ok(Outcome::finished(done))
}

/// `e`, and that the `n` items before the failed one were restored and
/// stay so; overwritten ones as the undo step, which :undo puts back.
fn with_earlier(e: anyhow::Error, n: usize, how: &How) -> anyhow::Error {
    let (before, s, stay) = match n {
        0 => return e,
        1 => ("the item before it was".to_string(), "", "stays"),
        n => (format!("the {n} items before it were"), "s", "stay"),
    };
    let then = match how {
        How::Overwrite => format!("overwritten, and :undo puts the old version{s} back"),
        How::Tar => format!("written and {stay}"),
        _ => format!("restored and {stay}"),
    };
    anyhow::anyhow!("{e:#}; {before} {then}")
}

/// `<session>/files/<i>/<name>`: where item `i` of a step keeps what it
/// replaced. Built from the item's number and name only; joining the
/// original's absolute path would, on Windows, give that path itself.
fn saved_at(session: &Path, i: usize, name: &str) -> PathBuf {
    session.join("files").join(i.to_string()).join(name)
}

/// The step an undo copy (an [`Undone::left`] path) is kept in.
pub fn step_of(copy: &Path) -> &Path {
    copy.ancestors().nth(3).unwrap_or(copy)
}

fn new_session(undo_dir: Option<&Path>) -> Result<PathBuf> {
    let undo_dir = undo_dir
        .context("no folder to keep what an overwrite replaces: set HOME or XDG_DATA_HOME")?;
    crate::dirs::create_private(undo_dir)
        .with_context(|| format!("creating {}", undo_dir.display()))?;
    let now = Timestamp::now().strftime("%Y%m%dT%H%M%S%.f").to_string();
    let session = free(undo_dir, &now, "");
    fs::create_dir(&session)?;
    // With a manifest from the start: a step the process dies in before
    // its first entry is then one [`undo`] finds empty and removes.
    if let Err(e) = write_manifest(&session, &Manifest::default()) {
        remove_session(&session);
        return Err(e);
    }
    Ok(session)
}

/// Writes the manifest whole or not at all: to a temporary name, then
/// renamed over the old one, so a crash or a full disk mid-write leaves
/// the previous manifest, never a truncated one that undo can't read.
fn write_manifest(session: &Path, m: &Manifest) -> Result<()> {
    let path = session.join(MANIFEST);
    let tmp = session.join(format!("{MANIFEST}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(m)?)
        .and_then(|()| fs::rename(&tmp, &path))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// What an undo did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Undone {
    /// The paths put back.
    pub back: Vec<PathBuf>,
    /// Old copies that were put back but couldn't be removed from the
    /// undo step afterwards, with why. They stay there, and the step with
    /// them, until something removes them.
    pub left: Vec<(PathBuf, String)>,
}

/// Undoes the last overwrite.
///
/// A step with nothing left in it (put back, but its folder couldn't be
/// removed then) is removed if it can be, and never counts as the last
/// overwrite. A step whose manifest can't be read stops the undo, named:
/// it isn't skipped, since its files folder may hold what that overwrite
/// replaced, and the steps before it go back in order once it's moved
/// away.
pub fn undo(places: &Places) -> Result<Undone> {
    let read = places.undo_dir.as_deref().map(fs::read_dir);
    let mut sessions: Vec<PathBuf> = match read {
        Some(Ok(rd)) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join(MANIFEST).is_file())
            .collect(),
        _ => Vec::new(),
    };
    sessions.sort();
    while let Some(session) = sessions.pop() {
        let manifest = session.join(MANIFEST);
        let m: Manifest = match fs::read(&manifest)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice(&bytes)?))
        {
            Ok(m) => m,
            Err(e) => bail!(
                "can't read the undo step {}: {e:#}. Its files folder keeps what that overwrite replaced; move the step away to undo earlier ones",
                session.display()
            ),
        };
        if m.entries.is_empty() {
            remove_session(&session);
            continue;
        }
        return rollback(&session, &m);
    }
    bail!("Nothing to undo.");
}

/// Puts back everything an undo step replaced, then removes the step.
///
/// Can be run again after failing half way: each entry leaves the manifest
/// as it's put back, and an entry whose copy is gone (put back already, or
/// never made), or whose name the restore never claimed, leaves its path
/// alone rather than deleting what's there.
fn rollback(session: &Path, m: &Manifest) -> Result<Undone> {
    if m.entries.is_empty() {
        remove_session(session);
        return Ok(Undone::default());
    }
    let mut todo = m.clone();
    let mut undone = Undone::default();
    while !todo.entries.is_empty() {
        // The last one removes the step.
        undone
            .back
            .extend(put_back_last(session, &mut todo, &mut undone.left)?);
    }
    undone.back.reverse();
    Ok(undone)
}

/// Removes a step nothing is left in. Everything is back by then; what
/// can stop this is a copy that couldn't be removed, which stays as a
/// leftover the next undo tries again.
fn remove_session(session: &Path) {
    if let Err(e) = fs::remove_dir_all(session) {
        tracing::warn!("leaving the undo step {}: {e}", session.display());
    }
}

/// Puts back the last entry of `todo`, drops it from the manifest, and
/// removes the step once it's empty. Returns the path if something was put
/// back (or removed): an entry whose copy is gone (never made, or put back
/// already), or whose free name the restore never claimed, or holds
/// something other than what the restore made, leaves its path alone.
/// Fails with the entry still in the manifest, so it can be run again.
///
/// The copy is removed only after the manifest no longer names it, so a
/// copy that was put back across file systems and then couldn't be
/// removed is a leftover, never something a later undo would put back
/// over the user's files. Such a copy is added to `left`, with why.
fn put_back_last(
    session: &Path,
    todo: &mut Manifest,
    left: &mut Vec<(PathBuf, String)>,
) -> Result<Option<PathBuf>> {
    let Some(e) = todo.entries.last() else {
        return Ok(None);
    };
    let path = e.path.clone();
    let saved = e.saved.clone();
    let back = match (&saved, &e.claimed) {
        (Some(s), _) if !exists(s) => false,
        (Some(s), _) => {
            remove_path(&path).with_context(|| format!("removing {}", path.display()))?;
            place(s, &path).with_context(|| format!("putting back {}", path.display()))?;
            true
        }
        // Removing what the restore made counts only if it was still there.
        (None, Some(c)) => {
            unclaim(c, &path).with_context(|| format!("removing {}", path.display()))?
        }
        (None, None) => false,
    };
    drop_last(session, todo)?;
    if let Some(s) = &saved
        && let Err(e) = remove_path(s)
    {
        tracing::warn!("leaving the undo copy {}: {e}", s.display());
        left.push((s.clone(), e.to_string()));
    }
    Ok(back.then_some(path))
}

/// Drops the last entry of `m` from the manifest on disk, and the step
/// with it once it's empty. Fails with the entry still in `m`.
fn drop_last(session: &Path, m: &mut Manifest) -> Result<()> {
    let Some(entry) = m.entries.pop() else {
        return Ok(());
    };
    if let Err(e) = write_manifest(session, m) {
        m.entries.push(entry);
        return Err(e);
    }
    if m.entries.is_empty() {
        remove_session(session);
    }
    Ok(())
}

/// Removes what [`claim`](crate::repo::claim) and a restore of what the
/// name was claimed for left at `dest`, and nothing else: the claimed file
/// or folder (with whatever the restore put in it), or a link to the
/// target. Anything else at the name was put there by something else, and
/// stays. Says whether something was removed.
fn unclaim(c: &Claimed, dest: &Path) -> io::Result<bool> {
    match c {
        Claimed::File | Claimed::Dir => remove_path(dest),
        Claimed::Link(target) => match fs::read_link(dest) {
            Ok(t) if t == *target => fs::remove_file(dest).map(|()| true),
            _ => Ok(false),
        },
        Claimed::Nothing => Ok(false),
    }
}

/// Removes a file, link or folder; never follows symlinks. Says whether
/// there was one.
fn remove_path(p: &Path) -> io::Result<bool> {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p)?,
        Ok(_) => fs::remove_file(p)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    Ok(true)
}

/// Moves `from` to `to`, a name nothing is at in a folder of the undo
/// step: a rename, or across file systems [`copy_aside`]. Either way, `to`
/// holds a copy only once it's whole.
fn move_aside(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_aside(from, to),
        Err(e) => Err(e),
    }
}

/// Copies `from` to `<to>.part`, renames that to `to` once it's whole,
/// then removes `from`. A copy that fails is removed and the source is
/// untouched; removing the source after a whole copy can fail part way,
/// and then the copy at `to` is what's whole.
fn copy_aside(from: &Path, to: &Path) -> io::Result<()> {
    let mut part = to.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    copy_fresh(from, &part)?;
    if let Err(e) = fs::rename(&part, to) {
        let _ = remove_path(&part);
        return Err(e);
    }
    remove_path(from)?;
    Ok(())
}

/// Puts `from` at `to`, a name nothing is at: a rename, or across file
/// systems a copy, after which the source is still where it was. A copy
/// that fails is removed.
fn place(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_fresh(from, to),
        Err(e) => Err(e),
    }
}

/// Copies `from` to `to`, which must not exist (so that what's removed
/// after a failure is only this copy).
fn copy_fresh(from: &Path, to: &Path) -> io::Result<()> {
    if exists(to) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    if let Err(e) = copy_path(from, to) {
        let _ = remove_path(to);
        return Err(e);
    }
    Ok(())
}

/// Copies a file, link or folder with its permissions and times (a
/// folder's time only on unix); links are copied as links.
fn copy_path(from: &Path, to: &Path) -> io::Result<()> {
    let m = from.symlink_metadata()?;
    if m.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return Err(io::Error::other(format!(
            "{} is a link, and links can't be moved across drives here",
            from.display()
        )));
    }
    if m.is_dir() {
        fs::create_dir(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy_path(&e.path(), &to.join(e.file_name()))?;
        }
        // The time after the entries, which writing them changed, and
        // before the mode, which may not allow opening the folder.
        #[cfg(unix)]
        if let Ok(t) = m.modified() {
            fs::File::open(to)?.set_modified(t)?;
        }
        fs::set_permissions(to, m.permissions())?;
        return Ok(());
    }
    fs::copy(from, to)?;
    if let Ok(t) = m.modified() {
        set_modified(to, t, &m)?;
    }
    Ok(())
}

/// Sets the modification time of the fresh copy at `p`, which `fs::copy`
/// has already given `m`'s mode.
#[cfg(unix)]
fn set_modified(p: &Path, t: std::time::SystemTime, _m: &fs::Metadata) -> io::Result<()> {
    // The owner can set the time through a read-only handle (`futimens`
    // doesn't need write access), so a read-only mode (`0o444`, every
    // file under `.git/objects`) needs no relaxing: the copy never
    // spends a moment writable.
    fs::File::open(p)?.set_modified(t)
}

/// Sets the modification time of the fresh copy at `p`, which `fs::copy`
/// has already given `m`'s mode. The read-only attribute stops the file
/// being opened for writing, so the time is set with it cleared for the
/// moment it takes, then put back.
#[cfg(not(unix))]
fn set_modified(p: &Path, t: std::time::SystemTime, m: &fs::Metadata) -> io::Result<()> {
    let open = || fs::File::options().write(true).open(p);
    match open() {
        Ok(f) => f.set_modified(t),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let mut writable = m.permissions();
            writable.set_readonly(false);
            fs::set_permissions(p, writable)?;
            let res = open().and_then(|f| f.set_modified(t));
            fs::set_permissions(p, m.permissions())?;
            res
        }
        Err(e) => Err(e),
    }
}

/// Reads a file node from the repository in chunks.
pub struct NodeReader<'a> {
    repo: &'a dyn Repo,
    node: &'a Node,
    offset: u64,
    buf: Vec<u8>,
    pos: usize,
}

impl<'a> NodeReader<'a> {
    pub fn new(repo: &'a dyn Repo, node: &'a Node) -> Self {
        Self {
            repo,
            node,
            offset: 0,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for NodeReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.buf.len() {
            if self.offset >= self.node.size {
                return Ok(0);
            }
            self.buf = self
                .repo
                .read_at(self.node, self.offset, 1 << 20)
                .map_err(io::Error::other)?;
            if self.buf.is_empty() {
                return Ok(0);
            }
            self.offset += self.buf.len() as u64;
            self.pos = 0;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// How far a tar archive has got.
struct TarProgress<'a> {
    done: u64,
    total: u64,
    report: &'a mut dyn FnMut(RestoreStep),
    cancel: &'a Cancel,
}

impl TarProgress<'_> {
    fn add(&mut self, n: u64) -> io::Result<()> {
        if self.cancel.cancelled() {
            return Err(io::Error::other(Stopped));
        }
        self.done += n;
        (self.report)(RestoreStep::Bytes {
            done: self.done,
            total: self.total,
        });
        Ok(())
    }
}

/// A reader that counts what goes through it.
struct Counted<'a, 'b, R> {
    inner: R,
    p: &'a mut TarProgress<'b>,
}

impl<R: Read> Read for Counted<'_, '_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        self.p.add(n as u64)?;
        Ok(n)
    }
}

/// The size of the files in a node and under it.
fn size_of(repo: &dyn Repo, node: &Node) -> Result<u64> {
    Ok(match (&node.kind, node.subtree) {
        (NodeKind::File, _) => node.size,
        (NodeKind::Dir, Some(sub)) => {
            let mut n = 0;
            for c in &repo.tree(&sub)?.nodes {
                n += size_of(repo, c)?;
            }
            n
        }
        _ => 0,
    })
}

/// Writes a folder (or file) from a snapshot as a tar archive. Stopped,
/// it removes the archive and fails with [`Stopped`].
fn write_tar(
    repo: &dyn Repo,
    t: &Target,
    dest: &Path,
    report: &mut dyn FnMut(RestoreStep),
    cancel: &Cancel,
) -> Result<()> {
    report(RestoreStep::Preparing);
    // Walking the trees first gives a total; writing reads them again from
    // the cache.
    let total = size_of(repo, &t.node)?;
    let mut p = TarProgress {
        done: 0,
        total,
        report,
        cancel,
    };
    p.add(0)?;
    let file =
        fs::File::create_new(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut b = tar::Builder::new(io::BufWriter::new(file));
    b.follow_symlinks(false);
    let res = append(repo, &mut b, &t.node, Path::new(&t.name()), &mut p).and_then(|()| {
        b.into_inner()?.flush()?;
        Ok(())
    });
    if res.is_err() {
        let _ = fs::remove_file(dest);
        if cancel.cancelled() {
            return Err(Stopped.into());
        }
    }
    res
}

fn append<W: Write>(
    repo: &dyn Repo,
    b: &mut tar::Builder<W>,
    node: &Node,
    path: &Path,
    p: &mut TarProgress,
) -> Result<()> {
    p.add(0)?;
    let mut h = tar::Header::new_gnu();
    h.set_mode(node.mode.unwrap_or(0o644) & 0o7777);
    h.set_mtime(node.mtime.map_or(0, |t| t.as_second().max(0) as u64));
    h.set_uid(u64::from(node.uid.unwrap_or(0)));
    h.set_gid(u64::from(node.gid.unwrap_or(0)));
    match &node.kind {
        NodeKind::Dir => {
            h.set_entry_type(tar::EntryType::Directory);
            h.set_size(0);
            b.append_data(&mut h, path, io::empty())?;
            if let Some(sub) = node.subtree {
                for n in &repo.tree(&sub)?.nodes {
                    append(repo, b, n, &path.join(&n.name), p)?;
                }
            }
        }
        NodeKind::File => {
            h.set_entry_type(tar::EntryType::Regular);
            h.set_size(node.size);
            let r = Counted {
                inner: NodeReader::new(repo, node),
                p,
            };
            b.append_data(&mut h, path, r)?;
        }
        NodeKind::Symlink { target } => {
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, path, target)?;
        }
        NodeKind::Other(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file under `dir`, relative to it, with its content.
    fn tree(dir: &Path) -> Vec<(PathBuf, String)> {
        fn walk(root: &Path, dir: &Path, v: &mut Vec<(PathBuf, String)>) {
            for e in fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(root, &p, v);
                } else {
                    let rel = p.strip_prefix(root).unwrap().to_path_buf();
                    v.push((rel, fs::read_to_string(&p).unwrap_or_default()));
                }
            }
        }
        let mut v = Vec::new();
        walk(dir, dir, &mut v);
        v.sort();
        v
    }

    fn sample(dir: &Path) -> PathBuf {
        let from = dir.join("from");
        fs::create_dir_all(from.join("sub")).unwrap();
        fs::write(from.join("a"), "a\n").unwrap();
        fs::write(from.join("sub/b"), "b\n").unwrap();
        from
    }

    #[test]
    fn an_undo_copy_knows_its_step() {
        let step = Path::new("/u/20260701T100000.000");
        assert_eq!(step_of(&saved_at(step, 1, "sub")), step);
    }

    /// The copy a cross-device move makes shows up at `to` whole, with no
    /// `.part` left, and the source is gone.
    #[test]
    fn copy_aside_moves_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let from = sample(dir.path());
        let to = dir.path().join("to");
        copy_aside(&from, &to).unwrap();
        assert!(!exists(&from));
        assert!(!exists(&dir.path().join("to.part")));
        assert_eq!(
            tree(&to),
            [
                (PathBuf::from("a"), "a\n".into()),
                (PathBuf::from("sub/b"), "b\n".into())
            ]
        );
    }

    /// A copy that fails part way leaves nothing at `to` or next to it,
    /// and the source as it was.
    #[cfg(unix)]
    #[test]
    fn a_failed_copy_aside_leaves_the_source_and_no_copy() {
        use std::os::unix::fs::PermissionsExt;
        if crate::repo::is_root() {
            eprintln!("skipped: root can read anything");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let from = sample(dir.path());
        fs::set_permissions(from.join("sub/b"), fs::Permissions::from_mode(0o000)).unwrap();
        let to = dir.path().join("to");
        copy_aside(&from, &to).unwrap_err();
        assert!(!exists(&to));
        assert!(!exists(&dir.path().join("to.part")));
        fs::set_permissions(from.join("sub/b"), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(tree(&from).len(), 2);
    }

    /// A read-only file (as every object in a `.git` is) is moved across
    /// file systems with its content, mode and time, and nothing else is
    /// left behind.
    #[cfg(unix)]
    #[test]
    fn copy_aside_keeps_a_read_only_file_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let from = sample(dir.path());
        let ro = from.join("sub/b");
        let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        fs::File::options()
            .write(true)
            .open(&ro)
            .unwrap()
            .set_modified(t)
            .unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
        let to = dir.path().join("to");
        copy_aside(&from, &to).unwrap();
        assert!(!exists(&from));
        let m = to.join("sub/b").metadata().unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o444);
        assert_eq!(m.modified().unwrap(), t);
        assert_eq!(fs::read_to_string(to.join("sub/b")).unwrap(), "b\n");
        // And back again, as an undo across file systems does it.
        let back = dir.path().join("back");
        place(&to, &back).unwrap();
        assert_eq!(
            back.join("sub/b").metadata().unwrap().modified().unwrap(),
            t
        );
    }

    /// A folder comes back with its own modification time, not the time
    /// of the copy: tools that compare trees by it would otherwise see
    /// every folder as changed.
    #[cfg(unix)]
    #[test]
    fn copy_fresh_keeps_a_folders_time() {
        let dir = tempfile::tempdir().unwrap();
        let from = sample(dir.path());
        let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        for p in [from.join("sub"), from.clone()] {
            fs::File::open(&p).unwrap().set_modified(t).unwrap();
        }
        let to = dir.path().join("to");
        copy_fresh(&from, &to).unwrap();
        assert_eq!(to.join("sub").metadata().unwrap().modified().unwrap(), t);
        assert_eq!(to.metadata().unwrap().modified().unwrap(), t);
    }

    /// A copy never goes over something that's already at `to`, and a
    /// failure doesn't remove it.
    #[test]
    fn copy_fresh_refuses_an_occupied_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = sample(dir.path());
        let to = dir.path().join("to");
        fs::create_dir(&to).unwrap();
        fs::write(to.join("theirs"), "x").unwrap();
        let e = copy_fresh(&from, &to).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(to.join("theirs")).unwrap(), "x");
    }
}
