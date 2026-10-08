//! The input line in the status bar (PLAN.md §3.6, §3.8, §3.15): `:`
//! commands, `/` search and `f` filter.

use jiff::civil::Date;
use jiff::{Timestamp, ToSpan};

use super::{Action, App, View};
use crate::index::Mode;
use crate::ui::fmt;
use crate::worker::Request;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputKind {
    #[default]
    Command,
    Search,
    Filter,
    /// `restore to:` on a foreign host (§3.18).
    Dir,
}

/// What's being typed in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub kind: InputKind,
    pub text: String,
    /// The selected row when typing started (search).
    pub origin: usize,
    /// The search or filter before typing started, for `esc`.
    pub prev: String,
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// A day from `:2026-09-01`, `:09-01`, `:sep 1`, `:september 1`, `:today`,
/// `:yesterday`, `:3d`, `:2w`.
pub fn parse_day(t: &str, today: Date) -> Option<Date> {
    let t = t.trim().to_lowercase();
    match t.as_str() {
        "today" => return Some(today),
        "yesterday" => return today.checked_sub(1.day()).ok(),
        _ => {}
    }
    if let Some(n) = t.strip_suffix('d').and_then(|n| n.parse::<i64>().ok()) {
        return today.checked_sub(n.days()).ok();
    }
    if let Some(n) = t.strip_suffix('w').and_then(|n| n.parse::<i64>().ok()) {
        return today.checked_sub((7 * n).days()).ok();
    }
    let nums: Vec<&str> = t.split('-').collect();
    let num = |s: &str| s.parse::<i16>().ok();
    match nums.as_slice() {
        [y, m, d] if y.len() == 4 => {
            return Date::new(num(y)?, num(m)? as i8, num(d)? as i8).ok();
        }
        [m, d] => return Date::new(today.year(), num(m)? as i8, num(d)? as i8).ok(),
        _ => {}
    }
    let (word, day) = t.split_once(char::is_whitespace)?;
    if word.len() < 3 {
        return None;
    }
    let month = MONTHS.iter().position(|m| m.starts_with(word))? as i8 + 1;
    Date::new(today.year(), month, day.trim().parse().ok()?).ok()
}

impl App {
    fn today(&self) -> Date {
        self.now
            .unwrap_or_else(Timestamp::now)
            .to_zoned(self.tz.clone())
            .date()
    }

    /// Goes to snapshot `i` of the folder's set, with a message.
    fn jump(&mut self, i: usize, why: String) {
        if !self.state().is_none_or(|s| s.exists(i)) {
            self.message = Some(format!(
                "{}/ did not exist then. Go up a folder and try again.",
                fmt::path(&self.folder, self.home.as_deref())
            ));
            return;
        }
        self.view = View::Folder;
        self.act(Action::GoSnapshot(i));
        self.message = Some(why);
    }

    /// Runs a command typed after `:`.
    pub(super) fn run_command(&mut self, text: &str) {
        let cmd = text.trim();
        let lower = cmd.to_lowercase();
        let set = self.set();
        let (word, rest) = match cmd.split_once(char::is_whitespace) {
            Some((w, r)) => (w.to_lowercase(), r.trim().to_string()),
            None => (lower.clone(), String::new()),
        };
        match (word.as_str(), rest.as_str()) {
            ("", _) => {}
            ("undo", "") => {
                if !self.restore_busy() {
                    self.outbox.push(Request::Undo);
                }
            }
            ("cancel", "") => self.stop_restore(false),
            ("q" | "quit" | "q!", "") => {
                self.act(Action::Quit);
            }
            ("help" | "h", "") => self.help = true,
            ("deleted", "") => {
                self.act(Action::ToggleDeleted);
                self.message = Some(
                    if self.ghosts {
                        "Showing deleted items."
                    } else {
                        "Hiding deleted items."
                    }
                    .into(),
                );
            }
            ("latest" | "last" | "now", "") if !set.is_empty() => {
                self.jump(set.len() - 1, "Latest snapshot.".into());
            }
            ("oldest" | "first", "") if !set.is_empty() => {
                self.jump(0, "Oldest snapshot.".into());
            }
            ("find" | "f", q) if !q.is_empty() => self.open_find(q),
            ("reload", "") => {
                self.outbox.push(Request::Reload { quiet: false });
                self.message = Some("Looking for new snapshots…".into());
            }
            ("host", h) if !h.is_empty() => {
                let mut f = self.filter.clone();
                f.hosts = h
                    .split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                self.set_filter(f);
            }
            ("tag", t) => {
                let mut f = self.filter.clone();
                f.tag = (!t.is_empty()).then(|| t.to_string());
                self.set_filter(f);
            }
            ("set", "strict") => self.set_mode(Mode::Strict),
            ("set", "nostrict") => self.set_mode(Mode::Content),
            _ => match parse_day(&lower, self.today()) {
                Some(day) => self.go_day(day),
                None => {
                    self.message = Some(format!(
                        "Unknown command \":{cmd}\". Try :sep 1, :yesterday, :3d, :find NAME, :undo"
                    ));
                }
            },
        }
    }

