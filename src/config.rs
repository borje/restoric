//! `~/.config/restoric/config.toml` (PLAN.md §10). Every key is optional.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Color;
use serde::Deserialize;

use crate::app::Action;
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
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "restoric")
            .map(|d| d.config_dir().join("config.toml"))
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
        match &self.host {
            Some(Hosts::One(h)) => vec![h.clone()],
            Some(Hosts::Many(h)) => h.clone(),
            None => Vec::new(),
        }
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
        let d = self.restore_dir.as_ref()?;
        Some(match (d.strip_prefix("~/"), home) {
            (Some(rest), Some(h)) => h.join(rest),
            _ if d == "~" => home
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(d)),
            _ => PathBuf::from(d),
        })
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
        assert!(bad("[keys]\nfly = \"x\"").contains("unknown action \"fly\""));
        assert!(bad("[keys]\nopen = \"Hyper\"").contains("isn't a key"));
        assert!(bad("[colors]\naccent = \"chartreuse!\"").contains("isn't a colour"));
        assert_eq!(Config::default().hosts(), Vec::<String>::new());
    }
}
