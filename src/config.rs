//! `~/.config/restoric/config.toml` (PLAN.md §10). Every key is optional.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;
use serde::Deserialize;

use crate::app::Action;
use crate::ui::fmt::expand_home;
use crate::ui::theme::Theme;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Hosts {
    One(String),
    Many(Vec<String>),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// This machine's hostname(s), if snapshots use other names (§2.4).
    pub host: Option<Hosts>,
    pub tag: Option<String>,
    pub icons: Option<bool>,
    pub preview_max_kb: Option<u64>,
    pub diff_max_mb: Option<u64>,
    pub memory_cache_mb: Option<u64>,
    pub disk_cache_mb: Option<u64>,
    pub restore_dir: Option<String>,
    /// Action name → key, e.g. `versions = "i"`.
    pub keys: BTreeMap<String, String>,
    /// Style name → colour, e.g. `accent = "magenta"` or `"#e4a84c"`.
    pub colors: BTreeMap<String, String>,
    /// Repositories to look in, as `[[repo]]` blocks (§4.6).
    pub repo: Vec<RepoEntry>,
}

/// One `[[repo]]` block: where a repository is and how to unlock it.
/// restoric uses the one whose snapshots hold the folder it opens.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    pub repository: String,
    pub password_file: Option<String>,
    pub password_command: Option<String>,
    #[serde(default)]
    pub insecure_no_password: bool,
    /// Replaces the top-level `host` for this repository.
    pub host: Option<Hosts>,
    /// Replaces the top-level `tag` for this repository.
    pub tag: Option<String>,
}

impl RepoEntry {
    pub fn hosts(&self) -> Vec<String> {
        hosts_of(&self.host)
    }

    fn has_password(&self) -> bool {
        self.password_file.is_some() || self.password_command.is_some() || self.insecure_no_password
    }
}

/// Where a repository is and how to unlock it, from one source: the flags,
/// the environment or a `[[repo]]` block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub repo: Option<String>,
    pub repo_file: Option<PathBuf>,
    /// Only from the environment (`RESTIC_PASSWORD`): there's no flag.
    pub password: Option<String>,
    pub password_file: Option<PathBuf>,
    pub password_command: Option<String>,
    pub no_password: bool,
}

impl Access {
    pub fn has_repo(&self) -> bool {
        self.repo.is_some() || self.repo_file.is_some()
    }

    fn has_password(&self) -> bool {
        self.password.is_some()
            || self.password_file.is_some()
            || self.password_command.is_some()
            || self.no_password
    }

    fn password_from(mut self, other: &Access) -> Self {
        self.password = other.password.clone();
        self.password_file = other.password_file.clone();
        self.password_command = other.password_command.clone();
        self.no_password = other.no_password;
        self
    }
}

/// Combines the sources. The repository comes from a flag, then `entry`
/// (the `[[repo]]` that holds the folder), then the environment. The
/// password comes from a flag, then `entry` if it gave the repository and
/// says how to unlock it, then the environment.
pub fn resolve_access(
    flags: &Access,
    env: &Access,
    entry: Option<&RepoEntry>,
    home: Option<&Path>,
) -> Access {
    let entry = entry.filter(|_| !flags.has_repo());
    let mut a = match entry {
        Some(e) => Access {
            repo: Some(e.repository.clone()),
            ..Access::default()
        },
        None if flags.has_repo() => Access {
            repo: flags.repo.clone(),
            repo_file: flags.repo_file.clone(),
            ..Access::default()
        },
        None => Access {
            repo: env.repo.clone(),
            repo_file: env.repo_file.clone(),
            ..Access::default()
        },
    };
    if flags.has_password() {
        a = a.password_from(flags);
    } else if let Some(e) = entry.filter(|e| e.has_password()) {
        a.password_file = e.password_file.as_deref().map(|f| expand_home(f, home));
        a.password_command = e.password_command.clone();
        a.no_password = e.insecure_no_password;
    } else {
        a = a.password_from(env);
    }
    a
}

