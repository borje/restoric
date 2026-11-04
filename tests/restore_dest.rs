//! Where a restore writes: a destination claimed first, so that nothing
//! slipped in at a predictable name is written through, and no restore
//! folder relative to wherever restoric was started.

use std::fs;
use std::path::PathBuf;

use jiff::tz::TimeZone;

use restoric::repo::fake::FakeRepo;
use restoric::repo::{Repo, SnapshotInfo, check_dest};
use restoric::restore::{How, Places, Target, run};
use restoric::worker::Cancel;

struct Setup {
    dir: tempfile::TempDir,
    root: PathBuf,
    repo: FakeRepo,
    snap: SnapshotInfo,
    places: Places,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    fs::create_dir(&root).unwrap();
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
        dir,
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

    fn restore(&self, name: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        let t = self.target(name);
        self.repo
            .restore(&self.snap, &t.path, dest, &mut |_| {}, &|| false)
    }
}

/// A name claimed by an empty file or folder is filled in; anything else
/// at the name, a link above all, stops the restore before it writes.
#[test]
fn a_restore_fills_a_claimed_name_and_refuses_any_other() {
    let s = setup();
    let out = s.dir.path().join("out");
    fs::create_dir(&out).unwrap();

    let file = out.join("a.txt");
    fs::File::create_new(&file).unwrap();
    s.restore("a.txt", &file).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"alpha\n");

    let dir = out.join("sub");
    fs::create_dir(&dir).unwrap();
    s.restore("sub", &dir).unwrap();
    assert_eq!(fs::read(dir.join("c.txt")).unwrap(), b"charlie\n");

    let victim = out.join("victim");
    fs::write(&victim, "mine\n").unwrap();
    let link = out.join("a.txt.2026-07-01_1000");
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let err = s.restore("a.txt", &link).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err:#}");
    assert_eq!(fs::read(&victim).unwrap(), b"mine\n");

    let full = out.join("full.txt");
    fs::write(&full, "x").unwrap();
    assert!(s.restore("a.txt", &full).is_err());
    assert_eq!(fs::read(&full).unwrap(), b"x");

    let taken = out.join("taken");
    fs::create_dir(&taken).unwrap();
    fs::write(taken.join("keep"), "").unwrap();
    assert!(s.restore("sub", &taken).is_err());
    assert!(!taken.join("c.txt").exists());
}

#[test]
fn check_dest_takes_only_an_empty_file_or_folder() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    assert!(check_dest(&p.join("absent")).is_ok());
    fs::File::create_new(p.join("empty")).unwrap();
    assert!(check_dest(&p.join("empty")).is_ok());
    fs::create_dir(p.join("emptydir")).unwrap();
    assert!(check_dest(&p.join("emptydir")).is_ok());
    fs::write(p.join("full"), "x").unwrap();
    assert!(check_dest(&p.join("full")).is_err());
    std::os::unix::fs::symlink(p.join("empty"), p.join("link")).unwrap();
    assert!(
        check_dest(&p.join("link")).is_err(),
        "a link to an empty file"
    );
    fs::create_dir(p.join("fulldir")).unwrap();
    fs::write(p.join("fulldir/x"), "").unwrap();
    assert!(check_dest(&p.join("fulldir")).is_err());
}

/// Restores next to the original and into the restore folder go through
/// a claimed name and come out right.
#[test]
fn restores_through_a_claimed_name_come_out_right() {
    let s = setup();
    let o = run(
        &s.repo,
        &[s.target("a.txt"), s.target("sub")],
        &How::RestoreDir,
        &s.places,
        &mut |_| {},
        &Cancel::default(),
    )
    .unwrap();
    assert_eq!(o.done.len(), 2);
    let day = s.places.restore_dir.join("2026-07-01_1000");
    assert_eq!(fs::read(day.join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(fs::read(day.join("sub/c.txt")).unwrap(), b"charlie\n");
    // Nothing on disk: next to it means its own place.
    let o = run(
        &s.repo,
        &[s.target("sub")],
        &How::NextTo,
        &s.places,
        &mut |_| {},
        &Cancel::default(),
    )
    .unwrap();
    assert_eq!(o.done[0].dest, s.root.join("sub"));
    assert_eq!(fs::read(s.root.join("sub/c.txt")).unwrap(), b"charlie\n");
}

/// Without a home folder the default restore folder would be relative to
/// the working directory; a restore there is refused instead.
#[test]
fn a_relative_restore_folder_is_refused() {
    let mut s = setup();
    s.places.restore_dir = PathBuf::from("Restored");
    let err = run(
        &s.repo,
        &[s.target("a.txt")],
        &How::RestoreDir,
        &s.places,
        &mut |_| {},
        &Cancel::default(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("restore folder"), "{err:#}");
    assert!(!std::path::Path::new("Restored").exists());
    assert!(!std::path::Path::new("Restored/2026-07-01_1000").exists());
}
