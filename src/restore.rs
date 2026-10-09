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

use crate::repo::{Node, NodeKind, Repo, RestoreStep, SnapshotInfo, Stopped, is_stopped};
use crate::worker::Cancel;

/// Something to restore: what was at `path` in `snapshot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub snapshot: SnapshotInfo,
    /// The absolute path, in the snapshot and on disk.
    pub path: PathBuf,
    pub node: Node,
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
    /// `~/Restored`
    pub restore_dir: PathBuf,
    /// `~/.local/share/restoric/undo`
    pub undo_dir: PathBuf,
    pub tz: TimeZone,
}

impl Places {
    pub fn default_for(tz: TimeZone) -> Self {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let data = directories::ProjectDirs::from("", "", "restoric")
            .map(|d| d.data_local_dir().to_path_buf());
        Places {
            restore_dir: home.unwrap_or_default().join("Restored"),
            undo_dir: data.unwrap_or_else(std::env::temp_dir).join("undo"),
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
/// without the `-2` a clash adds. Doesn't look at the disk.
pub fn planned(t: &Target, how: &How, places: &Places) -> PathBuf {
    let st = stamp(t.snapshot.time, &places.tz);
    let parent = t.path.parent().unwrap_or(Path::new("/"));
    let name = t.name();
    match how {
        How::Overwrite => t.path.clone(),
        How::NextTo => parent.join(format!("{name}.{st}")),
        How::RestoreDir => places.restore_dir.join(&st).join(&name),
        How::Tar => parent.join(format!("{name}-{st}.tar")),
        How::Into(dir) => dir.join(&name),
    }
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
    let mut done = Vec::new();
    for (i, t) in targets.iter().enumerate() {
        if cancel.cancelled() {
            return Ok(Outcome {
                done,
                stopped: true,
            });
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
                let dir = places.restore_dir.join(&st);
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
            repo.restore(&t.snapshot, &t.path, &dest, &mut report, &|| {
                cancel.cancelled()
            })
        };
        match res {
            Ok(()) => done.push(Done {
                target: t.path.clone(),
                dest,
            }),
            Err(e) if is_stopped(&e) => {
                return Ok(Outcome {
                    done,
                    stopped: true,
                });
            }
            Err(e) => {
                // Nothing was at `dest` before: what's there is the partial copy.
                let _ = remove_path(&dest);
                return Err(e.context(format!("restoring {}", t.path.display())));
            }
        }
    }
    Ok(Outcome {
        done,
        stopped: false,
    })
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
}

const MANIFEST: &str = "manifest.json";

/// Moves what's on disk into a new undo folder, then restores over it.
/// If a restore fails, what was moved away is put back. If it's stopped,
/// every item of this step is put back.
fn overwrite(
    repo: &dyn Repo,
    targets: &[Target],
    places: &Places,
    rep: &mut Reporter,
    cancel: &Cancel,
) -> Result<Outcome> {
    let stopped = |session: &Path, manifest: &Manifest| -> Result<Outcome> {
        rollback(session, manifest)?;
        Ok(Outcome {
            done: Vec::new(),
            stopped: true,
        })
    };
    let session = new_session(&places.undo_dir)?;
    let mut manifest = Manifest::default();
    let mut done = Vec::new();
    for (i, t) in targets.iter().enumerate() {
        if cancel.cancelled() {
            return stopped(&session, &manifest);
        }
        let saved = if exists(&t.path) {
            let rel = t.path.strip_prefix("/").unwrap_or(&t.path);
            let to = session.join("files").join(rel);
            fs::create_dir_all(to.parent().expect("has a parent"))?;
            move_path(&t.path, &to)
                .with_context(|| format!("moving {} aside", t.path.display()))?;
            Some(to)
        } else {
            if let Some(p) = t.path.parent() {
                fs::create_dir_all(p)?;
            }
            None
        };
        manifest.entries.push(UndoEntry {
            path: t.path.clone(),
            saved: saved.clone(),
        });
        write_manifest(&session, &manifest)?;
        let mut report = |step| rep.send(i, t, step);
        if let Err(e) = repo.restore(&t.snapshot, &t.path, &t.path, &mut report, &|| {
            cancel.cancelled()
        }) {
            if is_stopped(&e) {
                return stopped(&session, &manifest);
            }
            // Put the old version back and stop.
            let _ = remove_path(&t.path);
            if let Some(s) = &saved {
                let _ = move_path(s, &t.path);
            }
            manifest.entries.pop();
            write_manifest(&session, &manifest)?;
            if manifest.entries.is_empty() {
                let _ = fs::remove_dir_all(&session);
            }
            return Err(e.context(format!("restoring {}", t.path.display())));
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
    Ok(Outcome {
        done,
        stopped: false,
    })
}

fn new_session(undo_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(undo_dir).with_context(|| format!("creating {}", undo_dir.display()))?;
    let now = Timestamp::now().strftime("%Y%m%dT%H%M%S%.f").to_string();
    let session = free(undo_dir, &now, "");
    fs::create_dir(&session)?;
    Ok(session)
}

fn write_manifest(session: &Path, m: &Manifest) -> Result<()> {
    fs::write(session.join(MANIFEST), serde_json::to_vec_pretty(m)?)?;
    Ok(())
}

/// Undoes the last overwrite. Returns the paths it put back.
pub fn undo(places: &Places) -> Result<Vec<PathBuf>> {
    let mut sessions: Vec<PathBuf> = match fs::read_dir(&places.undo_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join(MANIFEST).is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    sessions.sort();
    let Some(session) = sessions.pop() else {
        bail!("Nothing to undo.");
    };
    let m: Manifest = serde_json::from_slice(&fs::read(session.join(MANIFEST))?)?;
    rollback(&session, &m)
}

/// Puts back everything an undo step replaced, then removes the step.
/// Returns the paths it put back.
fn rollback(session: &Path, m: &Manifest) -> Result<Vec<PathBuf>> {
    let mut back = Vec::new();
    for e in m.entries.iter().rev() {
        remove_path(&e.path).with_context(|| format!("removing {}", e.path.display()))?;
        if let Some(s) = &e.saved {
            move_path(s, &e.path).with_context(|| format!("putting back {}", e.path.display()))?;
        }
        back.push(e.path.clone());
    }
    fs::remove_dir_all(session)?;
    back.reverse();
    Ok(back)
}

/// Removes a file, link or folder; never follows symlinks.
fn remove_path(p: &Path) -> io::Result<()> {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Renames, or copies and removes across file systems.
fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_path(from, to)?;
            remove_path(from)
        }
        Err(e) => Err(e),
    }
}

/// Copies a file, link or folder with its permissions and times; links
/// are copied as links.
fn copy_path(from: &Path, to: &Path) -> io::Result<()> {
    let m = from.symlink_metadata()?;
    if m.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        return Ok(());
    }
    if m.is_dir() {
        fs::create_dir(to)?;
        for e in fs::read_dir(from)? {
            let e = e?;
            copy_path(&e.path(), &to.join(e.file_name()))?;
        }
        fs::set_permissions(to, m.permissions())?;
        return Ok(());
    }
    fs::copy(from, to)?;
    if let Ok(t) = m.modified() {
        fs::File::options().write(true).open(to)?.set_modified(t)?;
    }
    Ok(())
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
