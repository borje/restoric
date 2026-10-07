//! Restore progress and stopping a restore (PLAN.md §3.11), on FakeRepo
//! and a temporary folder.

use std::fs;
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;

use restoric::repo::fake::FakeRepo;
use restoric::repo::{Repo, RestoreStep, SnapshotInfo};
use restoric::restore::{How, Places, Progress, Target, run, undo};
use restoric::worker::Cancel;

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: FakeRepo,
    snap: SnapshotInfo,
    places: Places,
}

/// One snapshot of `root` with `a.txt`, `b.txt` and `sub/` holding two files.
fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir(&root).unwrap();
    let dsl = format!(
        "root {}\nsnapshot 2026-07-01 10:00\n  write a.txt alpha\\n\n  write b.txt bravo\\n\n  write sub/c.txt charlie\\n\n  write sub/d.txt delta\\n\n",
        root.display()
    );
    let repo = FakeRepo::parse(&dsl).unwrap();
    let snap = repo.snapshots().unwrap().remove(0);
    let places = Places {
        restore_dir: dir.path().join("Restored"),
        undo_dir: dir.path().join("undo"),
        tz: TimeZone::UTC,
    };
    Setup {
        _dir: dir,
        root,
        repo,
        snap,
        places,
    }
}

impl Setup {
    fn target(&self, name: &str) -> Target {
        let path = self.root.join(name);
        let mut tree = self.repo.tree(&self.snap.tree).unwrap();
        let mut node = None;
        for c in path.components().skip(1) {
            let n = tree.get(c.as_os_str()).unwrap().clone();
            if let Some(sub) = n.subtree {
                tree = self.repo.tree(&sub).unwrap();
            }
            node = Some(n);
        }
        Target {
            snapshot: self.snap.clone(),
            path,
            node: node.unwrap(),
        }
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn progress_names_each_item_and_counts_bytes() {
    let s = setup();
    let items = [s.target("a.txt"), s.target("sub")];
    let mut seen: Vec<Progress> = Vec::new();
    let o = run(
        &s.repo,
        &items,
        How::RestoreDir,
        &s.places,
        &mut |p| seen.push(p),
        &Cancel::default(),
    )
    .unwrap();
    assert!(!o.stopped);
    assert_eq!(o.done.len(), 2);
    let firsts: Vec<(usize, usize, &str, RestoreStep)> = seen
        .iter()
        .filter(|p| p.step == RestoreStep::Preparing)
        .map(|p| (p.item, p.items, p.name.as_str(), p.step))
        .collect();
    assert_eq!(
        firsts,
        [
            (0, 2, "a.txt", RestoreStep::Preparing),
            (1, 2, "sub/", RestoreStep::Preparing)
        ]
    );
    let total = ("charlie\n".len() + "delta\n".len()) as u64;
    assert_eq!(
        seen.last().unwrap().step,
        RestoreStep::Bytes { done: total, total }
    );
}

#[test]
fn stopped_copies_keep_finished_items() {
    let s = setup();
    let items = [s.target("a.txt"), s.target("sub"), s.target("b.txt")];
    let cancel = Cancel::default();
    // Stop as the second item starts.
    let stop = cancel.clone();
    let o = run(
        &s.repo,
        &items,
        How::RestoreDir,
        &s.places,
        &mut |p| {
            if p.item == 1 {
                stop.cancel();
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(o.stopped);
    assert_eq!(o.done.len(), 1);
    let day = s.places.restore_dir.join("2026-07-01_1000");
    assert_eq!(names(&day), ["a.txt"], "the partial sub/ is removed");
}

#[test]
fn a_stopped_overwrite_puts_everything_back() {
    let s = setup();
    fs::write(s.root.join("a.txt"), b"edited a\n").unwrap();
    fs::create_dir(s.root.join("sub")).unwrap();
    fs::write(s.root.join("sub/new.txt"), b"new\n").unwrap();
    let items = [s.target("a.txt"), s.target("sub")];
    let cancel = Cancel::default();
    let stop = cancel.clone();
    let o = run(
        &s.repo,
        &items,
        How::Overwrite,
        &s.places,
        &mut |p| {
            if p.item == 1 {
                stop.cancel();
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(o.stopped);
    assert!(o.done.is_empty());
    assert_eq!(fs::read(s.root.join("a.txt")).unwrap(), b"edited a\n");
    assert_eq!(names(&s.root.join("sub")), ["new.txt"]);
    assert!(undo(&s.places).is_err(), "no undo step is left behind");
}

#[test]
fn an_overwrite_stopped_during_the_last_item_is_undone_too() {
    let s = setup();
    fs::write(s.root.join("a.txt"), b"edited a\n").unwrap();
    let cancel = Cancel::default();
    let stop = cancel.clone();
    // Stopped once copying started: the copy finishes, then it's put back.
    let o = run(
        &s.repo,
        &[s.target("a.txt")],
        How::Overwrite,
        &s.places,
        &mut |p| {
            if matches!(p.step, RestoreStep::Bytes { .. }) {
                stop.cancel();
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(o.stopped);
    assert_eq!(fs::read(s.root.join("a.txt")).unwrap(), b"edited a\n");
}

#[test]
fn tar_counts_bytes_and_stops_anywhere() {
    let s = setup();
    let total = ("charlie\n".len() + "delta\n".len()) as u64;
    let mut last = None;
    let o = run(
        &s.repo,
        &[s.target("sub")],
        How::Tar,
        &s.places,
        &mut |p| last = Some(p.step),
        &Cancel::default(),
    )
    .unwrap();
    assert_eq!(last, Some(RestoreStep::Bytes { done: total, total }));
    fs::remove_file(&o.done[0].dest).unwrap();

    // Stopped once writing has started: no archive is left.
    let cancel = Cancel::default();
    let stop = cancel.clone();
    let o = run(
        &s.repo,
        &[s.target("sub")],
        How::Tar,
        &s.places,
        &mut |p| {
            if matches!(p.step, RestoreStep::Bytes { .. }) {
                stop.cancel();
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(o.stopped);
    assert!(names(&s.root).is_empty());
}
