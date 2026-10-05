//! Snapshot tests of the folder view (PLAN.md §3.1, §3.4, §3.12, §3.13),
//! driven by FakeRepo with tests/fixtures/project.dsl. Times are in UTC.

use std::path::PathBuf;
use std::sync::Arc;

use jiff::tz::TimeZone;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use restoric::app::{Action, App};
use restoric::cache::Cache;
use restoric::index::{Index, Mode};
use restoric::repo::Repo;
use restoric::repo::fake::FakeRepo;
use restoric::ui;
use restoric::ui::theme::Theme;
use restoric::worker::handle;

const PROJECT: &str = include_str!("fixtures/project.dsl");

struct Harness {
    app: App,
    index: Index,
}

impl Harness {
    fn new(folder: &str) -> Self {
        let repo = Arc::new(FakeRepo::parse(PROJECT).unwrap());
        let snaps = repo.snapshots().unwrap();
        let index = Index::new(repo, Cache::in_memory(), Mode::Content, 1 << 24);
        let app = App::new(
            snaps,
            PathBuf::from(folder),
            TimeZone::UTC,
            Some(PathBuf::from("/home/bege")),
        );
        let mut h = Harness { app, index };
        h.pump();
        h
    }

    /// Runs every request the app has made, until it makes no more.
    fn pump(&mut self) {
        while !self.app.outbox.is_empty() {
            let reqs = std::mem::take(&mut self.app.outbox);
            let mut out = Vec::new();
            for r in reqs {
                handle(&self.index, r, &mut |resp| out.push(resp));
            }
            for r in out {
                self.app.apply(r);
            }
        }
        self.app.bumped = false;
    }

    fn act(&mut self, a: Action) -> &mut Self {
        self.app.act(a);
        self.pump();
        self
    }

    fn keys(&mut self, keys: &str) -> &mut Self {
        for c in keys.chars() {
            self.app
                .key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            self.pump();
        }
        self
    }

    /// Views the snapshot taken at `when` (`2026-09-06 18:03`).
    fn at(&mut self, when: &str) -> &mut Self {
        let set = self.app.set();
        let i = set
            .iter()
            .position(|s| ui::fmt::time(s.time, &TimeZone::UTC) == fmt_like(when))
            .unwrap_or_else(|| panic!("no snapshot at {when}"));
        self.act(Action::GoSnapshot(i))
    }

    fn select(&mut self, name: &str) -> &mut Self {
        let rows = self.app.rows();
        let i = rows
            .iter()
            .position(|r| {
                self.app
                    .entry(*r)
                    .is_some_and(|e| e.node.name.to_string_lossy() == name)
            })
            .unwrap_or_else(|| panic!("no row {name}"));
        let sel = self.app.sel;
        if i > sel {
            self.act(Action::Down(i - sel))
        } else {
            self.act(Action::Up(sel - i))
        }
    }

    fn screen(&mut self, cols: u16, rows: u16) -> String {
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        ui::draw(&mut self.app, &mut buf, area, &Theme::new(false));
        ui::text(&buf)
    }
}

/// `2026-09-06 18:03` → `Sep 06 18:03`
fn fmt_like(when: &str) -> String {
    let dt: jiff::civil::DateTime = when.replace(' ', "T").parse().unwrap();
    ui::fmt::time(
        dt.to_zoned(TimeZone::UTC).unwrap().timestamp(),
        &TimeZone::UTC,
    )
}

const SRC: &str = "/home/bege/dev/project/src";

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
    let mut app = App::new(snaps, PathBuf::from(SRC), TimeZone::UTC, None);
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
