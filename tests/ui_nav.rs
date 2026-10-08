//! Search, filter, the command line, find and zoom (PLAN.md §3.4, §3.6,
//! §3.8, §3.15).

mod common;

use ratatui::crossterm::event::KeyCode;

use common::{Harness, SRC};
use restoric::app::View;
use restoric::ui;

fn when(h: &Harness) -> String {
    ui::fmt::time(h.app.set()[h.app.idx()].time, &jiff::tz::TimeZone::UTC)
}

fn command(h: &mut Harness, cmd: &str) {
    h.keys(":").keys(cmd).key(KeyCode::Enter);
}

#[test]
fn which_key_z_04() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("main.go").keys("z");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn command_line_08() {
    let mut h = Harness::new(SRC);
    h.keys("s").keys("legacy");
    insta::assert_snapshot!(h.screen(100, 12));
}

#[test]
fn find_09_and_jump_10() {
    let mut h = Harness::new(SRC);
    h.keys("slegacy").key(KeyCode::Enter);
    insta::assert_snapshot!("find_09", h.screen(100, 12));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.view, View::Folder);
    assert_eq!(when(&h), "Sep 05 22:40");
    assert_eq!(
        h.app.selected().unwrap().node.name.to_string_lossy(),
        "legacy.go"
    );
    insta::assert_snapshot!("find_jump_10", h.screen(100, 34));
}

#[test]
fn find_nothing_and_back() {
    let mut h = Harness::new(SRC);
    command(&mut h, "find nosuchthing");
    insta::assert_snapshot!(h.screen(100, 10));
    h.keys("h");
    assert_eq!(h.app.view, View::Folder);
}

#[test]
fn filter_12() {
    let mut h = Harness::new(SRC);
    h.keys("fco");
    insta::assert_snapshot!("filter_typing_12", h.screen(100, 12));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.rows().len(), 2, ".. and config.go");
    insta::assert_snapshot!("filter_kept", h.screen(100, 12));
    h.key(KeyCode::Esc);
    assert!(h.app.name_filter.is_empty());
    assert!(h.app.rows().len() > 2);
}

#[test]
fn search_and_next() {
    let mut h = Harness::new(SRC);
    h.keys("n");
    assert_eq!(
        h.app.message.as_deref(),
        Some("No search yet. Press / to search this folder.")
    );
    h.keys("/.go");
    insta::assert_snapshot!(h.screen(100, 14));
    h.key(KeyCode::Enter);
    let first = h.app.sel;
    h.keys("n");
    assert!(h.app.sel > first);
    h.keys("N");
    assert_eq!(h.app.sel, first);
    h.keys("/zzz").key(KeyCode::Enter);
    assert_eq!(
        h.app.message.as_deref(),
        Some("No match for \"zzz\" in this folder. Press s to search every snapshot.")
    );
    // esc while typing goes back to where it was.
    h.keys("/mod").key(KeyCode::Esc);
    assert_eq!(h.app.search, "zzz");
}

#[test]
fn arrows_are_hjkl_and_q_goes_back() {
    let mut h = Harness::new(SRC);
    let start = when(&h);
    // Plain arrows move in the tree, not in time.
    h.select("api").key(KeyCode::Right);
    assert!(h.app.folder.ends_with("api"));
    assert_eq!(when(&h), start);
    h.key(KeyCode::Left);
    assert!(h.app.folder.ends_with("src"));
    assert_eq!(when(&h), start);
    // Sub-views go back with h, esc, backspace and q.
    h.select("main.go").keys("l");
    assert!(matches!(h.app.view, View::Versions(_)));
    h.key(KeyCode::Left);
    assert_eq!(h.app.view, View::Folder);
    h.keys("lq");
    assert_eq!(h.app.view, View::Folder);
    assert!(!h.app.quit);
}

#[test]
fn dates() {
    let mut h = Harness::new(SRC);
    command(&mut h, "sep 1");
    assert_eq!(when(&h), "Aug 28 13:10");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Jumped to Aug 28 13:10, the last snapshot on or before Sep 01.")
    );
    command(&mut h, "2026-07-24");
    assert_eq!(when(&h), "Jul 24 12:16");
    command(&mut h, "09-13");
    assert_eq!(when(&h), "Sep 13 20:51");
    command(&mut h, "yesterday");
    assert_eq!(when(&h), "Oct 02 12:21");
    command(&mut h, "2w");
    assert_eq!(when(&h), "Sep 20 20:35");
    command(&mut h, "jan 1");
    assert_eq!(
        h.app.message.as_deref(),
        Some("No snapshots that early. The oldest is from Jul 14 09:00.")
    );
    command(&mut h, "oldest");
    assert_eq!(when(&h), "Jul 14 09:00");
    command(&mut h, "latest");
    assert_eq!(when(&h), "Oct 02 12:21");
}

#[test]
fn zoom() {
    let mut h = Harness::new(SRC);
    h.keys("zi");
    assert_eq!(h.app.zoom, 2);
    h.keys("zizi");
    insta::assert_snapshot!(h.screen(100, 12));
    h.keys("zozozo");
    assert_eq!(h.app.zoom, 1);
    h.keys("zo");
    assert_eq!(h.app.message.as_deref(), Some("Fully zoomed out."));
}

#[test]
fn host_tag_and_strict() {
    let mut h = Harness::new(SRC);
    let all = h.app.set().len();
    command(&mut h, "host nobody");
    assert!(
        h.app
            .message
            .as_ref()
            .unwrap()
            .starts_with("No snapshots from this machine")
    );
    assert_eq!(h.app.set().len(), all);
    command(&mut h, "host bege-laptop");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Showing snapshots from bege-laptop.")
    );
    assert_eq!(h.app.set().len(), all);

    // Strict counts the touch-only snapshots of util.go and main.go.
    let versions = h.app.state().unwrap().versions().len();
    command(&mut h, "set strict");
    let strict = h.app.state().unwrap().versions().len();
    assert!(strict > versions, "{strict} > {versions}");
    command(&mut h, "set nostrict");
    assert_eq!(h.app.state().unwrap().versions().len(), versions);
}

#[test]
fn reload() {
    let mut h = Harness::new(SRC);
    command(&mut h, "reload");
    assert_eq!(h.app.message.as_deref(), Some("No new snapshots."));
}

/// macOS stores ä as a + U+0308. It must be drawn with its accent, and an ä
/// typed as one char must find it in search, filter and find.
#[test]
fn decomposed_names() {
    let dsl = "host bege-laptop\nroot /home/bege/data\nsnapshot 2026-07-01 10:00\n  write ha\u{308}lsa.pdf x\n  write other.txt y\n";
    let mut h = Harness::with(dsl, "/home/bege/data");
    assert!(h.screen(100, 12).contains("ha\u{308}lsa.pdf"));

    h.keys("/h\u{e4}lsa").key(KeyCode::Enter);
    assert!(h.app.matches(h.app.rows()[h.app.sel]));

    h.keys("fH\u{e4}lsa").key(KeyCode::Enter);
    let screen = h.screen(100, 12);
    assert!(screen.contains("ha\u{308}lsa.pdf") && !screen.contains("other.txt"));
    h.key(KeyCode::Esc);

    h.keys("sh\u{e4}lsa").key(KeyCode::Enter);
    assert!(h.screen(100, 12).contains("ha\u{308}lsa.pdf"));
}
