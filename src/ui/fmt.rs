//! Text formats shared by the UI and messages.

use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use unicode_width::UnicodeWidthStr;

/// Within about six months of `now`, where a date without its year is
/// unambiguous; `ls -l` draws the same line.
fn recent(t: Timestamp, now: Timestamp) -> bool {
    now.duration_since(t).abs() < SignedDuration::from_hours(24 * 182)
}

/// `Sep 06 18:03` in local time, or `Sep 06  2024` when it isn't recent:
/// the year instead of the time of day, as `ls -l` does. Both are 12
/// columns wide.
pub fn time(t: Timestamp, tz: &TimeZone, now: Timestamp) -> String {
    let f = if recent(t, now) {
        "%b %d %H:%M"
    } else {
        "%b %d  %Y"
    };
    t.to_zoned(tz.clone()).strftime(f).to_string()
}

/// `Sep 06`, or `Sep 06 2024` when it isn't recent.
pub fn day(t: Timestamp, tz: &TimeZone, now: Timestamp) -> String {
    let f = if recent(t, now) { "%b %d" } else { "%b %d %Y" };
    t.to_zoned(tz.clone()).strftime(f).to_string()
}

/// `737 B`, `1.3K`, `4.0M`, `2.1G`
pub fn size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1}K", f / K)
    } else if f < K * K * K {
        format!("{:.1}M", f / (K * K))
    } else {
        format!("{:.1}G", f / (K * K * K))
    }
}

/// The path with the home folder as `~`.
pub fn path(p: &Path, home: Option<&Path>) -> String {
    if let Some(h) = home
        && let Ok(rest) = p.strip_prefix(h)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    p.display().to_string()
}

/// `just now`, `5 min ago`, `3 h ago`, `2 d ago`
pub fn ago(d: SignedDuration) -> String {
    let s = d.as_secs().max(0);
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86_400)
    }
}

/// `p` with a leading `~` replaced by `home`.
pub fn expand_home(p: &str, home: Option<&Path>) -> PathBuf {
    match (p.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => h.join(rest),
        (_, Some(h)) if p == "~" => h.to_path_buf(),
        _ => PathBuf::from(p),
    }
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// `s` cut to `w` columns, ending in `…` when cut.
pub fn fit(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w - 1 {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// `s` cut to `w` columns in the middle, with `…` there when cut: the ends
/// of a path or URL say more than its middle.
pub fn fit_middle(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    let mut tail = Vec::new();
    let mut used = 0;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w.saturating_sub(1) / 2 {
            break;
        }
        tail.push(c);
        used += cw;
    }
    let head = fit(s, w - used);
    head + &tail.iter().rev().collect::<String>()
}

/// Words wrapped to `w` columns.
pub fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && width(&line) + 1 + width(word) > w {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(737), "737 B");
        assert_eq!(size(1331), "1.3K");
        assert_eq!(size(5 << 20), "5.0M");
    }

    #[test]
    fn fits_and_wraps() {
        assert_eq!(fit("config.go", 20), "config.go");
        assert_eq!(fit("a-very-long-name.go", 8), "a-very-…");
        let url = "sftp:nas:/a/very/long/backup/path";
        assert_eq!(fit_middle(url, 40), url);
        assert_eq!(fit_middle(url, 16), "sftp:nas…up/path");
        assert_eq!(width(&fit_middle(url, 17)), 17);
        assert_eq!(fit_middle(url, 1), "…");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    }

    #[test]
    fn paths() {
        let home = Path::new("/home/bege");
        assert_eq!(path(Path::new("/home/bege/dev"), Some(home)), "~/dev");
        assert_eq!(path(Path::new("/etc"), Some(home)), "/etc");
        assert_eq!(
            expand_home("~/x", Some(home)),
            PathBuf::from("/home/bege/x")
        );
        assert_eq!(expand_home("/x", Some(home)), PathBuf::from("/x"));
    }

    #[test]
    fn ages() {
        assert_eq!(ago(SignedDuration::from_secs(5)), "just now");
        assert_eq!(ago(SignedDuration::from_mins(7)), "7 min ago");
        assert_eq!(ago(SignedDuration::from_hours(3)), "3 h ago");
        assert_eq!(ago(SignedDuration::from_hours(50)), "2 d ago");
    }
}
