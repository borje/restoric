//! Where restoric keeps its files: the XDG folders on Linux and macOS alike
//! as yazi does, rather than `~/Library` on macOS.

use std::path::{Path, PathBuf};

pub fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// `~/.config/restoric`
pub fn config() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// `~/.cache/restoric`
pub fn cache() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// `~/.local/share/restoric`
pub fn data() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// `~/.local/state/restoric`
pub fn state() -> Option<PathBuf> {
    xdg("XDG_STATE_HOME", ".local/state")
}

/// Creates `dir` and any missing parents, and makes `dir` readable only by
/// this user (0700), as restic does its cache. restoric's folders hold the
/// paths it has looked at, the repositories' backup paths and overwritten
/// files. An existing `dir` is made private too.
pub fn create_private(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var).map(PathBuf::from);
    resolve(base.as_deref(), home().as_deref(), fallback)
}

/// `$XDG_…_HOME/restoric` if it's set to an absolute path (the spec says to
/// ignore a relative one), else `~/<fallback>/restoric`.
fn resolve(base: Option<&Path>, home: Option<&Path>, fallback: &str) -> Option<PathBuf> {
    let base = match base {
        Some(b) if b.is_absolute() => b.to_path_buf(),
        _ => home?.join(fallback),
    };
    Some(base.join("restoric"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_xdg_then_home() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            resolve(Some(Path::new("/xdg")), home, ".config"),
            Some(PathBuf::from("/xdg/restoric"))
        );
        assert_eq!(
            resolve(Some(Path::new("rel")), home, ".config"),
            Some(PathBuf::from("/home/me/.config/restoric"))
        );
        assert_eq!(
            resolve(None, home, ".local/state"),
            Some(PathBuf::from("/home/me/.local/state/restoric"))
        );
        assert_eq!(resolve(None, None, ".config"), None);
    }

    #[cfg(unix)]
    #[test]
    fn private_folders_are_this_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let new = tmp.path().join("a/restoric");
        create_private(&new).unwrap();
        assert_eq!(mode(&new), 0o700);
        assert_eq!(mode(&tmp.path().join("a")), 0o700);
        // One made group-writable by an older restoric is tightened.
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o775)).unwrap();
        create_private(&new).unwrap();
        assert_eq!(mode(&new), 0o700);
    }
}
