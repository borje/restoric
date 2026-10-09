//! Select, yank, paste and the restore dialog.

mod common;

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use common::{Harness, SRC};
use restoric::app::Effect;
use restoric::repo::RestoreStep;
use restoric::restore::{Done, How, Progress};
use restoric::worker::{Request, Response};

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
fn o_shows_a_file_and_the_dialog_has_three_options() {
    let mut h = Harness::new(SRC);
    h.at("2026-08-17 16:04").select("main.go").keys("o");
    assert!(matches!(&h.app.effects[..], [Effect::Pager { name, .. }] if name == "main.go"));
    h.app.effects.clear();
    h.select("api").keys("o");
    assert!(h.app.effects.is_empty());
    assert_eq!(
        h.app.message.as_deref(),
        Some("Select a file to show. o shows a file in $PAGER.")
    );
    // A file: three options, so 4 and a third j wrap to the first.
    h.select("main.go").keys("r4");
    assert_eq!(h.app.dialog.as_ref().unwrap().sel, 1);
    h.keys("jj");
    assert_eq!(h.app.dialog.as_ref().unwrap().sel, 0);
    h.key(KeyCode::Esc);
    // A folder: the tar archive is the fourth.
    h.select("api").keys("r4");
    assert_eq!(h.app.dialog.as_ref().unwrap().sel, 3);
    h.key(KeyCode::Esc);
    // The versions view shows the selected version.
    h.select("main.go").keys("lo");
    assert!(matches!(&h.app.effects[..], [Effect::Pager { name, .. }] if name == "main.go"));
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

/// Presses `keys` and keeps the restore they start from running: it stays
/// "in progress" until the test applies a response. Nothing is written.
fn start_held(h: &mut Harness, keys: &str) {
    for c in keys.chars() {
        h.app
            .key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    h.app
        .outbox
        .retain(|r| !matches!(r, Request::Restore { .. }));
    h.pump();
}

fn progress(item: usize, items: usize, name: &str, step: RestoreStep) -> Response {
    Response::RestoreProgress(Progress {
        item,
        items,
        name: name.into(),
        step,
    })
}

fn status(h: &mut Harness, cols: u16) -> String {
    h.screen(cols, 34).lines().last().unwrap().to_string()
}

#[test]
fn restore_progress_in_the_status_bar() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("  y");
    start_held(&mut h, "p");
    assert_eq!(h.app.restoring.as_ref().unwrap().items, 2);
    let mut lines = vec![status(&mut h, 100)];
    h.app
        .apply(progress(0, 2, "config.go", RestoreStep::Preparing));
    lines.push(status(&mut h, 100));
    let mid = RestoreStep::Bytes {
        done: 3 << 20,
        total: 8 << 20,
    };
    h.app.apply(progress(1, 2, "main.go", mid));
    lines.push(status(&mut h, 100));
    lines.push(status(&mut h, 80));
    h.app.apply(progress(0, 1, "api/", mid));
    lines.push(status(&mut h, 100));
    insta::assert_snapshot!(lines.join("\n"));

    // It shows in every view: here the versions view.
    h.keys("l");
    assert!(matches!(h.app.view, restoric::app::View::Versions(_)));
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn stop_restore_18() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("y");
    start_held(&mut h, "p");
    h.app.apply(progress(
        0,
        1,
        "config.go",
        RestoreStep::Bytes { done: 1, total: 4 },
    ));
    // Esc clears the selection first.
    h.keys(" ");
    assert_eq!(h.app.marks.len(), 1);
    h.key(KeyCode::Esc);
    assert!(h.app.marks.is_empty() && h.app.confirm.is_none());
    // Then asks.
    h.key(KeyCode::Esc);
    insta::assert_snapshot!(h.screen(100, 34));
    h.keys("n");
    assert!(h.app.confirm.is_none());
    let cancel = h.app.restoring.as_ref().unwrap().cancel.clone();
    assert!(!cancel.cancelled());
    h.key(KeyCode::Esc).keys("y");
    assert!(cancel.cancelled());
    assert!(h.app.restoring.as_ref().unwrap().stopping);
    insta::assert_snapshot!(status(&mut h, 100));
    // A second esc doesn't ask again.
    h.key(KeyCode::Esc);
    assert!(h.app.confirm.is_none());
    h.app.apply(Response::Restored {
        how: How::NextTo,
        done: Vec::new(),
        stopped: true,
    });
    assert!(h.app.restoring.is_none());
    assert_eq!(
        h.app.message.as_deref(),
        Some("Stopped · nothing was restored")
    );
}

#[test]
fn one_restore_at_a_time_and_cancel() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("  y");
    start_held(&mut h, "p");
    let busy = Some("A restore is running · esc to stop");
    for keys in ["p", "P", "r"] {
        h.keys(keys);
        assert_eq!(h.app.message.as_deref(), busy, "{keys}");
        assert!(h.app.confirm.is_none() && h.app.dialog.is_none());
    }
    h.keys(":undo").key(KeyCode::Enter);
    assert_eq!(h.app.message.as_deref(), busy);
    // :cancel doesn't ask.
    h.keys(":cancel").key(KeyCode::Enter);
    assert!(h.app.confirm.is_none());
    assert!(h.app.restoring.as_ref().unwrap().cancel.cancelled());
    h.app.apply(Response::Restored {
        how: How::NextTo,
        done: vec![Done {
            target: PathBuf::from(SRC).join("config.go"),
            dest: PathBuf::from(SRC).join("config.go.2026-09-09_1923"),
        }],
        stopped: true,
    });
    assert_eq!(h.app.message.as_deref(), Some("Stopped · restored 1 of 2"));
    h.keys(":cancel").key(KeyCode::Enter);
    assert_eq!(h.app.message.as_deref(), Some("No restore is running."));
}

#[test]
fn a_restore_that_ends_closes_the_stop_popup() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("y");
    start_held(&mut h, "p");
    h.key(KeyCode::Esc);
    assert!(h.app.confirm.is_some());
    h.app.apply(Response::Restored {
        how: How::NextTo,
        done: vec![Done {
            target: PathBuf::from(SRC).join("config.go"),
            dest: PathBuf::from(SRC).join("config.go.2026-09-09_1923"),
        }],
        stopped: false,
    });
    assert!(h.app.confirm.is_none());
    assert_eq!(
        h.app.message.as_deref(),
        Some("Restored as config.go.2026-09-09_1923")
    );
}

#[test]
fn going_back_to_the_picker_during_a_restore_asks() {
    let mut h = Harness::new(SRC);
    h.app.from_picker = true;
    h.at("2026-09-09 19:23").select("config.go").keys("yP");
    start_held(&mut h, "y");
    h.keys("q");
    assert!(!h.app.quit && h.app.to_picker);
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn quitting_during_a_restore_asks_and_waits() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("config.go").keys("yP");
    start_held(&mut h, "y");
    assert_eq!(h.app.restoring.as_ref().unwrap().how, How::Overwrite);
    h.keys("q");
    assert!(!h.app.quit);
    insta::assert_snapshot!(h.screen(100, 34));
    h.keys("y");
    assert!(!h.app.quit, "waits for the restore to stop");
    assert!(h.app.restoring.as_ref().unwrap().cancel.cancelled());
    h.app.apply(Response::Restored {
        how: How::Overwrite,
        done: Vec::new(),
        stopped: true,
    });
    assert!(h.app.quit);
    assert_eq!(
        h.app.message.as_deref(),
        Some("Stopped · nothing was overwritten")
    );
}