fn hosts_of(h: &Option<Hosts>) -> Vec<String> {
    match h {
        Some(Hosts::One(h)) => vec![h.clone()],
        Some(Hosts::Many(h)) => h.clone(),
        None => Vec::new(),
    }
}

/// `$XDG_CONFIG_HOME` or `~/.config`, on every platform (not macOS's
/// `Library/Application Support`).
fn config_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")))?;
    Some(base.join("restoric").join("config.toml"))
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        config_path(xdg, home)
    }

    /// The config file, or the defaults if there isn't one.
    pub fn load() -> Result<Self> {
        match Self::path() {
            Some(p) if p.exists() => Self::from_file(&p),
            _ => Ok(Self::default()),
        }
    }

    pub fn from_file(p: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        let c: Config = toml::from_str(&text).with_context(|| format!("in {}", p.display()))?;
        c.keymap().with_context(|| format!("in {}", p.display()))?;
        c.apply_colors(Theme::new(true))
            .with_context(|| format!("in {}", p.display()))?;
        Ok(c)
    }

    pub fn hosts(&self) -> Vec<String> {
        hosts_of(&self.host)
    }

    pub fn preview_limit(&self) -> u64 {
        self.preview_max_kb.unwrap_or(64) * 1024
    }

    pub fn diff_limit(&self) -> u64 {
        self.diff_max_mb.unwrap_or(2) * 1024 * 1024
    }

    pub fn memory_cache(&self) -> u64 {
        self.memory_cache_mb.unwrap_or(256) << 20
    }

    pub fn disk_cache(&self) -> u64 {
        self.disk_cache_mb.unwrap_or(2048) << 20
    }

    /// `restore_dir` with `~` expanded, if set.
    pub fn restore_dir(&self, home: Option<&Path>) -> Option<PathBuf> {
        Some(expand_home(self.restore_dir.as_ref()?, home))
    }

    /// The `[keys]` overrides.
    pub fn keymap(&self) -> Result<HashMap<(KeyCode, KeyModifiers), Action>> {
        let mut out = HashMap::new();
        for (name, key) in &self.keys {
            let action = action_named(name).with_context(|| {
                format!(
                    "unknown action \"{name}\" in [keys]. Actions: {}",
                    ACTIONS
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            out.insert(parse_key(key)?, action);
        }
        Ok(out)
    }

    /// The theme with the `[colors]` overrides.
    pub fn apply_colors(&self, mut t: Theme) -> Result<Theme> {
        for (name, value) in &self.colors {
            let c: Color = value.parse().map_err(|_| {
                anyhow::anyhow!("\"{value}\" isn't a colour (a name, \"#rrggbb\" or 0–255)")
            })?;
            let fg = |s: ratatui::style::Style| s.fg(c);
            match name.as_str() {
                "accent" => t.accent = fg(t.accent),
                "added" => t.added = fg(t.added),
                "changed" => t.changed = fg(t.changed),
                "deleted" => t.deleted = fg(t.deleted),
                "live" => t.live = fg(t.live),
                "dim" => {
                    t.dim = fg(t.dim);
                    t.dim2 = fg(t.dim2);
                }
                "dir" => t.dir = fg(t.dir),
                "code" => t.code = fg(t.code),
                "selected" => t.selected = t.selected.bg(c),
                _ => bail!(
                    "unknown colour \"{name}\" in [colors]. Colours: accent, added, changed, deleted, live, dim, dir, code, selected"
                ),
            }
        }
        Ok(t)
    }
}

/// Actions that keys can be bound to.
const ACTIONS: &[(&str, Action)] = &[
    ("down", Action::Down(1)),
    ("up", Action::Up(1)),
    ("top", Action::Top),
    ("bottom", Action::Bottom),
    ("half_down", Action::HalfDown),
    ("half_up", Action::HalfUp),
    ("parent", Action::Parent),
    ("open", Action::Open),
    ("versions", Action::Open),
    ("root", Action::Root),
    ("older_change", Action::OlderChange),
    ("newer_change", Action::NewerChange),
    ("older_snapshot", Action::OlderSnapshot),
    ("newer_snapshot", Action::NewerSnapshot),
    ("oldest_change", Action::OldestChange),
    ("newest_change", Action::NewestChange),
    ("older_item_change", Action::OlderItemChange),
    ("newer_item_change", Action::NewerItemChange),
    ("preview_mode", Action::TogglePreview),
    ("scroll_down", Action::Scroll(3)),
    ("scroll_up", Action::Scroll(-3)),
    ("deleted", Action::ToggleDeleted),
    ("diff", Action::Diff),
    ("select", Action::ToggleMark),
    ("visual", Action::Visual),
    ("yank", Action::Yank),
    ("paste", Action::Paste),
    ("overwrite", Action::PasteOver),
    ("restore", Action::RestoreDialog),
    ("show", Action::Pager),
    ("search", Action::Search),
    ("filter", Action::FilterInput),
    ("find", Action::Find),
    ("next_match", Action::NextMatch),
    ("prev_match", Action::PrevMatch),
    ("command", Action::CommandLine),
    ("zoom_in", Action::ZoomIn),
    ("zoom_out", Action::ZoomOut),
    ("help", Action::Help),
    ("quit", Action::Quit),
];

fn action_named(name: &str) -> Option<Action> {
    ACTIONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| a.clone())
}

/// `i`, `C-d`, `Enter`, `Tab`, `Space`, `Backspace`, `Left` … `F5`.
pub fn parse_key(s: &str) -> Result<(KeyCode, KeyModifiers)> {
    let (mods, rest) = match s.strip_prefix("C-") {
        Some(r) => (KeyModifiers::CONTROL, r),
        None => (KeyModifiers::NONE, s),
    };
    let code = match rest {
        "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "Space" => KeyCode::Char(' '),
        "Backspace" => KeyCode::Backspace,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        f if f.starts_with('F') && f.len() > 1 => KeyCode::F(f[1..].parse().context("bad F key")?),
        c if c.chars().count() == 1 => KeyCode::Char(c.chars().next().expect("one char")),
        _ => bail!("\"{s}\" isn't a key (try \"i\", \"C-d\", \"Enter\", \"Space\", \"F5\")"),
    };
    Ok((code, mods))
}

#[cfg(test)]
mod tests {
    #[test]
    fn config_path_prefers_xdg_then_dot_config() {
        let p = |x: Option<&str>| super::config_path(x.map(Into::into), Some("/h".into()));
        assert_eq!(p(None), Some("/h/.config/restoric/config.toml".into()));
        assert_eq!(p(Some("/x")), Some("/x/restoric/config.toml".into()));
        assert_eq!(
            p(Some("rel")),
            Some("/h/.config/restoric/config.toml".into())
        );
    }

    use super::*;

    #[test]
    fn parses_every_key() {
        let c: Config = toml::from_str(
            r##"
host = ["new-name", "old-name"]
tag = "laptop"
icons = false
preview_max_kb = 128
restore_dir = "~/Got back"

[keys]
versions = "i"
half_down = "C-f"

[colors]
accent = "magenta"
selected = "#202830"
"##,
        )
        .unwrap();
        assert_eq!(c.hosts(), ["new-name", "old-name"]);
        assert_eq!(c.preview_limit(), 128 * 1024);
        assert_eq!(c.diff_limit(), 2 << 20);
        assert_eq!(
            c.restore_dir(Some(Path::new("/home/me"))),
            Some(PathBuf::from("/home/me/Got back"))
        );
        let keys = c.keymap().unwrap();
        assert_eq!(
            keys[&(KeyCode::Char('i'), KeyModifiers::NONE)],
            Action::Open
        );
        assert_eq!(
            keys[&(KeyCode::Char('f'), KeyModifiers::CONTROL)],
            Action::HalfDown
        );
        let t = c.apply_colors(Theme::new(true)).unwrap();
        assert_eq!(t.accent.fg, Some(Color::Magenta));
    }

    #[test]
    fn explains_mistakes() {
        let bad = |s: &str| -> String {
            let c: Result<Config, _> = toml::from_str(s);
            match c {
                Err(e) => e.to_string(),
                Ok(c) => format!(
                    "{:#}",
                    c.keymap()
                        .err()
                        .or_else(|| c.apply_colors(Theme::new(true)).err())
                        .unwrap()
                ),
            }
        };
        assert!(bad("hots = 1").contains("unknown field"));
        assert!(bad("[[repo]]\nrepository = \"x\"\npaths = []").contains("unknown field"));
        assert!(bad("[[repo]]\ntag = \"x\"").contains("missing field `repository`"));
        assert!(bad("[keys]\nfly = \"x\"").contains("unknown action \"fly\""));
        assert!(bad("[keys]\nopen = \"Hyper\"").contains("isn't a key"));
        assert!(bad("[colors]\naccent = \"chartreuse!\"").contains("isn't a colour"));
        assert_eq!(Config::default().hosts(), Vec::<String>::new());
    }

    #[test]
    fn parses_repos() {
        let c: Config = toml::from_str(
            r#"
host = "top"

[[repo]]
repository = "rest:http://iridium:8000/dev-vm"
insecure_no_password = true

[[repo]]
repository = "/mnt/photos"
password_file = "~/.photos-pw"
password_command = "pass photos"
host = ["a", "b"]
tag = "photos"
"#,
        )
        .unwrap();
        assert_eq!(c.repo.len(), 2);
        assert!(c.repo[0].insecure_no_password);
        assert_eq!(c.repo[0].hosts(), Vec::<String>::new());
        assert_eq!(c.repo[1].hosts(), ["a", "b"]);
        assert_eq!(c.repo[1].tag.as_deref(), Some("photos"));
        assert_eq!(c.hosts(), ["top"]);
    }

    #[test]
    fn flags_then_the_repo_entry_then_the_environment() {
        let home = Some(Path::new("/home/me"));
        let env = Access {
            repo: Some("env-repo".into()),
            password: Some("env-pw".into()),
            ..Access::default()
        };
        let entry = RepoEntry {
            repository: "entry-repo".into(),
            password_file: Some("~/pw".into()),
            ..RepoEntry::default()
        };
        let none = Access::default();

        // The entry's repository and password beat the environment's.
        let a = resolve_access(&none, &env, Some(&entry), home);
        assert_eq!(a.repo.as_deref(), Some("entry-repo"));
        assert_eq!(a.password, None);
        assert_eq!(a.password_file, Some(PathBuf::from("/home/me/pw")));

        // An entry that doesn't say how to unlock it takes the environment's.
        let bare = RepoEntry {
            repository: "entry-repo".into(),
            ..RepoEntry::default()
        };
        let a = resolve_access(&none, &env, Some(&bare), home);
        assert_eq!(a.repo.as_deref(), Some("entry-repo"));
        assert_eq!(a.password.as_deref(), Some("env-pw"));

        // A password flag beats the entry.
        let flags = Access {
            password_command: Some("pass x".into()),
            ..Access::default()
        };
        let a = resolve_access(&flags, &env, Some(&entry), home);
        assert_eq!(a.repo.as_deref(), Some("entry-repo"));
        assert_eq!(a.password_command.as_deref(), Some("pass x"));
        assert_eq!(a.password_file, None);

        // --repo leaves the entry out, password and all.
        let flags = Access {
            repo: Some("flag-repo".into()),
            ..Access::default()
        };
        let no_pw = RepoEntry {
            insecure_no_password: true,
            ..entry.clone()
        };
        let a = resolve_access(&flags, &env, Some(&no_pw), home);
        assert_eq!(a.repo.as_deref(), Some("flag-repo"));
        assert!(!a.no_password);
        assert_eq!(a.password.as_deref(), Some("env-pw"));

        // No entry: the environment.
        let a = resolve_access(&none, &env, None, home);
        assert_eq!(a, env);
    }
}
