//! The undo folder keeps the user's files through failures: a crash
//! between steps, a put-back that fails half way, a retry.

use std::fs;
use std::path::PathBuf;

use jiff::tz::TimeZone;

use restoric::repo::fake::FakeRepo;
use restoric::repo::{Repo, RestoreStep, SnapshotInfo};
use restoric::restore::{How, Places, Target, run, undo};
use restoric::worker::Cancel;

struct Setup {
    _dir: tempfile::TempDir,
    /// The undo folder, when it's on another file system.
    _other: Option<tempfile::TempDir>,
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
        "root {}\nsnapshot 2026-07-01 10:00\n  write a.txt alpha\\n\n  write sub/c.txt charlie\\n\n  link cur -> a.txt\n",
        root.display()
    );
    let repo = FakeRepo::parse(&dsl).unwrap();
    let snap = repo.snapshots().unwrap().remove(0);
    let places = Places {
        restore_dir: Some(dir.path().join("Restored")),
        undo_dir: Some(dir.path().join("undo")),
        tz: TimeZone::UTC,
    };
    Setup {
        _dir: dir,
        _other: None,
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
    assert_eq!(undo(&s.places).unwrap().back.len(), 2);
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

    let back = undo(&s.places).unwrap().back;
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
    let text = format!("{err:#}");
    assert!(text.contains("b.txt"), "{text}");
    assert!(
        text.contains("the item before it was overwritten, and :undo puts the old version back"),
        "{text}"
    );
    assert_eq!(s.read("b.txt"), "my b\n");
    assert_eq!(s.read("a.txt"), "alpha\n", "the first item stays restored");
    assert_eq!(undo(&s.places).unwrap().back, [s.root.join("a.txt")]);
    assert_eq!(s.read("a.txt"), "my a\n");
}

/// A step whose copy was only part made (the process died while copying
/// across file systems) leaves the original where it is: the copy is at
/// its recorded place only once it's whole.
#[test]
fn undo_leaves_a_path_alone_when_only_a_partial_copy_was_made() {
    let s = setup();
    let undo_dir = s.places.undo_dir.clone().unwrap();
    let session = undo_dir.join("20260701T100000.000");
    let saved = session.join("files").join("0").join("a.txt");
    fs::create_dir_all(saved.parent().unwrap()).unwrap();
    fs::write(saved.with_file_name("a.txt.part"), "my").unwrap();
    let manifest = serde_json::json!({
        "entries": [{ "path": s.root.join("a.txt"), "saved": saved }]
    });
    fs::write(session.join("manifest.json"), manifest.to_string()).unwrap();

    assert!(undo(&s.places).unwrap().back.is_empty());
    assert_eq!(s.read("a.txt"), "my a\n");
    assert!(s.sessions().is_empty(), "the step is finished with");
}

/// An undo folder on another file system than the files: moving aside is
/// a copy. Linux only, and only where the temporary folder and /dev/shm
/// are different mounts.
#[cfg(target_os = "linux")]
fn across_devices() -> Option<Setup> {
    let mut s = setup();
    let shm = std::path::Path::new("/dev/shm");
    let probe = s.root.join("probe");
    fs::write(&probe, "").unwrap();
    let other = shm.join(format!("restoric-probe-{}", std::process::id()));
    let crosses = matches!(
        fs::rename(&probe, &other),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices
    );
    let _ = fs::remove_file(&probe);
    let _ = fs::remove_file(&other);
    if !crosses {
        eprintln!(
            "skipped: {} and /dev/shm are one file system",
            s.root.display()
        );
        return None;
    }
    let other = tempfile::Builder::new()
        .prefix("restoric-undo-")
        .tempdir_in(shm)
        .unwrap();
    s.places.undo_dir = Some(other.path().join("undo"));
    s._other = Some(other);
    Some(s)
}

/// Across file systems the copy is whole at its recorded place, with no
/// `.part` left, and undo copies it back and removes the step.
#[cfg(target_os = "linux")]
#[test]
fn an_overwrite_across_file_systems_can_be_undone() {
    let Some(s) = across_devices() else { return };
    s.overwrite(&["a.txt", "sub"]).unwrap();
    assert_eq!(s.read("a.txt"), "alpha\n");
    assert_eq!(s.read("sub/c.txt"), "charlie\n");
    let session = s.sessions().pop().unwrap();
    let saved = walk(&session.join("files"));
    assert_eq!(
        saved,
        [
            session.join("files/0/a.txt"),
            session.join("files/1/sub/c.txt")
        ]
    );

    assert_eq!(undo(&s.places).unwrap().back.len(), 2);
    assert_eq!(s.read("a.txt"), "my a\n");
    assert_eq!(s.read("sub/c.txt"), "my c\n");
    assert!(s.sessions().is_empty());
}

