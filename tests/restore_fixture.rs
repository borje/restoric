//! Restoring from the repository restic wrote (tests/fixtures/make_repo.sh),
//! with every option, into the fixture's folder on disk.
//! One test, in order: the steps change what's on disk.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jiff::tz::TimeZone;

use common::fixture::fixture;
use restoric::repo::{Repo, RestoreStep, SnapshotInfo, is_stopped};
use restoric::restore::{Done, How, Places, Target, planned, run, stamp, undo};
use restoric::worker::Cancel;

fn target(repo: &dyn Repo, snap: &SnapshotInfo, path: &Path) -> Target {
    let mut tree = repo.tree(&snap.tree).unwrap();
    let mut node = None;
    for c in path.components().skip(1) {
        let n = tree.get(c.as_os_str()).unwrap().clone();
        if let Some(sub) = n.subtree {
            tree = repo.tree(&sub).unwrap();
        }
        node = Some(n);
    }
    Target {
        snapshot: snap.clone(),
        path: path.to_path_buf(),
        node: node.unwrap(),
    }
}

/// Runs a restore to the end, without watching its progress.
fn go(repo: &dyn Repo, targets: &[Target], how: How, places: &Places) -> anyhow::Result<Vec<Done>> {
    let o = run(repo, targets, &how, places, &mut |_| {}, &Cancel::default())?;
    assert!(!o.stopped);
    Ok(o.done)
}

#[test]
fn every_option() {
    let Some(f) = fixture() else { return };
    let repo = f.open();
    let mut snaps = repo.snapshots().unwrap();
    snaps.sort_by_key(|s| s.time);
    let src = f.src();
    let places = Places {
        restore_dir: f.root.join("Restored"),
        undo_dir: f.root.join("undo"),
        tz: TimeZone::UTC,
    };
    let st0 = stamp(snaps[0].time, &TimeZone::UTC);
    let a = src.join("a.txt");
    assert_eq!(fs::read(&a).unwrap(), b"alpha 2\n");

    // Next to the original, twice: name.stamp, then name.stamp-2.
    let t = target(&repo, &snaps[0], &a);
    assert_eq!(
        planned(&t, &How::NextTo, &places),
        src.join(format!("a.txt.{st0}"))
    );
    let done = go(&repo, std::slice::from_ref(&t), How::NextTo, &places).unwrap();
    assert_eq!(done[0].dest, src.join(format!("a.txt.{st0}")));
    assert_eq!(fs::read(&done[0].dest).unwrap(), b"alpha\n");
    let done = go(&repo, std::slice::from_ref(&t), How::NextTo, &places).unwrap();
    assert_eq!(done[0].dest, src.join(format!("a.txt.{st0}-2")));

    // Missing on disk: next to it means back in its place.
    let d = src.join("sub/d.txt");
    assert!(!d.exists());
    let done = go(&repo, &[target(&repo, &snaps[0], &d)], How::NextTo, &places).unwrap();
    assert_eq!(done[0].dest, d);
    assert_eq!(fs::read(&d).unwrap(), b"delta\n");

    // Permissions are kept (a.txt is 600 from snapshot 4 on).
    let t4 = target(&repo, &snaps[4], &a);
    let done = go(&repo, &[t4], How::NextTo, &places).unwrap();
    let mode = fs::metadata(&done[0].dest).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    // A folder into the restore folder.
    let sub = target(&repo, &snaps[0], &src.join("sub"));
    let done = go(&repo, std::slice::from_ref(&sub), How::RestoreDir, &places).unwrap();
    let dir = places.restore_dir.join(&st0).join("sub");
    assert_eq!(done[0].dest, dir);
    assert_eq!(fs::read(dir.join("c.txt")).unwrap(), b"charlie\n");
    assert_eq!(fs::read(dir.join("d.txt")).unwrap(), b"delta\n");

    // A folder as a tar archive next to it.
    let done = go(&repo, &[sub], How::Tar, &places).unwrap();
    assert_eq!(done[0].dest, src.join(format!("sub-{st0}.tar")));
    let mut ar = tar::Archive::new(fs::File::open(&done[0].dest).unwrap());
    let mut entries: Vec<(String, String)> = ar
        .entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let p = e.path().unwrap().to_string_lossy().into_owned();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut e, &mut s).unwrap();
            (p, s)
        })
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        [
            ("sub".into(), String::new()),
            ("sub/c.txt".into(), "charlie\n".into()),
            ("sub/d.txt".into(), "delta\n".into()),
        ]
    );

    // Overwrite a file, then undo: the original bytes come back.
    go(&repo, std::slice::from_ref(&t), How::Overwrite, &places).unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"alpha\n");
    assert_eq!(undo(&places).unwrap(), std::slice::from_ref(&a));
    assert_eq!(fs::read(&a).unwrap(), b"alpha 2\n");
    assert!(undo(&places).is_err(), "nothing left to undo");

    // Overwrite a folder and a file at once, then undo both.
    let before_c = fs::read(src.join("sub/c.txt")).unwrap();
    fs::write(src.join("sub/new.txt"), b"made after the backup").unwrap();
    let items = [
        target(&repo, &snaps[0], &src.join("sub")),
        target(&repo, &snaps[0], &src.join("keep.txt")),
    ];
    go(&repo, &items, How::Overwrite, &places).unwrap();
    assert!(!src.join("sub/new.txt").exists());
    assert_eq!(fs::read(src.join("keep.txt")).unwrap(), b"keep\n");
    undo(&places).unwrap();
    assert_eq!(
        fs::read(src.join("sub/new.txt")).unwrap(),
        b"made after the backup"
    );
    assert_eq!(fs::read(src.join("sub/c.txt")).unwrap(), before_c);
    assert!(
        !src.join("keep.txt").exists(),
        "keep.txt wasn't on disk before"
    );

    // Progress: preparing, then bytes up to the folder's size.
    let sub = target(&repo, &snaps[0], &src.join("sub"));
    let mut steps = Vec::new();
    let o = run(
        &repo,
        std::slice::from_ref(&sub),
        &How::RestoreDir,
        &places,
        &mut |p| steps.push(p.step),
        &Cancel::default(),
    )
    .unwrap();
    assert!(!o.stopped);
    assert_eq!(steps.first(), Some(&RestoreStep::Preparing));
    let size = fs::read(src.join("sub/c.txt")).unwrap().len() + b"delta\n".len();
    assert_eq!(
        steps.last(),
        Some(&RestoreStep::Bytes {
            done: size as u64,
            total: size as u64
        })
    );

    // Stopped before the contents are copied: nothing is left at the destination.
    let dest = f.root.join("stopped");
    let e = repo
        .restore(&snaps[0], &sub.path, &dest, &mut |_| {}, &|| true)
        .unwrap_err();
    assert!(is_stopped(&e));
    assert!(!dest.exists());
}
