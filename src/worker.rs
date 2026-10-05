//! Background work (PLAN.md §4.5). The UI thread sends [`Request`]s and
//! draws from the [`Response`]s; it never touches the repository or disk.
//!
//! Each request carries a generation. When the user moves on, the UI bumps
//! the generation, and requests about what was on screen that haven't
//! started yet are skipped. [`handle`] does the work; tests call it directly.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};

use crate::diff::{self, DiffKey, FileDiff};
use crate::disk::Disk;
use crate::index::find::Found;
use crate::index::fingerprint::{self, Fp};
use crate::index::folder::Counts;
use crate::index::listing::Entry;
use crate::index::timeline::ChangePoint;
use crate::index::versions::Run;
use crate::index::{Index, NodeRef};
use crate::repo::{FileBytes, Node, SnapshotId, SnapshotInfo};
use crate::restore::{self, Done, How, Places, Target};

/// Most of a file the preview reads (PLAN.md §4.8).
pub const PREVIEW_LIMIT: u64 = 64 * 1024;

/// What the worker works with.
pub struct Ctx {
    pub index: Index,
    pub disk: Arc<dyn Disk>,
    pub places: Places,
}

/// Most of a file "Show in $PAGER" reads.
pub const PAGER_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub enum Request {
    /// What's at `path` in each snapshot of `set`, its change points and
    /// versions. `item` marks the selected entry's track (skippable) rather
    /// than the folder's.
    ChangePoints {
        set: Arc<Vec<SnapshotInfo>>,
        path: PathBuf,
        item: bool,
    },
    /// Counts for some of a folder's change points (indexes into its refs).
    PointCounts {
        path: PathBuf,
        refs: Arc<Vec<NodeRef>>,
        points: Vec<usize>,
    },
    /// The folder's entries at `set[snap]`.
    Listing {
        set: Arc<Vec<SnapshotInfo>>,
        snap: usize,
        folder: PathBuf,
    },
    /// Items deleted from the folder before `set[snap - 1]`.
    DeletedEarlier {
        set: Arc<Vec<SnapshotInfo>>,
        snap: usize,
        folder: PathBuf,
    },
    /// What changed on disk under `path` since the newest snapshot in `set`.
    Live {
        set: Arc<Vec<SnapshotInfo>>,
        path: PathBuf,
        item: bool,
    },
    /// The start of a file in the repository.
    ReadFile { node: Node },
    /// The start of a file on disk.
    ReadDisk { path: PathBuf },
    /// The nodes at `path` in some snapshots (the starts of versions).
    Nodes {
        set: Arc<Vec<SnapshotInfo>>,
        path: PathBuf,
        snaps: Vec<usize>,
    },
    /// Restore these items (writes to disk, never to the repository).
    Restore { targets: Vec<Target>, how: How },
    /// Undo the last overwrite.
    Undo,
    /// Whether these paths exist on disk.
    Exists { paths: Vec<PathBuf> },
    /// A whole file (up to a limit), for the pager.
    ReadAll { node: Node, name: String },
    /// Read the snapshot list again (`:reload`).
    Reload,
    /// Everything under `root` with `query` in its path, in every snapshot.
    Find {
        set: Arc<Vec<SnapshotInfo>>,
        root: PathBuf,
        query: String,
    },
    /// A full-screen diff: read both sides up to `key.limit` and diff them.
    Diff {
        key: DiffKey,
        old: Box<Side>,
        new: Box<Side>,
    },
}

/// Where a side of a diff comes from.
#[derive(Clone, Debug)]
pub enum Side {
    Repo(Node),
    Disk(PathBuf),
    Nothing,
}

impl Request {
    /// Whether the request is only about what's on screen right now, so it
    /// can be skipped once the user has moved on.
    fn skippable(&self) -> bool {
        match self {
            Request::ChangePoints { item, .. } | Request::Live { item, .. } => *item,
            Request::PointCounts { .. }
            | Request::Restore { .. }
            | Request::Undo
            | Request::ReadAll { .. }
            | Request::Reload
            | Request::Find { .. } => false,
            _ => true,
        }
    }
}

