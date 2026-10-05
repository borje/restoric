//! Drives the app against FakeRepo for the UI tests: requests run
//! synchronously through `worker::handle`. Times are in UTC.

#![allow(dead_code)]

pub mod fixture;

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
use restoric::worker::{Ctx, handle};

pub const PROJECT: &str = include_str!("../fixtures/project.dsl");

pub const SRC: &str = "/home/bege/dev/project/src";

pub struct Harness {
    pub app: App,
    pub ctx: Ctx,
}

impl Harness {
    /// The sample project, opened at `folder`.
    pub fn new(folder: &str) -> Self {
        Self::with(PROJECT, folder)
    }

    /// A history in the FakeRepo DSL, opened at `folder`.
    pub fn with(dsl: &str, folder: &str) -> Self {
        let repo = Arc::new(FakeRepo::parse(dsl).unwrap());
        let snaps = repo.snapshots().unwrap();
        let disk = Arc::new(repo.disk());
        let index = Index::new(repo, Cache::in_memory(), Mode::Content, 1 << 24);
        let mode_switch = index.mode_switch();
        let app = App::new(
            snaps,
            restoric::index::timeline::Filter::default(),
            PathBuf::from(folder),
            TimeZone::UTC,
            Some(PathBuf::from("/home/bege")),
        );
        let mut app = app;
        app.mode_switch = Some(mode_switch);
        app.now = Some("2026-10-05T12:00:00Z".parse().unwrap());
        app.places.restore_dir = PathBuf::from("/home/bege/Restored");
        let mut h = Harness {
            app,
            ctx: Ctx {
                index,
                disk,
                places: restoric::restore::Places {
                    restore_dir: PathBuf::from("/home/bege/Restored"),
                    undo_dir: std::env::temp_dir().join("restoric-test-undo"),
                    tz: TimeZone::UTC,
                },
            },
        };
        h.pump();
        h
    }

    /// Runs every request the app has made, until it makes no more.
    pub fn pump(&mut self) {
        while !self.app.outbox.is_empty() {
            let reqs = std::mem::take(&mut self.app.outbox);
            let mut out = Vec::new();
            for r in reqs {
                handle(&self.ctx, r, &mut |resp| out.push(resp));
            }
            for r in out {
                self.app.apply(r);
            }
        }
        self.app.bumped = false;
    }

    pub fn act(&mut self, a: Action) -> &mut Self {
        self.app.act(a);
        self.pump();
        self
    }

    pub fn keys(&mut self, keys: &str) -> &mut Self {
        for c in keys.chars() {
            self.key(KeyCode::Char(c));
        }
        self
    }

    pub fn key(&mut self, code: KeyCode) -> &mut Self {
        self.app.key(KeyEvent::new(code, KeyModifiers::NONE));
        self.pump();
        self
    }

    /// Views the snapshot taken at `when` (`2026-09-06 18:03`).
    pub fn at(&mut self, when: &str) -> &mut Self {
        let set = self.app.set();
        let i = set
            .iter()
            .position(|s| ui::fmt::time(s.time, &TimeZone::UTC) == fmt_like(when))
            .unwrap_or_else(|| panic!("no snapshot at {when}"));
        self.act(Action::GoSnapshot(i))
    }

    pub fn select(&mut self, name: &str) -> &mut Self {
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

    pub fn screen(&mut self, cols: u16, rows: u16) -> String {
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        ui::draw(&mut self.app, &mut buf, area, &Theme::new(false));
        ui::text(&buf)
    }
}

/// `2026-09-06 18:03` → `Sep 06 18:03`
pub fn fmt_like(when: &str) -> String {
    let dt: jiff::civil::DateTime = when.replace(' ', "T").parse().unwrap();
    ui::fmt::time(
        dt.to_zoned(TimeZone::UTC).unwrap().timestamp(),
        &TimeZone::UTC,
    )
}
