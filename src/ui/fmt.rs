//! Text formats shared by the UI and messages.

use std::path::Path;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use unicode_width::UnicodeWidthStr;

/// `Sep 06 18:03`, in local time.
pub fn time(t: Timestamp, tz: &TimeZone) -> String {
    t.to_zoned(tz.clone()).strftime("%b %d %H:%M").to_string()
}

/// `Sep 06`
pub fn day(t: Timestamp, tz: &TimeZone) -> String {
    t.to_zoned(tz.clone()).strftime("%b %d").to_string()
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
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    }

    #[test]
    fn paths() {
        let home = Path::new("/home/bege");
        assert_eq!(path(Path::new("/home/bege/dev"), Some(home)), "~/dev");
        assert_eq!(path(Path::new("/etc"), Some(home)), "/etc");
    }
}