#[derive(Debug)]
pub enum Response {
    Progress {
        path: PathBuf,
        done: usize,
        total: usize,
    },
    ChangePoints {
        path: PathBuf,
        refs: Arc<Vec<NodeRef>>,
        points: Vec<ChangePoint>,
        runs: Vec<Run>,
    },
    PointCounts {
        path: PathBuf,
        counts: Vec<(usize, Counts)>,
    },
    Listing {
        snapshot: SnapshotId,
        folder: PathBuf,
        /// `None` when the folder doesn't exist in that snapshot.
        entries: Option<Arc<Vec<Entry>>>,
    },
    DeletedEarlier {
        snapshot: SnapshotId,
        folder: PathBuf,
        entries: Arc<Vec<Entry>>,
    },
    Live {
        path: PathBuf,
        counts: Counts,
    },
    File {
        key: Fp,
        bytes: Arc<FileBytes>,
    },
    DiskFile {
        path: PathBuf,
        /// `None` when it's missing or not a file.
        bytes: Option<Arc<FileBytes>>,
    },
    Nodes {
        path: PathBuf,
        nodes: Vec<(usize, Option<Node>)>,
    },
    Diff {
        key: DiffKey,
        diff: Arc<FileDiff>,
    },
    Restored {
        how: How,
        done: Vec<Done>,
    },
    Undone(Vec<PathBuf>),
    Exists(Vec<(PathBuf, bool)>),
    Pager {
        name: String,
        bytes: Vec<u8>,
    },
    Snapshots(Vec<SnapshotInfo>),
    Found {
        query: String,
        /// With whether each is on disk now.
        results: Vec<(Found, bool)>,
        /// `(done, total)` while still searching.
        progress: Option<(usize, usize)>,
    },
    Error(String),
}

/// Does one request, sending responses as they're ready.
pub fn handle(ctx: &Ctx, req: Request, send: &mut dyn FnMut(Response)) {
    let index = &ctx.index;
    let res = (|| -> anyhow::Result<()> {
        match req {
            Request::ChangePoints { set, path, .. } => {
                let mut progress = |done: usize, total: usize| {
                    if done.is_multiple_of(64) && done < total {
                        send(Response::Progress {
                            path: path.clone(),
                            done,
                            total,
                        });
                    }
                };
                let refs = index.refs(&set, &path, &mut progress)?;
                let points = index.change_points(&refs)?;
                let runs = index.runs(&refs)?;
                send(Response::ChangePoints {
                    path,
                    refs: Arc::new(refs),
                    points,
                    runs,
                });
            }
            Request::PointCounts { path, refs, points } => {
                let mut counts = Vec::with_capacity(points.len());
                for i in points {
                    let prev = i
                        .checked_sub(1)
                        .map(|p| refs[p])
                        .unwrap_or(NodeRef::Missing);
                    counts.push((i, index.ref_counts(&prev, &refs[i])?));
                }
                send(Response::PointCounts { path, counts });
            }
            Request::Listing { set, snap, folder } => {
                let entries = index.listing(&set, snap, &folder)?;
                send(Response::Listing {
                    snapshot: set[snap].id,
                    folder,
                    entries: entries.map(Arc::new),
                });
            }
            Request::DeletedEarlier { set, snap, folder } => {
                let entries = index.deleted_earlier(&set, snap, &folder)?;
                send(Response::DeletedEarlier {
                    snapshot: set[snap].id,
                    folder,
                    entries: Arc::new(entries),
                });
            }
            Request::Live { set, path, .. } => {
                let counts = match set.last() {
                    Some(newest) => index.live(ctx.disk.as_ref(), newest, &path)?,
                    None => Counts::default(),
                };
                send(Response::Live { path, counts });
            }
            Request::ReadFile { node } => {
                let bytes = index.repo().read_file(&node, PREVIEW_LIMIT)?;
                send(Response::File {
                    key: fingerprint::content(&node),
                    bytes: Arc::new(bytes),
                });
            }
            Request::ReadDisk { path } => {
                let size = ctx.disk.stat(&path).map_or(0, |d| d.size);
                let bytes = ctx
                    .disk
                    .read(&path, PREVIEW_LIMIT)
                    .map(|data| Arc::new(FileBytes { data, size }));
                send(Response::DiskFile { path, bytes });
            }
            Request::Nodes { set, path, snaps } => {
                let mut nodes = Vec::with_capacity(snaps.len());
                for i in snaps {
                    nodes.push((i, index.node_at(&set[i], &path)?));
                }
                send(Response::Nodes { path, nodes });
            }
            Request::Restore { targets, how } => {
                let done = restore::run(index.repo().as_ref(), &targets, how, &ctx.places)?;
                send(Response::Restored { how, done });
            }
            Request::Undo => match restore::undo(&ctx.places) {
                Ok(paths) => send(Response::Undone(paths)),
                Err(e) => send(Response::Error(format!("{e:#}"))),
            },
            Request::Exists { paths } => {
                let found = paths
                    .into_iter()
                    .map(|p| {
                        let e = ctx.disk.stat(&p).is_some();
                        (p, e)
                    })
                    .collect();
                send(Response::Exists(found));
            }
            Request::ReadAll { node, name } => {
                let bytes = index.repo().read_at(&node, 0, PAGER_LIMIT)?;
                send(Response::Pager { name, bytes });
            }
            Request::Reload => send(Response::Snapshots(index.repo().snapshots()?)),
            Request::Find { set, root, query } => {
                let on_disk = |f: Vec<Found>| -> Vec<(Found, bool)> {
                    f.into_iter()
                        .map(|f| {
                            let e = ctx.disk.stat(&root.join(&f.path)).is_some();
                            (f, e)
                        })
                        .collect()
                };
                let found = index.find(&set, &root, &query, &mut |done, total, partial| {
                    send(Response::Found {
                        query: query.clone(),
                        results: on_disk(partial),
                        progress: Some((done, total)),
                    });
                })?;
                send(Response::Found {
                    query,
                    results: on_disk(found),
                    progress: None,
                });
            }
            Request::Diff { key, old, new } => {
                let diff = file_diff(ctx, &old, &new, key.limit)?;
                send(Response::Diff {
                    key,
                    diff: Arc::new(diff),
                });
            }
        }
        index.flush()?;
        Ok(())
    })();
    if let Err(e) = res {
        tracing::warn!("worker: {e:#}");
        send(Response::Error(format!("{e:#}")));
    }
}