    /// The last snapshot on or before the end of `day`.
    fn go_day(&mut self, day: Date) {
        let set = self.set();
        let end = day
            .at(23, 59, 59, 0)
            .to_zoned(self.tz.clone())
            .map(|z| z.timestamp())
            .unwrap_or(Timestamp::MAX);
        match set.iter().rposition(|s| s.time <= end) {
            Some(i) => {
                let why = format!(
                    "Jumped to {}, the last snapshot on or before {}.",
                    fmt::time(set[i].time, &self.tz),
                    day.strftime("%b %d")
                );
                self.jump(i, why);
            }
            None => {
                let oldest = set.first().map(|s| fmt::time(s.time, &self.tz));
                self.message = Some(format!(
                    "No snapshots that early. The oldest is from {}.",
                    oldest.unwrap_or_default()
                ));
            }
        }
    }

    /// Starts typing a search (`/`), a filter (`f`) or a command (`:`).
    pub(super) fn start_input(&mut self, kind: InputKind, text: &str) {
        let prev = match kind {
            InputKind::Search => self.search.clone(),
            InputKind::Filter => self.name_filter.clone(),
            InputKind::Command | InputKind::Dir => String::new(),
        };
        self.input = Some(Input {
            kind,
            text: text.to_string(),
            origin: self.sel,
            prev,
        });
    }

    /// The text changed while typing a search or filter.
    pub(super) fn input_changed(&mut self) {
        let Some(input) = self.input.clone() else {
            return;
        };
        match input.kind {
            InputKind::Filter => {
                self.name_filter = input.text;
                self.select_first();
            }
            InputKind::Search => {
                self.search = input.text;
                if self.search.is_empty() {
                    self.select_row(input.origin);
                    return;
                }
                let rows = self.rows();
                let n = rows.len();
                if let Some(k) = (0..n)
                    .map(|d| (input.origin + d) % n)
                    .find(|&k| self.matches(rows[k]))
                {
                    self.select_row(k);
                }
            }
            InputKind::Command | InputKind::Dir => {}
        }
    }

    /// `esc` while typing.
    pub(super) fn input_cancel(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        match input.kind {
            InputKind::Search => {
                self.search = input.prev;
                self.select_row(input.origin);
            }
            InputKind::Filter => {
                self.name_filter = input.prev;
                self.select_first();
            }
            InputKind::Command => {}
            InputKind::Dir => self.pending_targets = None,
        }
    }

    /// `⏎` while typing.
    pub(super) fn input_enter(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        match input.kind {
            InputKind::Command => self.run_command(&input.text),
            InputKind::Search => {
                let rows = self.rows();
                if !input.text.is_empty() && !rows.iter().any(|r| self.matches(*r)) {
                    self.message = Some(format!(
                        "No match for \"{}\" in this folder. Press s to search every snapshot.",
                        input.text
                    ));
                }
            }
            InputKind::Filter => {}
            InputKind::Dir => self.restore_into(&input.text),
        }
    }

    /// `n` / `N`: the next or previous match of the search.
    pub(super) fn search_step(&mut self, forward: bool) -> bool {
        if self.search.is_empty() {
            self.message = Some("No search yet. Press / to search this folder.".into());
            return false;
        }
        let rows = self.rows();
        let n = rows.len();
        let found = (1..=n)
            .map(|d| {
                if forward {
                    (self.sel + d) % n
                } else {
                    (self.sel + n * 2 - d) % n
                }
            })
            .find(|&k| self.matches(rows[k]));
        match found {
            Some(k) => {
                self.select_row(k);
                true
            }
            None => {
                self.message = Some(format!(
                    "No match for \"{}\" in this folder. Press s to search every snapshot.",
                    self.search
                ));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days() {
        let today = Date::new(2026, 10, 5).unwrap();
        let d = |s| parse_day(s, today).map(|d| d.to_string());
        assert_eq!(d("2026-09-01").as_deref(), Some("2026-09-01"));
        assert_eq!(d("09-01").as_deref(), Some("2026-09-01"));
        assert_eq!(d("sep 1").as_deref(), Some("2026-09-01"));
        assert_eq!(d("September 1").as_deref(), Some("2026-09-01"));
        assert_eq!(d("today").as_deref(), Some("2026-10-05"));
        assert_eq!(d("yesterday").as_deref(), Some("2026-10-04"));
        assert_eq!(d("3d").as_deref(), Some("2026-10-02"));
        assert_eq!(d("2w").as_deref(), Some("2026-09-21"));
        assert_eq!(d("se 1"), None);
        assert_eq!(d("x"), None);
        assert_eq!(d("13-40"), None);
    }
}
