//! The undo folder keeps the user's files through failures: a crash
//! between steps, a put-back that fails half way, a retry.

use std::fs;
use std::path::PathBuf;

use jiff::tz::TimeZone;

use restoric::repo::fake::FakeRepo;
use restoric::repo::{Repo, SnapshotInfo};
use restoric::restore::{How, Places, Target, run, undo};
use restoric::worker::Cancel;

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: FakeRepo,
    snap: SnapshotInfo,
    places: Places,
}

/// One snapshot of `root` with `a.txt` and `sub/c.txt`; on disk both hold
/// the user's own edits.
fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "my a\n").unwrap();
    fs::write(root.join("sub/c.txt"), "my c\n").unwrap();
    let dsl = format!(
        "root {}\nsnapshot 2026-07-01 10:00\n  write a.txt alpha\\n\n  write sub/c.txt charlie\\n\n",
        root.display()
    );
    let repo = FakeRepo::parse(&dsl).unwrap();
    let snap = repo.snapshots().unwrap().remove(0);
    let places = Places {
        restore_dir: dir.path().join("Restored"),
        undo_dir: Some(dir.path().join("undo")),
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

    fn overwrite(&self, names: &[&str]) -> anyhow::Result<()> {
        let items: Vec<Target> = names.iter().map(|n| self.target(n)).collect();
        let o = run(
            &self.repo,
            &items,
            &How::Overwrite,
            &self.places,
            &mut |_| {},
            &Cancel::default(),
        )?;
        assert!(!o.stopped);
        Ok(())
    }

    fn sessions(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = match fs::read_dir(self.places.undo_dir.as_ref().unwrap()) {
            Ok(rd) => rd.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        };
        v.sort();
        v
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(name)).unwrap()
    }
}

/// The copy an undo step keeps lives under the step, never at a path
/// derived by joining the original's absolute path (on Windows that join
/// would be the original itself).
#[test]
fn the_undo_copy_is_kept_under_the_step() {
    let s = setup();
    s.overwrite(&["a.txt", "sub/c.txt"]).unwrap();
    assert_eq!(s.read("a.txt"), "alpha\n");
    let session = s.sessions().pop().unwrap();
    let saved: Vec<PathBuf> = walk(&session.join("files"));
    assert_eq!(saved.len(), 2, "{saved:?}");
    for p in &saved {
        let rel = p.strip_prefix(session.join("files")).unwrap();
        assert!(
            !rel.starts_with(s.root.strip_prefix("/").unwrap()),
            "{} mirrors the original's absolute path",
            p.display()
        );
    }
    assert_eq!(undo(&s.places).unwrap().len(), 2);
    assert_eq!(s.read("a.txt"), "my a\n");
    assert_eq!(s.read("sub/c.txt"), "my c\n");
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v.sort();
    v
}

/// A step whose manifest names a copy that isn't there (the process died
/// between recording the entry and moving the file) leaves the original
/// where it is, rather than deleting it and then failing to put it back.
#[test]
fn undo_leaves_a_path_alone_when_its_saved_copy_is_missing() {
    let s = setup();
    let undo_dir = s.places.undo_dir.clone().unwrap();
    let session = undo_dir.join("20260701T100000.000");
    fs::create_dir_all(session.join("files")).unwrap();
    let saved = session.join("files").join("0").join("a.txt");
    let manifest = serde_json::json!({
        "entries": [{ "path": s.root.join("a.txt"), "saved": saved }]
    });
    fs::write(session.join("manifest.json"), manifest.to_string()).unwrap();

    undo(&s.places).unwrap();
    assert_eq!(s.read("a.txt"), "my a\n");
    assert!(s.sessions().is_empty(), "the step is finished with");
}

/// An undo that fails half way can be run again, and the second run
/// doesn't delete what the first one already put back.
#[cfg(unix)]
#[test]
fn a_failed_undo_can_be_retried_without_losing_what_it_put_back() {
    use std::os::unix::fs::PermissionsExt;
    let s = setup();
    s.overwrite(&["a.txt", "sub/c.txt"]).unwrap();
    assert_eq!(s.read("a.txt"), "alpha\n");
    assert_eq!(s.read("sub/c.txt"), "charlie\n");

    // Entries are put back last first: sub/c.txt goes back, then removing
    // a.txt from the read-only root fails.
    fs::set_permissions(&s.root, fs::Permissions::from_mode(0o555)).unwrap();
    let err = undo(&s.places).unwrap_err();
    fs::set_permissions(&s.root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(format!("{err:#}").contains("a.txt"), "{err:#}");
    assert_eq!(s.read("sub/c.txt"), "my c\n");
    assert_eq!(s.read("a.txt"), "alpha\n");

    let back = undo(&s.places).unwrap();
    assert_eq!(s.read("sub/c.txt"), "my c\n", "put back once, kept");
    assert_eq!(s.read("a.txt"), "my a\n");
    assert!(back.contains(&s.root.join("a.txt")));
    assert!(s.sessions().is_empty());
    assert!(undo(&s.places).is_err(), "nothing left to undo");
}

/// A restore that fails after the original was moved aside puts it back
/// and leaves no trace of it in the undo step.
#[test]
fn a_failed_overwrite_puts_the_original_back() {
    let s = setup();
    // The snapshot has no b.txt: its restore fails after the move aside.
    let mut gone = s.target("a.txt");
    gone.path = s.root.join("b.txt");
    fs::write(s.root.join("b.txt"), "my b\n").unwrap();
    let items = [s.target("a.txt"), gone];
    let err = run(
        &s.repo,
        &items,
        &How::Overwrite,
        &s.places,
        &mut |_| {},
        &Cancel::default(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("b.txt"), "{err:#}");
    assert_eq!(s.read("b.txt"), "my b\n");
    assert_eq!(s.read("a.txt"), "alpha\n", "the first item stays restored");
    assert_eq!(undo(&s.places).unwrap(), [s.root.join("a.txt")]);
    assert_eq!(s.read("a.txt"), "my a\n");
}
