//! Background work (PLAN.md §4.5). The UI thread sends [`Request`]s and
//! draws from the [`Response`]s; it never touches the repository.
//!
//! Each request carries a generation. When the user moves on, the UI bumps
//! the generation, and requests from older generations that haven't started
//! yet are skipped. [`handle`] does the work and is called directly by tests.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};

use crate::index::folder::Counts;
use crate::index::listing::Entry;
use crate::index::timeline::ChangePoint;
use crate::index::{Index, NodeRef};
use crate::repo::{SnapshotId, SnapshotInfo};

#[derive(Clone, Debug)]
pub enum Request {
    /// What's at `folder` in each snapshot of `set`, and its change points.
    ChangePoints {
        set: Arc<Vec<SnapshotInfo>>,
        folder: PathBuf,
    },
    /// Counts for some of a folder's change points (indexes into its refs).
    PointCounts {
        folder: PathBuf,
        refs: Arc<Vec<NodeRef>>,
        points: Vec<usize>,
    },
    /// The folder's entries at `set[snap]`.
    Listing {
        set: Arc<Vec<SnapshotInfo>>,
        snap: usize,
        folder: PathBuf,
    },
    /// Names deleted from the folder before `set[snap - 1]`.
    DeletedEarlier {
        set: Arc<Vec<SnapshotInfo>>,
        snap: usize,
        folder: PathBuf,
    },
}

impl Request {
    /// Whether the request is about what's on screen right now, so it can be
    /// skipped once the user has moved on.
    fn skippable(&self) -> bool {
        matches!(
            self,
            Request::Listing { .. } | Request::DeletedEarlier { .. }
        )
    }
}

#[derive(Debug)]
pub enum Response {
    Progress {
        folder: PathBuf,
        done: usize,
        total: usize,
    },
    ChangePoints {
        folder: PathBuf,
        refs: Arc<Vec<NodeRef>>,
        points: Vec<ChangePoint>,
    },
    PointCounts {
        folder: PathBuf,
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
        names: Vec<OsString>,
    },
    Error(String),
}

/// Does one request, sending responses as they're ready.
pub fn handle(index: &Index, req: Request, send: &mut dyn FnMut(Response)) {
    let res = (|| -> anyhow::Result<()> {
        match req {
            Request::ChangePoints { set, folder } => {
                let mut progress = |done: usize, total: usize| {
                    if done.is_multiple_of(64) && done < total {
                        send(Response::Progress {
                            folder: folder.clone(),
                            done,
                            total,
                        });
                    }
                };
                let refs = index.refs(&set, &folder, &mut progress)?;
                let points = index.change_points(&refs)?;
                send(Response::ChangePoints {
                    folder,
                    refs: Arc::new(refs),
                    points,
                });
            }
            Request::PointCounts {
                folder,
                refs,
                points,
            } => {
                let mut counts = Vec::with_capacity(points.len());
                for i in points {
                    let prev = i
                        .checked_sub(1)
                        .map(|p| refs[p])
                        .unwrap_or(NodeRef::Missing);
                    counts.push((i, index.ref_counts(&prev, &refs[i])?));
                }
                send(Response::PointCounts { folder, counts });
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
                let names = index.deleted_earlier(&set, snap, &folder)?;
                send(Response::DeletedEarlier {
                    snapshot: set[snap].id,
                    folder,
                    names,
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

/// A request and the generation it was sent in.
type Job = (u64, Request);

/// The worker pool.
pub struct Worker {
    tx: Sender<Job>,
    generation: Arc<AtomicU64>,
}

impl Worker {
    pub fn start(index: Arc<Index>, threads: usize, out: Sender<Response>) -> Self {
        let (tx, rx): (Sender<Job>, Receiver<Job>) = crossbeam_channel::unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..threads.max(2) {
            let (rx, out, index, generation) =
                (rx.clone(), out.clone(), index.clone(), generation.clone());
            std::thread::spawn(move || {
                for (g, req) in rx {
                    if req.skippable() && g < generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    handle(&index, req, &mut |r| {
                        let _ = out.send(r);
                    });
                }
            });
        }
        Self { tx, generation }
    }

    /// Requests from before this call that haven't started are skipped.
    pub fn bump(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn send(&self, req: Request) {
        let g = self.generation.load(Ordering::Relaxed);
        let _ = self.tx.send((g, req));
    }
}
