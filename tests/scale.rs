//! Timing on a synthetic repository from tests/fixtures/make_big_repo.sh
//! (PLAN.md §4.8). Runs only when RESTORIC_SCALE_REPO points at one, e.g.
//! `RESTORIC_SCALE_REPO=/tmp/big/small cargo test --release --test scale`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use restoric::cache::Cache;
use restoric::index::timeline::{Filter, timeline_set};
use restoric::index::{Index, Mode};
use restoric::repo::rustic::{BACKEND, OpenOptions, RusticRepo};

#[test]
fn change_points_and_listing_are_fast() {
    let Some(dir) = std::env::var_os("RESTORIC_SCALE_REPO").map(PathBuf::from) else {
        eprintln!("RESTORIC_SCALE_REPO not set: skipping");
        return;
    };
    let t = Instant::now();
    let repo = RusticRepo::open(&OpenOptions {
        repo: Some(dir.join("repo").to_string_lossy().into_owned()),
        password: Some("restoric".into()),
        ..OpenOptions::default()
    })
    .unwrap();
    let open = t.elapsed();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(&cache_dir.path().join("c.redb"), BACKEND, u64::MAX).unwrap();
    let index = Index::new(Arc::new(repo), cache, Mode::Content, 256 << 20);
    let snaps = index.repo().snapshots().unwrap();
    let filter = Filter {
        hosts: vec!["restoric-big".into()],
        tag: None,
    };
    let folder = dir.join("src/data");
    let set = timeline_set(&snaps, &filter, &folder);

    let mut timings = Vec::new();
    for pass in ["cold", "warm"] {
        let t = Instant::now();
        let refs = index.refs(&set, &folder, &mut |_, _| {}).unwrap();
        let points = index.change_points(&refs).unwrap();
        let listing = index.listing(&set, set.len() - 1, &folder).unwrap();
        index.flush().unwrap();
        let took = t.elapsed();
        eprintln!(
            "{pass}: {} snapshots, {} change points, {} entries — {took:?} (open {open:?})",
            set.len(),
            points.len(),
            listing.map_or(0, |l| l.len())
        );
        timings.push(took);
    }
    // §4.8 targets: a folder's change points under 2 s; warm is from the cache.
    assert!(timings[0] < Duration::from_secs(2), "cold {:?}", timings[0]);
    assert!(
        timings[1] < Duration::from_millis(500),
        "warm {:?}",
        timings[1]
    );
}
