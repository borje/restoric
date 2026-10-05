//! The repository written by the real `restic` binary
//! (tests/fixtures/make_repo.sh), built once per test binary. `None`,
//! with a note, when restic isn't installed; CI installs it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use restoric::repo::rustic::{OpenOptions, RusticRepo};

pub struct Fixture {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
}

impl Fixture {
    pub fn open(&self) -> RusticRepo {
        RusticRepo::open(&OpenOptions {
            repo: Some(self.root.join("repo").to_string_lossy().into_owned()),
            password: Some("restoric".into()),
            cache_dir: Some(self.root.join("rustic-cache")),
            ..OpenOptions::default()
        })
        .unwrap()
    }

    /// The backed-up folder.
    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }
}

pub fn fixture() -> Option<&'static Fixture> {
    static F: OnceLock<Option<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        if Command::new("restic").arg("version").output().is_err() {
            eprintln!("restic not installed: skipping the fixture tests");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/make_repo.sh");
        let status = Command::new("bash")
            .arg(script)
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "make_repo.sh failed");
        let root = dir.path().to_path_buf();
        Some(Fixture { _dir: dir, root })
    })
    .as_ref()
}
