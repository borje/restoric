//! Snapshot tests of the folder view,
//! driven by FakeRepo with tests/fixtures/project.dsl. Times are in UTC.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use jiff::tz::TimeZone;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use common::{Harness, PROJECT, SRC};
use restoric::app::{Action, App};
use restoric::repo::Repo;
use restoric::repo::fake::FakeRepo;
use restoric::ui;
use restoric::ui::theme::Theme;

#[test]
fn starts_at_the_newest_change() {
    let mut h = Harness::new(SRC);
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn folder_view_01() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn eighty_columns_17() {
    let mut h = Harness::new(SRC);
    h.at("2026-10-02 12:21").select("main.go");
    insta::assert_snapshot!(h.screen(80, 34));
}

#[test]
fn wide_terminal_widens_the_listing() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go");
    insta::assert_snapshot!(h.screen(171, 34));
}

#[test]
fn under_eighty_columns() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go");
    insta::assert_snapshot!(h.screen(70, 24));
}

#[test]
fn backup_root() {
    let mut h = Harness::new(SRC);
    h.keys("gh");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn which_key_g() {
    let mut h = Harness::new(SRC);
    h.keys("g");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn help_16() {
    let mut h = Harness::new(SRC);
    h.keys("?");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn boundary_message() {
    let mut h = Harness::new(SRC);
    h.act(Action::OldestChange).keys("H");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn indexing() {
    let repo = Arc::new(FakeRepo::parse(PROJECT).unwrap());
    let snaps = repo.snapshots().unwrap();
    let mut app = App::new(
        snaps,
        restoric::index::timeline::Filter::default(),
        PathBuf::from(SRC),
        TimeZone::UTC,
        None,
    );
    let area = Rect::new(0, 0, 100, 12);
    let mut buf = Buffer::empty(area);
    ui::draw(&mut app, &mut buf, area, &Theme::new(false));
    insta::assert_snapshot!(ui::text(&buf));
}

#[test]
fn counts_repeat_motions_and_stop_at_boundaries() {
    let mut h = Harness::new(SRC);
    let newest = h.app.idx();
    h.keys("3H");
    let set = h.app.set();
    let state = h.app.state().unwrap();
    let versions = state.versions();
    assert_eq!(h.app.idx(), versions[versions.len() - 4]);
    h.keys("99H");
    assert_eq!(h.app.idx(), versions[0]);
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the oldest version of this folder.")
    );
    h.keys("L");
    assert_eq!(h.app.message, None);
    assert!(h.app.idx() < newest && set.len() > 1);
}

#[test]
fn opening_a_folder_keeps_the_time_and_h_selects_where_you_came_from() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("api");
    let snap = h.app.snap;
    h.keys("l");
    assert_eq!(h.app.folder, PathBuf::from(SRC).join("api"));
    assert_eq!(h.app.snap, snap);
    h.keys("h");
    assert_eq!(h.app.folder, PathBuf::from(SRC));
    assert_eq!(h.app.selected().unwrap().node.name.to_string_lossy(), "api");
    h.keys("gh").keys("h");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Already at the top of the backup.")
    );
}

#[test]
fn file_track_02() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go").keys("}");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn preview_vs_disk_03() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("main.go").key(KeyCode::Tab);
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn deleted_shown_11() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-13 20:51").keys(".").select("legacy.go");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn versions_13() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-09 19:23").select("main.go").keys("l");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn folder_preview() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("api");
    insta::assert_snapshot!(h.screen(100, 34));
}

#[test]
fn deleted_items_toggle() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03");
    assert_eq!(h.app.hidden(), 1);
    h.keys(".");
    assert_eq!(h.app.hidden(), 0);
    let rows = h.app.rows();
    assert!(
        rows.iter()
            .any(|r| h.app.entry(*r).is_some_and(|e| e.node.name == "feature.go"))
    );
    h.keys("zd");
    assert_eq!(h.app.hidden(), 1);
}

#[test]
fn item_changes_and_versions_keys() {
    let mut h = Harness::new(SRC);
    h.at("2026-09-06 18:03").select("main.go");
    h.keys("{");
    let t = h.app.set()[h.app.idx()].time;
    assert_eq!(ui::fmt::time(t, &TimeZone::UTC, t), "Sep 02 20:16");
    h.keys("99{");
    assert_eq!(
        h.app.message.as_deref(),
        Some("No older change to main.go.")
    );
    h.keys("l");
    assert!(matches!(h.app.view, restoric::app::View::Versions(_)));
    h.keys("99j");
    assert_eq!(
        h.app.message.as_deref(),
        Some("This is the oldest version.")
    );
    h.keys("h");
    assert!(matches!(h.app.view, restoric::app::View::Folder));
    assert_eq!(h.app.selected().unwrap().node.name, "main.go");
}

