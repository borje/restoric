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
}