/// Across file systems, moving aside copies the original and then removes
/// it. When removing fails part way, the whole copy is what's left of the
/// original: the step keeps it, the error says so, and undo puts it back.
#[cfg(target_os = "linux")]
#[test]
fn a_move_aside_that_cant_remove_the_original_keeps_the_copy_for_undo() {
    use std::os::unix::fs::PermissionsExt;
    if restoric::repo::is_root() {
        eprintln!("skipped: root can remove anything");
        return;
    }
    let Some(s) = across_devices() else { return };
    let locked = s.root.join("sub/locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("f"), "keep\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

    let err = s.overwrite(&["sub"]).unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("moving") && text.contains("kept for undo"),
        "{text}"
    );
    let session = s.sessions().pop().expect("the step is kept");
    assert_eq!(
        fs::read_to_string(session.join("files/0/sub/c.txt")).unwrap(),
        "my c\n"
    );
    assert!(!session.join("files/0/sub.part").exists());

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(undo(&s.places).unwrap().back, [s.root.join("sub")]);
    assert_eq!(s.read("sub/c.txt"), "my c\n");
    assert_eq!(s.read("sub/locked/f"), "keep\n");
    assert!(undo(&s.places).is_err(), "nothing left to undo");
}

/// An overwrite of a link that isn't on disk: nothing of the user's is
/// moved aside, and the link is made by the restore itself. An empty file
/// something else put at the name in the meantime fails the restore and
/// stays, and the failed step leaves nothing to undo.
#[cfg(unix)]
#[test]
fn a_failed_overwrite_leaves_what_something_else_put_at_a_free_name() {
    let s = setup();
    let planted = s.root.join("cur");
    let plant = planted.clone();
    let err = run(
        &s.repo,
        &[s.target("cur")],
        &How::Overwrite,
        &s.places,
        &mut |p| {
            if matches!(p.step, RestoreStep::Preparing) {
                fs::write(&plant, "").unwrap();
            }
        },
        &Cancel::default(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("cur"), "{err:#}");
    let m = planted.symlink_metadata().expect("the planted file stays");
    assert!(m.is_file(), "the planted file stays a file");
    assert!(
        s.sessions().is_empty(),
        "nothing was overwritten: {:?}",
        s.sessions()
    );
    assert!(undo(&s.places).is_err(), "nothing to undo");
}

/// A path that can't be made, because its parent is a file: nothing of
/// the user's was at it, so there is nothing to put back, and the failed
/// step leaves no undo step behind that every later undo would trip on.
#[test]
fn a_path_that_cant_be_claimed_leaves_no_undo_step() {
    let s = setup();
    fs::write(s.root.join("new"), "a file, not a folder\n").unwrap();
    let mut t = s.target("a.txt");
    t.path = s.root.join("new/a.txt");
    let err = run(
        &s.repo,
        &[t],
        &How::Overwrite,
        &s.places,
        &mut |_| {},
        &Cancel::default(),
    )
    .unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("new/a.txt"), "{text}");
    assert!(
        !text.contains("putting it back"),
        "nothing to put back: {text}"
    );
    assert_eq!(s.read("new"), "a file, not a folder\n");
    assert!(s.sessions().is_empty(), "{:?}", s.sessions());
    let err = undo(&s.places).unwrap_err();
    assert!(format!("{err:#}").contains("Nothing to undo"), "{err:#}");
}

/// Across file systems an undo copies the old version back, then removes
/// the copy. A copy that can't be removed stays in the undo folder, and
/// the undo says so: the user's files are back, and the old copies are
/// still taking up room.
#[cfg(target_os = "linux")]
#[test]
fn an_undo_says_which_old_copies_it_couldnt_remove() {
    use std::os::unix::fs::PermissionsExt;
    if restoric::repo::is_root() {
        eprintln!("skipped: root can remove anything");
        return;
    }
    let Some(s) = across_devices() else { return };
    fs::create_dir(s.root.join("sub/locked")).unwrap();
    fs::write(s.root.join("sub/locked/f"), "keep\n").unwrap();
    s.overwrite(&["a.txt", "sub"]).unwrap();
    let session = s.sessions().pop().unwrap();
    let copy = session.join("files/1/sub");
    let locked = copy.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

    let undone = undo(&s.places).unwrap();
    assert_eq!(undone.back, [s.root.join("a.txt"), s.root.join("sub")]);
    assert_eq!(s.read("a.txt"), "my a\n");
    assert_eq!(s.read("sub/c.txt"), "my c\n");
    assert_eq!(s.read("sub/locked/f"), "keep\n");
    assert_eq!(undone.left.len(), 1, "{:?}", undone.left);
    assert_eq!(undone.left[0].0, copy);
    assert!(
        copy.join("locked/f").exists(),
        "the copy stays until it can be removed"
    );

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(s.root.join("sub/locked"), fs::Permissions::from_mode(0o755)).unwrap();
}