/// Backups that name files, not their folder (`restic backup trading/a.log …`):
/// the folder above them still opens.
#[test]
fn folder_above_the_backup_paths() {
    let dsl = "\
host bege-laptop
root /home/bege/trading/logs
snapshot 2026-09-01 10:00
  write run.log one\\n
snapshot 2026-09-02 10:00
  append run.log two\\n
";
    let mut h = Harness::with(dsl, "/home/bege/trading");
    insta::assert_snapshot!(h.screen(100, 20));
}

#[test]
fn j_k_scroll_the_preview() {
    let body: String = (1..=60).map(|n| format!("line {n}\\n")).collect();
    let dsl = format!(
        "host bege-laptop\nroot /home/bege/data\nsnapshot 2026-07-01 10:00\n  write long.txt {body}\n"
    );
    let mut h = Harness::with(&dsl, "/home/bege/data");
    h.select("long.txt");
    let before = h.screen(100, 34);
    assert!(before.contains("line 1"));
    h.keys("J");
    assert_eq!(h.app.scroll, Some(3));
    let after = h.screen(100, 34);
    assert_ne!(before, after, "J should scroll the preview");
    assert!(after.contains("line 4"));
    h.keys("K");
    assert_eq!(h.screen(100, 34), before);
}

/// A file replaced by a folder of the same name in a snapshot shows both
/// rows, as the on disk version does.
#[test]
fn snapshot_listing_kind_change() {
    let dsl = PROJECT.replace(
        "\ndisk\n",
        "snapshot 2026-10-05 12:00\n  rm src/util.go\n  write src/util.go/a.go package util\\n\n\ndisk\n",
    );
    let mut h = Harness::with(&dsl, SRC);
    h.at("2026-10-05 12:00");
    let rows = h.app.rows();
    let utils: Vec<_> = rows
        .iter()
        .filter_map(|r| h.app.entry(*r))
        .filter(|e| e.node.name.to_string_lossy() == "util.go")
        .map(|e| (e.is_dir(), e.is_deleted()))
        .collect();
    assert_eq!(utils, vec![(true, false), (false, true)]);
}

const LABELLED: &str = "
host bege-laptop
root /home/bege/data
snapshot 2026-07-01 10:00
  write a.txt one\\n
  write b.txt two\\n
snapshot 2026-07-02 10:00 label=pre-migration tags=manual
  describe Before moving the catalog to the new disk
  write a.txt one more\\n
  write c.txt three\\n
";

/// `i` shows the snapshot's details. Moves in time keep them open on the
/// snapshot moved to, the files on disk included; any other key closes them.
#[test]
fn snapshot_info() {
    let mut h = Harness::with(LABELLED, "/home/bege/data");
    h.at("2026-07-02 10:00").keys("i");
    insta::assert_snapshot!(h.screen(100, 30));
    let place = |h: &Harness| h.app.info_snapshot().map(|(_, n, _)| n);
    h.keys("H");
    assert!(h.app.info);
    assert_eq!(place(&h), Some(1));
    h.keys("]");
    assert_eq!(place(&h), Some(2));
    h.keys("L");
    assert!(h.app.info && place(&h).is_none());
    insta::assert_snapshot!("snapshot_info_on_disk", h.screen(100, 14));
    h.keys("j");
    assert!(!h.app.info);
    h.keys("i");
    assert!(!h.app.info);
    assert_eq!(
        h.app.message.as_deref(),
        Some("The files on disk aren't a snapshot.")
    );
    // In the versions view, the selected version's snapshot.
    h.keys("H").select("a.txt").keys("li");
    let label = |h: &Harness| h.app.info_snapshot().map(|(s, ..)| s.label);
    assert_eq!(label(&h).as_deref(), Some("pre-migration"));
    h.keys("H");
    assert_eq!(label(&h).as_deref(), Some(""));
}

/// A click on a timeline dot keeps the details open on that snapshot;
/// a click anywhere else closes them.
#[test]
fn snapshot_info_follows_clicks() {
    let mut h = Harness::with(LABELLED, "/home/bege/data");
    h.at("2026-07-02 10:00").keys("i");
    let click = |h: &mut Harness, a: Option<Action>| {
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let hits = ui::draw(&mut h.app, &mut buf, area, &Theme::new(false));
        let (column, row) = match a {
            Some(a) => {
                let (r, _) = hits.areas.iter().find(|(_, b)| *b == a).unwrap();
                (r.x, r.y)
            }
            None => (50, 15),
        };
        let kind = MouseEventKind::Down(MouseButton::Left);
        let modifiers = KeyModifiers::NONE;
        h.app.mouse(
            MouseEvent {
                kind,
                column,
                row,
                modifiers,
            },
            &hits,
        );
        h.pump();
    };
    click(&mut h, Some(Action::GoSnapshot(0)));
    assert!(h.app.info);
    assert_eq!(h.app.info_snapshot().map(|(_, n, _)| n), Some(1));
    click(&mut h, None);
    assert!(!h.app.info);
}