/// Reads both sides (unless one is over `limit`) and diffs them.
fn file_diff(ctx: &Ctx, old: &Side, new: &Side, limit: u64) -> anyhow::Result<FileDiff> {
    let size = |s: &Side| -> Option<u64> {
        match s {
            Side::Repo(n) => Some(n.size),
            Side::Disk(p) => ctx.disk.stat(p).map(|d| d.size),
            Side::Nothing => Some(0),
        }
    };
    let (Some(old_size), Some(new_size)) = (size(old), size(new)) else {
        return Ok(FileDiff::Missing);
    };
    if old_size.max(new_size) > limit {
        return Ok(FileDiff::TooBig(old_size.max(new_size)));
    }
    let read = |s: &Side| -> anyhow::Result<Option<Vec<u8>>> {
        Ok(match s {
            Side::Repo(n) => Some(ctx.index.repo().read_file(n, limit)?.data),
            Side::Disk(p) => ctx.disk.read(p, limit),
            Side::Nothing => Some(Vec::new()),
        })
    };
    let (Some(a), Some(b)) = (read(old)?, read(new)?) else {
        return Ok(FileDiff::Missing);
    };
    Ok(match (diff::text(&a), diff::text(&b)) {
        (Some(a), Some(b)) => FileDiff::text(&a, &b),
        _ => FileDiff::Binary {
            old: old_size,
            new: new_size,
        },
    })
}

/// A request and the generation it was sent in.
type Job = (u64, Request);

/// The worker pool.
pub struct Worker {
    tx: Sender<Job>,
    generation: Arc<AtomicU64>,
}

impl Worker {
    pub fn start(ctx: Arc<Ctx>, threads: usize, out: Sender<Response>) -> Self {
        let (tx, rx): (Sender<Job>, Receiver<Job>) = crossbeam_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..threads.max(2) {
            let (rx, out, ctx, generation) =
                (rx.clone(), out.clone(), ctx.clone(), generation.clone());
            std::thread::spawn(move || {
                for (g, req) in rx {
                    if req.skippable() && g < generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    handle(&ctx, req, &mut |r| {
                        let _ = out.send(r);
                    });
                }
            });
        }
        Self { tx, generation }
    }

    /// Skippable requests from before this call that haven't started are skipped.
    pub fn bump(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn send(&self, req: Request) {
        let g = self.generation.load(Ordering::Relaxed);
        let _ = self.tx.send((g, req));
    }
}
