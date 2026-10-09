//! The `on disk` version: the files on disk as the newest
//! version, compared with the newest snapshot.

mod common;

use std::path::PathBuf;

use common::{Harness, PROJECT, SRC, drifted};
use ratatui::crossterm::event::KeyCode;
use restoric::app::{Action, DiffMode, Effect, View};
use restoric::index::Version;
use restoric::index::timeline::Filter;

/// `L` from the newest change point goes to the newest snapshot, then on
/// to disk.
fn on_disk() -> Harness {
    let mut h = Harness::with(&drifted(), SRC);
    h.keys("LL");
    assert!(h.app.on_disk());
    h
}

#[test]
fn on_disk_21() {
    let mut h = on_disk();
    h.select("main.go");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn on_disk_preview() {
    let mut h = on_disk();
    h.select("main.go").key(KeyCode::Tab);
    let s = h.screen(100, 34);
    assert!(s.contains("vs latest"), "{s}");
    assert!(s.contains("+ // unsaved edit"), "{s}");
    insta::assert_snapshot!(s);
    h.select("scratch.go");
    let s = h.screen(100, 34);
    assert!(s.contains("Not in any backup."), "{s}");
    h.key(KeyCode::Tab).select("util.go");
    let s = h.screen(100, 34);
    assert!(s.contains("util.go · deleted"), "{s}");
}

#[test]
fn on_disk_navigation() {
    let mut h = Harness::with(&drifted(), SRC);
    let newest_change = h.app.idx();
    let newest = h.app.set().len() - 1;
    h.keys("L");
    assert_eq!(h.app.idx(), newest);
    h.keys("]");
    assert!(h.app.on_disk());
    assert_eq!(h.app.message, None);
    h.keys("L");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the version on disk.")
    );
    h.keys("]");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the version on disk.")
    );
    h.keys("[");
    assert_eq!(h.app.idx(), newest);
    h.keys("L");
    assert!(h.app.on_disk());
    h.keys("H");
    assert_eq!(h.app.idx(), newest_change);
    h.act(Action::GoDisk);
    assert!(h.app.on_disk());
    h.key(KeyCode::End);
    assert_eq!(h.app.idx(), newest_change);

    // Stepping onto disk reads the folder again.
    h.act(Action::GoDisk);
    let key = (Version::Disk, PathBuf::from(SRC));
    assert!(h.app.listings.contains_key(&key));
    h.app.act(Action::GoDisk);
    assert!(!h.app.listings.contains_key(&key));
    h.pump();
    assert!(h.app.listings.contains_key(&key));

    // A subfolder opened on disk is listed from disk too.
    h.select("models").keys("l");
    assert!(h.app.on_disk());
    assert_eq!(h.app.folder, PathBuf::from(SRC).join("models"));
    let s = h.screen(100, 34);
    assert!(s.contains("draft.go"), "{s}");
    h.keys("h");
    assert!(h.app.on_disk());
}

#[test]
fn on_disk_restore_keys() {
    let mut h = on_disk();
    let refused = Some("That's the file on disk. Pick a version to restore.");
    h.select("main.go").keys("y");
    assert_eq!(h.app.message.as_deref(), refused);
    assert!(h.app.yanked.is_none());
    h.keys("r");
    assert_eq!(h.app.message.as_deref(), refused);
    assert!(h.app.dialog.is_none());
    h.select("scratch.go").keys("y");
    assert_eq!(h.app.message.as_deref(), refused);
    // A file deleted on disk restores from the newest snapshot.
    h.select("util.go").keys("y");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Yanked util.go from Oct 02 12:21. p restores next to the original, P overwrites.")
    );
    // Reading and copying work on the file on disk.
    h.select("main.go").keys("o");
    match h.app.effects.pop() {
        Some(Effect::Pager { name, bytes }) => {
            assert_eq!(name, "main.go");
            assert!(String::from_utf8_lossy(&bytes).contains("unsaved edit"));
        }
        e => panic!("no pager: {e:?}"),
    }
    h.keys("cc");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Copied /home/bege/dev/project/src/main.go")
    );
    h.keys("cf");
    assert_eq!(h.app.message.as_deref(), Some("Copied main.go"));
}

#[test]
fn on_disk_versions_view() {
    let mut h = on_disk();
    h.select("main.go").keys("l");
    match &h.app.view {
        View::Versions(v) => assert!(v.disk),
        v => panic!("{v:?}"),
    }
    insta::assert_snapshot!(h.screen(100, 34));
    h.keys("y");
    assert_eq!(
        h.app.message.as_deref(),
        Some("That's the file on disk. Pick a version to restore.")
    );
    h.keys("k");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the version on disk.")
    );
    h.keys("j");
    match &h.app.view {
        View::Versions(v) => assert!(!v.disk && v.sel == 0),
        v => panic!("{v:?}"),
    }
    h.keys("k");
    match &h.app.view {
        View::Versions(v) => assert!(v.disk),
        v => panic!("{v:?}"),
    }
    // A file never backed up has a versions view with only the on disk row.
    h.keys("h").select("scratch.go").keys("l");
    let s = h.screen(100, 34);
    assert!(s.contains("not in any backup"), "{s}");
}