const PHOTOS: &str = "
host bege-laptop
root /home/bege/Bilder
snapshot 2026-07-01 10:00
  write 2026/a.jpg a
  write 2026/b.jpg b
  write old/c.jpg c
  write d.jpg d
  write e.jpg e
  write f.jpg f
snapshot 2026-07-02 10:00
  write 2026/b.jpg bb
  write e.jpg ee
  write g.jpg g
";

/// `zc` hides what didn't change, and the footer says how much.
#[test]
fn changes_only() {
    let mut h = Harness::with(PHOTOS, "/home/bege/Bilder");
    h.at("2026-07-02 10:00").keys("zc");
    insta::assert_snapshot!(h.screen(100, 14));
    h.keys("zc");
    assert!(!h.app.changes_only);
    assert_eq!(h.app.rows().len(), 6);
}

/// A snapshot with a label or description gets a ▵ under its dot, and
/// its label in the status bar when it's the one shown.
#[test]
fn labelled_snapshots() {
    let mut h = Harness::with(LABELLED, "/home/bege/data");
    h.at("2026-07-01 10:00");
    insta::assert_snapshot!(h.screen(100, 8));
    h.at("2026-07-02 10:00");
    assert!(h.screen(100, 8).contains("“pre-migration”"));
}

/// `S` shows what's under each folder, and the folder's total and the
/// bytes this snapshot added to it in the status bar.
#[test]
fn folder_sizes() {
    let mut h = Harness::with(PHOTOS, "/home/bege/Bilder");
    h.at("2026-07-02 10:00").keys("S");
    insta::assert_snapshot!(h.screen(100, 12));
    // 2026/ holds a (1 B) and b (2 B); the snapshot changed b to "bb" and
    // e to "ee" and added g: 5 bytes of new or changed files.
    let id = h.app.set()[h.app.idx()].id;
    let key = (id, None, "/home/bege/Bilder".into());
    assert_eq!(h.app.added.get(&key), Some(&5));
    assert_eq!(h.app.folder_size(), Some(9));
}

const THREE: &str = "
host bege-laptop
root /home/bege/data
snapshot 2026-07-01 10:00
  write a.txt one\\n
  write b.txt two\\n
  write c.txt three\\n
snapshot 2026-07-02 10:00
  write a.txt one more\\n
snapshot 2026-07-03 10:00
  rm b.txt
  write d.txt four\\n
";

/// `b` pins a base: the listing and `d` compare with it instead of the
/// snapshot before, until `b` on it again.
#[test]
fn base_snapshot() {
    let mut h = Harness::with(THREE, "/home/bege/data");
    h.at("2026-07-01 10:00").keys("b");
    assert!(h.app.base.is_some());
    h.at("2026-07-03 10:00");
    h.app.message = None;
    insta::assert_snapshot!("base_listing", h.screen(100, 12));
    h.select("a.txt").keys("d");
    insta::assert_snapshot!("base_diff", h.screen(100, 10));
    h.keys("q");
    // S counts what changed since the base too: a's 9 bytes and d's 5.
    h.keys("S");
    let (id, base) = (h.app.set()[h.app.idx()].id, h.app.base);
    let key = (id, base, "/home/bege/data".into());
    assert_eq!(h.app.added.get(&key), Some(&14));
    h.at("2026-07-01 10:00").keys("b");
    assert!(h.app.base.is_none());
    // On disk the base can't be pinned.
    h.keys("99Lb");
    assert_eq!(
        h.app.message.as_deref(),
        Some("Pin a snapshot as the base, not the files on disk.")
    );
}

/// A date more than about six months back shows its year instead of the
/// time of day, as `ls -l` does, in the same 12 columns.
#[test]
fn old_dates_show_the_year() {
    let dsl = "
host bege-laptop
root /home/bege/data
snapshot 2025-01-15 10:00
  write a.txt one\\n
snapshot 2026-09-20 10:00
  write a.txt two\\n
";
    let mut h = Harness::with(dsl, "/home/bege/data");
    h.keys("H");
    insta::assert_snapshot!(h.screen(100, 10));
}
