//! Select, yank, paste and the restore dialog (PLAN.md §3.5, §3.11).

mod common;

use std::path::PathBuf;

use ratatui::crossterm::event::KeyCode;

use common::{Harness, SRC};
use restoric::app::Effect;

#[test]
fn selection_05() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("  ");
    assert_eq!(h.app.marks.len(), 2);
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn yank_toast_06() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("  y");
    assert!(h.app.marks.is_empty());
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn confirm_overwrite_07() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("  yP");
    insta::assert_snapshot!(h.screen(100, 34));
    h.keys("n");
    assert!(h.app.confirm.is_none());
}

#[test]
fn restore_dialog_15() {
    let mut h = Harness::new(SRC);
    h.at("2026-08-17 16:04").select("main.go").keys("dr");
    insta::assert_snapshot!(h.screen(100, 34));
    h.keys("j");
    assert_eq!(h.app.dialog.as_ref().unwrap().sel, 2);
    h.key(KeyCode::Esc);
    assert!(h.app.dialog.is_none());
}

#[test]
fn visual_mode_and_escape() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("vjj");
    assert!(h.app.visual.is_some());
    insta::assert_snapshot!(h.screen(100, 12));
    h.keys("v");
    assert_eq!(h.app.marks.len(), 3);
    h.key(KeyCode::Esc);
    assert!(h.app.marks.is_empty());
}

#[test]
fn copy_and_messages() {
    let mut h = Harness::new(SRC);
    h.keys("p");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Nothing yanked. Press y on a file first.")
    );
    h.at("2026-09-09 19:23").select("util.go").keys("cc");
    let id = h.app.set()[h.app.idx()].id.0.short();
    let want = format!("{id}:/home/bege/dev/project/src/util.go");
    assert_eq!(h.app.effects, [Effect::Clipboard(want.clone())]);
    assert_eq!(h.app.message, Some(format!("Copied {want}")));
    h.keys("cf");
    assert_eq!(h.app.effects[1], Effect::Clipboard("util.go".into()));
}

#[test]
fn command_line() {
    let mut h = Harness::new(SRC);
    h.keys(":fo");
    insta::assert_snapshot!(h.screen(100, 12));
    h.key(KeyCode::Enter);
    assert_eq!(
        h.app.message.as_deref(),
        Some("Unknown command \":fo\". Try :sep 1, :yesterday, :3d, :find NAME, :undo")
    );
}

/// The whole round trip on a real folder: p, P, :undo.
#[test]
fn paste_overwrite_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let dsl = format!(
        "root {}\nsnapshot 2026-07-01 10:00\n  write a.txt one\\n\nsnapshot 2026-07-02 10:00\n  write a.txt two\\n\n",
        root.display()
    );
    let mut h = Harness::with(&dsl, root.to_str().unwrap());
    let places = restoric::restore::Places {
        restore_dir: dir.path().join("Restored"),
        undo_dir: dir.path().join("undo"),
        tz: jiff::tz::TimeZone::UTC,
    };
    h.ctx.places = places.clone();
    h.app.places = places;
    let a = root.join("a.txt");

    // Not on disk yet: p puts it back in its place.
    h.select("a.txt").keys("yp");
    assert_eq!(std::fs::read(&a).unwrap(), b"two\n");
    assert_eq!(
        h.app.message,
        Some(format!("Restored a.txt to {}", a.display()))
    );
    // Now it's there: p restores next to it.
    h.keys("p");
    let next = PathBuf::from(format!("{}.2026-07-02_1000", a.display()));
    assert_eq!(std::fs::read(&next).unwrap(), b"two\n");
    // P overwrites with the older version after confirming; :undo puts it back.
    std::fs::write(&a, b"edited\n").unwrap();
    h.keys("H").select("a.txt").keys("yPy");
    assert_eq!(std::fs::read(&a).unwrap(), b"one\n");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Overwrote a.txt. :undo puts the old version back.")
    );
    h.keys(":undo").key(KeyCode::Enter);
    assert_eq!(std::fs::read(&a).unwrap(), b"edited\n");
    assert_eq!(h.app.message.as_deref(), Some("Put back a.txt."));
    h.keys(":undo").key(KeyCode::Enter);
    assert_eq!(h.app.message.as_deref(), Some("Nothing to undo."));
}