#[test]
fn on_disk_diff() {
    let mut h = on_disk();
    h.select("main.go").keys("d");
    match &h.app.view {
        View::Diff(d) => assert!(d.disk && d.mode == DiffMode::Previous),
        v => panic!("{v:?}"),
    }
    insta::assert_snapshot!(h.screen(100, 34));
    h.keys("c");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Already the version on disk.")
    );
    h.keys("L");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the version on disk.")
    );
    h.keys("H");
    match &h.app.view {
        View::Diff(d) => assert!(!d.disk),
        v => panic!("{v:?}"),
    }
    h.keys("L");
    match &h.app.view {
        View::Diff(d) => assert!(d.disk),
        v => panic!("{v:?}"),
    }
    // Back lands on the versions view's on disk row.
    h.select("main.go");
    h.keys("h").keys("l").key(KeyCode::Enter);
    h.keys("h");
    match &h.app.view {
        View::Versions(v) => assert!(v.disk),
        v => panic!("{v:?}"),
    }
}

#[test]
fn on_disk_foreign_host() {
    // Another host's snapshots: this machine's disk is no version of them.
    let f = Filter {
        hosts: vec!["dev-vm".into()],
        tag: None,
    };
    let dsl = PROJECT.replace("host bege-laptop", "host dev-vm");
    let mut h = Harness::with_filter(&dsl, SRC, f);
    h.app.mine = vec!["bege-laptop".into()];
    assert!(h.app.foreign());
    let refused = Some("on disk is this machine. The snapshots are dev-vm's.");
    h.keys("LL");
    assert!(!h.app.on_disk());
    assert_eq!(h.app.message.as_deref(), refused);
    h.act(Action::GoDisk);
    assert!(!h.app.on_disk());
    // The counts stay, as settled for the picker.
    assert!(h.app.live.contains_key(&PathBuf::from(SRC)));
    // The same at the versions view's on disk row.
    h.select("main.go").keys("l").keys("k");
    assert_eq!(h.app.message.as_deref(), refused);
    match &h.app.view {
        View::Versions(v) => assert!(!v.disk),
        v => panic!("{v:?}"),
    }
}

#[test]
fn on_disk_review_fixes() {
    // `gg` in the versions view lands on the on disk row with sel 0.
    let mut h = on_disk();
    h.select("main.go").keys("l").keys("jjj").keys("gg");
    match &h.app.view {
        View::Versions(v) => assert!(v.disk && v.sel == 0, "{v:?}"),
        v => panic!("{v:?}"),
    }
    h.keys("j");
    match &h.app.view {
        View::Versions(v) => assert!(!v.disk && v.sel == 0, "{v:?}"),
        v => panic!("{v:?}"),
    }
    h.keys("h");

    // A deleted-on-disk row says so, and `⇥` isn't "vs latest" on it.
    h.select("util.go");
    let s = h.screen(100, 34);
    assert!(s.contains("last version Sep 13, not on disk"), "{s}");
    assert!(s.contains("⇥ content"), "{s}");
    h.key(KeyCode::Tab);
    let s = h.screen(100, 34);
    assert!(s.contains("⇥ vs disk"), "{s}");
    h.key(KeyCode::Tab);

    // Switching to another host's snapshots leaves the disk version.
    let dsl = format!(
        "{}snapshot 2026-10-03 10:00 host=dev-vm\n  append src/main.go // theirs\\n\n",
        drifted().replace("\ndisk\n", "\n")
    );
    let dsl = format!("{dsl}\ndisk\n  append src/main.go // unsaved edit\\n\n");
    let mut h = Harness::with(&dsl, SRC);
    h.app.mine = vec!["bege-laptop".into()];
    h.keys("LL");
    assert!(h.app.on_disk());
    h.keys(":host dev-vm").key(KeyCode::Enter);
    assert!(!h.app.on_disk());
    assert!(h.app.foreign());
}

#[test]
fn disk_listing_kind_change() {
    // A file replaced by a folder on disk keeps a `−` row for the file.
    let dsl = format!(
        "{}  rm src/util.go\n  write src/util.go/a.go package util\\n\n",
        PROJECT
    );
    let mut h = Harness::with(&dsl, SRC);
    h.keys("LL");
    assert!(h.app.on_disk());
    let rows = h.app.rows();
    let utils: Vec<_> = rows
        .iter()
        .filter_map(|r| h.app.entry(*r))
        .filter(|e| e.node.name.to_string_lossy() == "util.go")
        .map(|e| (e.is_dir(), e.is_deleted()))
        .collect();
    assert_eq!(utils, vec![(true, false), (false, true)]);
    let s = h.screen(100, 34);
    assert!(s.contains("on disk  +1~1−1"), "{s}");
}
