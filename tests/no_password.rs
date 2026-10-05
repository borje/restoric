//! A repository made with `restic init --insecure-no-password` opens with
//! `--insecure-no-password`. Skipped when restic isn't installed.

use std::process::Command;

use restoric::repo::Repo;
use restoric::repo::rustic::{OpenOptions, RusticRepo};

#[test]
fn opens_a_repository_without_password() {
    if Command::new("restic").arg("version").output().is_err() {
        eprintln!("restic not installed: skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let src = dir.path().join("src");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("a.txt"), b"no password\n").unwrap();
    let restic = |args: &[&str]| {
        let st = Command::new("restic")
            .args(["--quiet", "--insecure-no-password", "-r"])
            .arg(&repo)
            .args(args)
            .env_remove("RESTIC_PASSWORD")
            .status()
            .unwrap();
        assert!(st.success(), "restic {args:?} failed");
    };
    restic(&["init"]);
    restic(&["backup", "--host", "np", src.to_str().unwrap()]);

    let open = |no_password: bool| {
        RusticRepo::open(&OpenOptions {
            repo: Some(repo.to_string_lossy().into_owned()),
            no_password,
            cache_dir: Some(dir.path().join("cache")),
            ..OpenOptions::default()
        })
    };
    let err = open(false).err().unwrap().to_string();
    assert!(err.contains("--insecure-no-password"), "{err}");

    let r = open(true).unwrap();
    let snaps = r.snapshots().unwrap();
    assert_eq!(snaps.len(), 1);
    let mut tree = r.tree(&snaps[0].tree).unwrap();
    let mut node = None;
    for c in src.join("a.txt").components().skip(1) {
        let n = tree.get(c.as_os_str()).unwrap().clone();
        if let Some(s) = n.subtree {
            tree = r.tree(&s).unwrap();
        }
        node = Some(n);
    }
    assert_eq!(
        r.read_file(&node.unwrap(), 100).unwrap().data,
        b"no password\n"
    );

    let both = RusticRepo::open(&OpenOptions {
        repo: Some(repo.to_string_lossy().into_owned()),
        password: Some("x".into()),
        no_password: true,
        ..OpenOptions::default()
    });
    assert!(both.is_err());
}
