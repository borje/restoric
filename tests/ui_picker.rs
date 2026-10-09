//! The repository picker and restores from a foreign host.
//! Times are in UTC.

mod common;

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use common::{Harness, SRC};
use restoric::app::cmdline::InputKind;
use restoric::index::timeline::{Filter, explain_empty};
use restoric::picker::{Level, Pick, Picker, Step};
use restoric::repo::fake::FakeRepo;
use restoric::repo::{Repo, SnapshotInfo};
use restoric::repos::{Candidate, ProbeCache};
use restoric::restore::How;
use restoric::ui;
use restoric::ui::theme::Theme;
use restoric::worker::Request;

/// This machine, a server and the machine's old name.
const TWO_HOSTS: &str = "
host bege-laptop
root /home/bege/dev/project
snapshot 2026-07-14 09:00
  write src/main.go package main\\n
  write README.md hello\\n
snapshot 2026-09-06 18:03
  append src/main.go // serve\\n
snapshot 2026-08-01 10:00 host=dev-vm root=/srv/data
  write etc/app.conf port=80\\n
snapshot 2026-09-01 10:00 host=dev-vm root=/srv/data
  append etc/app.conf debug=1\\n
snapshot 2026-03-01 10:00 host=old-laptop root=/home/bege
  write notes.txt old\\n
";

const LOCATION: &str = "rest:http://iridium:8000/dev-vm";

fn snaps() -> Vec<SnapshotInfo> {
    FakeRepo::parse(TWO_HOSTS).unwrap().snapshots().unwrap()
}