/// A step whose manifest names a path nothing was at, from a process that
/// died before the restore claimed the name, leaves whatever is at the
/// path now: the restore never put anything there, so it's the user's.
#[test]
fn undo_leaves_a_path_alone_when_the_restore_never_claimed_it() {
    let s = setup();
    let undo_dir = s.places.undo_dir.clone().unwrap();
    let session = undo_dir.join("20260701T100000.000");
    fs::create_dir_all(&session).unwrap();
    let manifest = serde_json::json!({
        "entries": [{ "path": s.root.join("new.txt"), "saved": null }]
    });
    fs::write(session.join("manifest.json"), manifest.to_string()).unwrap();
    fs::write(s.root.join("new.txt"), "made by hand later\n").unwrap();

    let undone = undo(&s.places).unwrap();
    assert_eq!(s.read("new.txt"), "made by hand later\n");
    assert!(undone.back.is_empty(), "{:?}", undone.back);
    assert!(s.sessions().is_empty(), "the step is finished with");
}

/// What a restore created at a free name is removed by undo, and counts
/// as put back only if it was still there.
#[test]
fn undo_removes_what_the_restore_created() {
    let s = setup();
    assert!(!s.root.join("cur").exists());
    s.overwrite(&["cur"]).unwrap();
    assert_eq!(
        fs::read_link(s.root.join("cur")).unwrap(),
        PathBuf::from("a.txt")
    );
    let undone = undo(&s.places).unwrap();
    assert_eq!(undone.back, [s.root.join("cur")]);
    assert!(s.root.join("cur").symlink_metadata().is_err());

    s.overwrite(&["cur"]).unwrap();
    fs::remove_file(s.root.join("cur")).unwrap();
    let undone = undo(&s.places).unwrap();
    assert!(undone.back.is_empty(), "{:?}", undone.back);
    assert!(s.sessions().is_empty());
}

/// A stop whose rollback puts everything back is a stop, not a failure,
/// even when an old copy couldn't be removed afterwards: the outcome
/// says which copies stay.
#[cfg(target_os = "linux")]
#[test]
fn a_stopped_overwrite_reports_the_old_copies_it_couldnt_remove() {
    use std::os::unix::fs::PermissionsExt;
    if restoric::repo::is_root() {
        eprintln!("skipped: root can remove anything");
        return;
    }
    let Some(s) = across_devices() else { return };
    fs::create_dir(s.root.join("sub/locked")).unwrap();
    fs::write(s.root.join("sub/locked/f"), "keep\n").unwrap();
    let items = [s.target("a.txt"), s.target("sub")];
    let cancel = Cancel::default();
    let stop = cancel.clone();
    let mut locked = None;
    let o = run(
        &s.repo,
        &items,
        &How::Overwrite,
        &s.places,
        &mut |p| {
            // `sub` has been moved aside by now; lock a folder in the copy
            // so that it can't be removed after it's put back.
            if p.item == 1 && locked.is_none() {
                let session = s.sessions().pop().unwrap();
                let l = session.join("files/1/sub/locked");
                fs::set_permissions(&l, fs::Permissions::from_mode(0o555)).unwrap();
                locked = Some(l);
                stop.cancel();
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(o.stopped);
    assert!(o.done.is_empty());
    assert_eq!(s.read("a.txt"), "my a\n");
    assert_eq!(s.read("sub/c.txt"), "my c\n");
    assert_eq!(s.read("sub/locked/f"), "keep\n");
    let locked = locked.unwrap();
    assert_eq!(o.left.len(), 1, "{:?}", o.left);
    assert_eq!(o.left[0].0, locked.parent().unwrap());
    assert!(
        locked.join("f").exists(),
        "the copy stays until it can be removed"
    );

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(s.root.join("sub/locked"), fs::Permissions::from_mode(0o755)).unwrap();
}
