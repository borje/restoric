//! Change points against a repository written by the real `restic` binary
//! (tests/fixtures/make_repo.sh). Skipped, with a note, when restic isn't
//! installed; CI installs it.

mod common;

use std::sync::Arc;

use common::fixture::{Fixture, fixture};

use restoric::cache::Cache;
use restoric::index::timeline::{ChangeKind, Filter, timeline_set};
use restoric::index::{Index, Mode};
use restoric::repo::Repo;

use ChangeKind::{Added as A, Changed as C, Deleted as D};

fn open(f: &Fixture, mode: Mode) -> (Index, Vec<restoric::repo::SnapshotInfo>) {
    let repo = f.open();
    let snaps = repo.snapshots().unwrap();
    (
        Index::new(Arc::new(repo), Cache::in_memory(), mode, 1 << 24),
        snaps,
    )
}

/// (snapshot number, kind) for each change point of `rel` under src/.
fn points(f: &Fixture, mode: Mode, rel: &str) -> Vec<(usize, ChangeKind)> {
    let (index, snaps) = open(f, mode);
    let filter = Filter {
        hosts: vec!["restoric-fixture".into()],
        tag: None,
    };
    let src = f.root.join("src");
    let path = if rel.is_empty() { src } else { src.join(rel) };
    let set = timeline_set(&snaps, &filter, &path);
    assert_eq!(set.len(), 11);
    let refs = index.refs(&set, &path, &mut |_, _| {}).unwrap();
    index
        .change_points(&refs)
        .unwrap()
        .into_iter()
        .map(|p| (p.index, p.kind))
        .collect()
}

#[test]
fn folder_change_points_skip_metadata_only_snapshots() {
    let Some(f) = fixture() else { return };
    assert_eq!(
        points(f, Mode::Content, ""),
        [
            (0, A),
            (1, C),
            (3, C),
            (4, C),
            (6, C),
            (7, C),
            (9, C),
            (10, C)
        ]
    );
    assert_eq!(
        points(f, Mode::Content, "sub"),
        [(0, A), (3, C), (6, C), (9, D), (10, A)]
    );
}

#[test]
fn file_change_points() {
    let Some(f) = fixture() else { return };
    assert_eq!(points(f, Mode::Content, "a.txt"), [(0, A), (1, C), (4, C)]);
    assert_eq!(
        points(f, Mode::Content, "sub/d.txt"),
        [(0, A), (3, D), (6, A), (9, D)]
    );
    assert_eq!(points(f, Mode::Content, "keep.txt"), [(0, A), (7, D)]);
    assert_eq!(points(f, Mode::Content, "kept.txt"), [(7, A)]);
}

#[test]
fn strict_shows_metadata_only_snapshots() {
    let Some(f) = fixture() else { return };
    let strict: Vec<usize> = points(f, Mode::Strict, "").iter().map(|p| p.0).collect();
    // restic also stores access times, so strict may see more; it must see these.
    for i in [0, 1, 2, 3, 4, 6, 7, 8, 9, 10] {
        assert!(
            strict.contains(&i),
            "strict misses snapshot {i}: {strict:?}"
        );
    }
    let b: Vec<usize> = points(f, Mode::Strict, "b.txt")
        .iter()
        .map(|p| p.0)
        .collect();
    assert!(b.contains(&2), "{b:?}");
}

#[test]
fn counts() {
    let Some(f) = fixture() else { return };
    let (index, snaps) = open(f, Mode::Content);
    let filter = Filter {
        hosts: vec!["restoric-fixture".into()],
        tag: None,
    };
    let path = f.root.join("src");
    let log = restoric::log::log(&index, &filter, &path, &mut |_, _| {}).unwrap();
    let labels: Vec<String> = log
        .entries
        .iter()
        .rev()
        .map(|e| e.counts.unwrap().label(" "))
        .collect();
    assert_eq!(labels, ["+5", "~1", "−1", "~1", "+1", "+1 −1", "−2", "+1"]);
    assert_eq!(log.snapshots, 11);
    assert_eq!(snaps.len(), 11);
}

#[test]
fn reads_a_file() {
    let Some(f) = fixture() else { return };
    let (index, snaps) = open(f, Mode::Content);
    let mut snaps = snaps;
    snaps.sort_by_key(|s| s.time);
    let node = index
        .node_at(&snaps[0], &f.root.join("src/a.txt"))
        .unwrap()
        .unwrap();
    let bytes = index.repo().read_file(&node, 1024).unwrap();
    assert_eq!(bytes.data, b"alpha\n");
    assert!(!bytes.truncated());
}
