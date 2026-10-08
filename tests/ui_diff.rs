//! Snapshot tests of the full-screen diff (PLAN.md §3.10).

mod common;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use common::{Harness, SRC};
use restoric::app::{DiffMode, View};

fn diff_view(h: &Harness) -> restoric::app::DiffView {
    match &h.app.view {
        View::Diff(d) => d.clone(),
        v => panic!("not in the diff view: {v:?}"),
    }
}

#[test]
fn diff_against_disk_14() {
    let mut h = Harness::new(SRC);
    h.at("2026-08-17 16:04").select("main.go").keys("d");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn diff_against_previous() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go").keys("lp");
    assert_eq!(diff_view(&h).mode, DiffMode::Previous);
    insta::assert_snapshot!(h.screen(100, 34));
}

/// A 400-line file with every 20th line changed: 20 hunks.
fn long_file() -> String {
    let v1: Vec<String> = (1..=400).map(|n| format!("line {n}")).collect();
    let mut v2 = v1.clone();
    for k in (10..400).step_by(20) {
        v2[k] = format!("line {} changed", k + 1);
    }
    format!(
        "root /home/bege/data\nsnapshot 2026-07-01 10:00\n  write long.txt {}\\n\n\
         snapshot 2026-07-02 10:00\n  write long.txt {}\\n\n",
        v1.join("\\n"),
        v2.join("\\n")
    )
}

#[test]
fn hunk_keys() {
    let mut h = Harness::with(&long_file(), "/home/bege/data");
    h.select("long.txt").keys("lp");
    h.screen(100, 34);
    assert_eq!(diff_view(&h).scroll, 0);
    h.keys("]c");
    let second = diff_view(&h).scroll;
    assert!(second > 0);
    h.keys("n");
    assert!(diff_view(&h).scroll > second);
    h.keys("99n");
    assert_eq!(h.app.message.as_deref(), Some("No more changes below."));
    h.keys("G[c[c");
    assert!(diff_view(&h).scroll > 0);
    h.keys("gg]c[c");
    assert_eq!(diff_view(&h).scroll, 0);
    h.keys("N");
    assert_eq!(h.app.message.as_deref(), Some("No more changes above."));
}

#[test]
fn keys_move_between_versions() {
    let mut h = Harness::new(SRC);
    h.at("2026-07-14 09:00").select("main.go").keys("d");
    let first = diff_view(&h).run;
    h.keys("H");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the oldest version.")
    );
    h.keys("L");
    assert_eq!(diff_view(&h).run, first - 1);
    h.keys("99L");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the newest saved version.")
    );
    // Back to where the diff was opened from. `q` quits instead.
    h.keys("h");
    assert_eq!(h.app.view, View::Folder);
    h.keys("l").key(KeyCode::Enter);
    assert!(diff_view(&h).from_versions);
    h.key(KeyCode::Esc);
    assert!(matches!(h.app.view, View::Versions(_)));
}

const FILES: &str = "
host bege-laptop
root /home/bege/data
snapshot 2026-07-01 10:00
  write image.bin PNG\\0\\0\\1
  fill big.log 3000000
  write same.txt same\\n
snapshot 2026-07-02 10:00
  write image.bin PNG\\0\\0\\2\\3
  fill big.log 3000100
";

#[test]
fn binary_files() {
    let mut h = Harness::with(FILES, "/home/bege/data");
    h.select("image.bin").keys("lp");
    insta::assert_snapshot!(h.screen(80, 12));
}

#[test]
fn large_files_are_diffed_on_request() {
    let mut h = Harness::with(FILES, "/home/bege/data");
    h.select("big.log").keys("lp");
    insta::assert_snapshot!("large_before", h.screen(90, 12));
    h.key(KeyCode::Enter);
    insta::assert_snapshot!("large_after", h.screen(90, 12));
}

#[test]
fn identical() {
    let mut h = Harness::with(FILES, "/home/bege/data");
    h.select("same.txt").keys("d");
    insta::assert_snapshot!(h.screen(100, 10));
}

/// `q` goes back one level: diff, versions, folder, then out (§3.14).
#[test]
fn q_goes_back_one_level() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go").keys("ld");
    assert!(matches!(h.app.view, View::Diff(_)));
    h.keys("q");
    assert!(matches!(h.app.view, View::Versions(_)));
    h.keys("q");
    assert_eq!(h.app.view, View::Folder);
    assert!(!h.app.quit);
    h.keys("q");
    assert!(h.app.quit && !h.app.to_picker);
}

/// After the picker, `q` in the folder view goes back to it; `Ctrl-c`
/// and `:q` still quit (§3.18).
#[test]
fn q_after_the_picker_goes_back_to_it() {
    let mut h = Harness::new(SRC);
    h.app.from_picker = true;
    h.keys("q");
    assert!(h.app.quit && h.app.to_picker);
    let mut h = Harness::new(SRC);
    h.app.from_picker = true;
    h.app
        .key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(h.app.quit && !h.app.to_picker);
    let mut h = Harness::new(SRC);
    h.app.from_picker = true;
    h.keys(":q").key(KeyCode::Enter);
    assert!(h.app.quit && !h.app.to_picker);
}