fn now() -> Timestamp {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

fn picker(path: &str) -> Picker {
    Picker::new(
        PathBuf::from(path),
        Some(PathBuf::from("/home/bege")),
        TimeZone::UTC,
        now(),
    )
}

fn mine() -> Vec<String> {
    vec!["bege-laptop".into()]
}

fn screen(p: &Picker, cols: u16, rows: u16) -> String {
    let area = Rect::new(0, 0, cols, rows);
    let mut buf = Buffer::empty(area);
    ui::picker::draw(p, &mut buf, area, &Theme::new(false));
    ui::text(&buf)
}

fn key(p: &mut Picker, code: KeyCode) -> Step {
    p.key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn keys(p: &mut Picker, s: &str) -> Step {
    let mut step = Step::Stay;
    for c in s.chars() {
        step = key(p, KeyCode::Char(c));
    }
    step
}

#[test]
fn groups_19() {
    let p = picker(SRC).with_groups(LOCATION, &snaps(), &mine(), None);
    assert_eq!(p.groups.as_ref().unwrap().sel, 0);
    insta::assert_snapshot!(screen(&p, 100, 34));
}

#[test]
fn groups_with_the_dead_end_message() {
    let path = "/home/bege/music";
    let all = snaps();
    let f = Filter {
        hosts: mine(),
        tag: None,
    };
    let message = explain_empty(&all, &f, Path::new(path));
    let p = picker(path).with_groups(LOCATION, &all, &mine(), Some(message));
    // The closest row: old-laptop's /home/bege.
    assert_eq!(p.groups.as_ref().unwrap().sel, 2);
    insta::assert_snapshot!(screen(&p, 100, 34));
}

#[test]
fn groups_filter() {
    let mut p = picker(SRC).with_groups(LOCATION, &snaps(), &mine(), None);
    keys(&mut p, "/vm");
    assert_eq!(p.visible(), [1]);
    insta::assert_snapshot!(screen(&p, 80, 20));
    key(&mut p, KeyCode::Enter);
    assert_eq!(p.groups.as_ref().unwrap().filter, "vm");
    keys(&mut p, "/x");
    assert!(p.visible().is_empty());
    insta::assert_snapshot!("groups_filter_none", screen(&p, 80, 12));
    key(&mut p, KeyCode::Esc);
    assert_eq!(p.visible().len(), 3);
}

#[test]
fn groups_empty() {
    let p = picker(SRC).with_groups(LOCATION, &[], &mine(), None);
    insta::assert_snapshot!(screen(&p, 80, 12));
    let mut p = p;
    assert_eq!(key(&mut p, KeyCode::Enter), Step::Stay);
}

fn cands() -> Vec<Candidate> {
    let cand = |r: &str| Candidate {
        repo: r.into(),
        filter: Filter {
            hosts: mine(),
            tag: None,
        },
    };
    vec![
        cand("/mnt/photos"),
        cand(LOCATION),
        cand("sftp:nas:/backup"),
    ]
}

#[test]
fn repos_20() {
    let mut cache = ProbeCache::default();
    // Read three hours ago; the photos repo last month; the NAS never.
    let three_h: Timestamp = "2026-10-05T09:00:00Z".parse().unwrap();
    cache.record(LOCATION, &snaps(), three_h);
    let month: Timestamp = "2026-09-05T09:00:00Z".parse().unwrap();
    let mut photo = snaps()[0].clone();
    photo.host = "nas".into();
    photo.paths = vec![PathBuf::from("/mnt/photos")];
    cache.record("/mnt/photos", &[photo], month);
    let mut p = picker(SRC).with_repos(&cands(), &cache);
    let order: Vec<&str> = p.repos.iter().map(|r| r.location.as_str()).collect();
    assert_eq!(order, [LOCATION, "/mnt/photos", "sftp:nas:/backup"]);
    insta::assert_snapshot!(screen(&p, 100, 34));

    // r refreshes the selected row; it reads "just now" afterwards.
    assert_eq!(keys(&mut p, "jjr"), Step::Refresh(2));
    let laptop: Vec<SnapshotInfo> = snaps()
        .into_iter()
        .filter(|s| s.host == "bege-laptop")
        .collect();
    p.refreshed(2, &laptop);
    assert_eq!(p.repos[2].hosts, ["bege-laptop"]);
    assert!(p.repos[2].mine && p.repos[2].holds);
    insta::assert_snapshot!("repos_refreshed", screen(&p, 100, 12));

    // ⏎ opens it: the group level, and esc comes back.
    assert_eq!(key(&mut p, KeyCode::Enter), Step::Open(2));
    p.opened(2, &laptop);
    assert_eq!(p.level, Level::Groups);
    assert_eq!(p.groups.as_ref().unwrap().location, "sftp:nas:/backup");
    insta::assert_snapshot!("repos_opened", screen(&p, 100, 12));
    key(&mut p, KeyCode::Esc);
    assert_eq!(p.level, Level::Repos);
}

#[test]
fn repos_open_error() {
    let mut p = picker(SRC).with_repos(&cands(), &ProbeCache::default());
    assert_eq!(key(&mut p, KeyCode::Enter), Step::Open(0));
    p.failed(0, "connection refused");
    assert_eq!(p.level, Level::Repos);
    insta::assert_snapshot!(screen(&p, 100, 12));
}

#[test]
fn repos_empty() {
    let p = picker(SRC).with_repos(&[], &ProbeCache::default());
    insta::assert_snapshot!(screen(&p, 80, 12));
}

/// The app as `main` builds it after a pick of `host` at `path`.
fn after_pick(pick: &Pick) -> Harness {
    let foreign = !mine().contains(&pick.host);
    let folder = if Path::new(SRC).starts_with(&pick.path) {
        SRC.to_string()
    } else {
        pick.path.to_string_lossy().into_owned()
    };
    let f = Filter {
        hosts: vec![pick.host.clone()],
        tag: None,
    };
    let mut h = Harness::with_filter(TWO_HOSTS, &folder, f);
    h.app.mine = mine();
    h.app.shown_host = Some(pick.host.clone());
    h.app.start_dir = PathBuf::from(SRC);
    assert_eq!(h.app.foreign(), foreign);
    h
}

#[test]
fn picking_the_other_host_opens_its_timeline() {
    let mut p = picker(SRC).with_groups(LOCATION, &snaps(), &mine(), None);
    keys(&mut p, "j");
    let Step::Picked(pick) = key(&mut p, KeyCode::Enter) else {
        panic!("no pick");
    };
    assert_eq!(
        pick,
        Pick {
            host: "dev-vm".into(),
            path: PathBuf::from("/srv/data"),
            repo: None,
        }
    );
    let mut h = after_pick(&pick);
    assert_eq!(h.app.folder, PathBuf::from("/srv/data"));
    assert_eq!(h.app.set().len(), 2);
    // /srv/data isn't on this machine: it shows as missing, with no error.
    assert_eq!(h.app.message, None);
    let live = h.app.live.get(Path::new("/srv/data")).unwrap();
    assert!(live.deleted > 0, "{live:?}");
    insta::assert_snapshot!(h.screen(100, 34));

    // P is off; p and r ask where the copy goes.
    h.select("etc").keys("yP");
    assert_eq!(
        h.app.message.as_deref(),
        Some(
            "Overwriting is off for another host's snapshots. p restores into a directory you choose."
        )
    );
    assert!(h.app.confirm.is_none());
    h.keys("p");
    let input = h.app.input.clone().unwrap();
    assert_eq!(input.kind, InputKind::Dir);
    assert_eq!(input.text, "~/dev/project/src");
    insta::assert_snapshot!(
        "restore_to_prompt",
        h.screen(100, 34).lines().last().unwrap()
    );
    // ⏎ sends the restore into that directory (not run here: it would write).
    h.app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let sent = h.app.outbox.iter().find_map(|r| match r {
        Request::Restore { how, targets, .. } => Some((how.clone(), targets.len())),
        _ => None,
    });
    assert_eq!(sent, Some((How::Into(PathBuf::from(SRC)), 1)));
    h.app.outbox.clear();
    h.app.restoring = None;
    assert_eq!(h.app.last_dir, Some(PathBuf::from(SRC)));
    // r asks too, prefilled with the last directory; esc cancels.
    h.keys("r");
    assert!(h.app.dialog.is_none());
    assert_eq!(h.app.input.as_ref().unwrap().text, "~/dev/project/src");
    h.key(KeyCode::Esc);
    assert!(h.app.input.is_none());
    h.app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let restore = h
        .app
        .outbox
        .iter()
        .any(|r| matches!(r, Request::Restore { .. }));
    assert!(!restore, "nothing pending after esc");
}

#[test]
fn picking_this_machine_keeps_restores_as_usual() {
    let pick = Pick {
        host: "bege-laptop".into(),
        path: PathBuf::from("/home/bege/dev/project"),
        repo: None,
    };
    let mut h = after_pick(&pick);
    assert_eq!(h.app.folder, PathBuf::from(SRC));
    h.select("main.go").keys("r");
    assert!(h.app.dialog.is_some());
}

/// A restore into a named directory on a real folder: `<dir>/name`, then `-2`.
#[test]
fn restore_into_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let dsl = format!(
        "host server\nroot {}\nsnapshot 2026-07-01 10:00\n  write a.txt one\\n\n",
        root.display()
    );
    let f = Filter {
        hosts: vec!["server".into()],
        tag: None,
    };
    let mut h = Harness::with_filter(&dsl, root.to_str().unwrap(), f);
    h.app.mine = vec!["me".into()];
    let out = dir.path().join("out");
    h.app.start_dir = out.clone();
    h.select("a.txt").keys("yp").key(KeyCode::Enter);
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"one\n");
    assert_eq!(
        h.app.message,
        Some(format!("Restored to {}", out.join("a.txt").display()))
    );
    h.keys("p").key(KeyCode::Enter);
    assert_eq!(std::fs::read(out.join("a.txt-2")).unwrap(), b"one\n");
}
