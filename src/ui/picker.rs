//! The repository picker screen (PLAN.md §3.18).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::theme::Theme;
use super::{Grid, fmt};
use crate::app::keys::Hits;
use crate::picker::{Level, Picker};
use crate::repos::RECHECK;

/// Draws the picker: a header, the message, a table and a status bar.
pub fn draw(p: &Picker, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let mut g = Grid {
        buf,
        area,
        theme,
        hits: Hits::default(),
    };
    let t = theme.clone();
    let (cols, rows) = (g.cols(), g.rows());
    if rows < 8 || cols < 40 {
        g.put(0, 0, "restoric: the terminal is too small", t.text);
        return;
    }
    let c = g.put(1, 0, "restoric", t.accent.patch(t.bold)) + 2;
    let title = match p.level {
        Level::Repos => "Pick a repository",
        Level::Groups => "Pick a host and path",
    };
    let c = g.put(c, 0, title, t.bold);
    if let (Level::Groups, Some(gr)) = (p.level, &p.groups) {
        let w = (cols as usize).saturating_sub(c as usize + 3);
        let loc = fmt::fit(&gr.location, w);
        g.put(
            cols.saturating_sub(1 + fmt::width(&loc) as u16),
            0,
            &loc,
            t.dim,
        );
    }

    let mut y = 1;
    if let Some(m) = &p.message {
        for line in m.lines() {
            for l in fmt::wrap(line, cols as usize - 2) {
                g.put_to(1, y, &l, t.text, cols - 1);
                y += 1;
            }
        }
    }
    y += 1;
    let bottom = rows - 2;
    match p.level {
        Level::Repos => repos(p, &mut g, y, bottom),
        Level::Groups => groups(p, &mut g, y, bottom),
    }
    status(p, &mut g);
}

/// The first row to show so that `sel` is visible in `height` rows.
fn top(sel: usize, height: usize) -> usize {
    sel.saturating_sub(height.saturating_sub(1))
}

fn repos(p: &Picker, g: &mut Grid, y: u16, bottom: u16) {
    let t = g.theme.clone();
    let cols = g.cols();
    if p.repos.is_empty() {
        g.put(3, y, "No repositories in the config.", t.dim);
        return;
    }
    let lw = p
        .repos
        .iter()
        .map(|r| fmt::width(&r.location))
        .max()
        .unwrap_or(8)
        .clamp(8, (cols / 2) as usize) as u16;
    let hx = 3 + lw + 2;
    g.put(3, y, "location", t.dim);
    g.put(hx, y, "hosts", t.dim);
    let height = bottom.saturating_sub(y + 1) as usize;
    let first = top(p.repo_sel, height);
    for (k, r) in p.repos.iter().enumerate().skip(first).take(height) {
        let row = y + 1 + (k - first) as u16;
        let on = k == p.repo_sel;
        if on {
            g.fill(row, 0, cols - 1, t.selected);
            g.put(1, row, "▶", t.accent.patch(t.bold));
        }
        let style = |s: ratatui::style::Style| if on { s.patch(t.selected) } else { s };
        g.put(3, row, &fmt::fit(&r.location, lw as usize), style(t.text));
        let (text, st) = match (&r.error, r.read) {
            (Some(_), _) => ("can't open".to_string(), t.warn),
            (None, None) => ("not read yet".to_string(), t.dim),
            (None, Some(read)) => {
                let mut s = r.hosts.join(", ");
                if s.is_empty() {
                    s = "no snapshots".to_string();
                }
                let age = p.now.duration_since(read);
                if age >= RECHECK {
                    s.push_str(&format!(" · read {}", fmt::ago(age)));
                }
                (s, if r.mine { t.text } else { t.dim })
            }
        };
        let w = (cols - 1).saturating_sub(hx) as usize;
        g.put(hx, row, &fmt::fit(&text, w), style(st));
    }
}

fn groups(p: &Picker, g: &mut Grid, y: u16, bottom: u16) {
    let t = g.theme.clone();
    let cols = g.cols();
    let Some(gr) = &p.groups else { return };
    let visible = p.visible();
    if gr.rows.is_empty() {
        g.put(3, y, "No snapshots in this repository.", t.dim);
        return;
    }
    if visible.is_empty() {
        g.put(3, y, &format!("No rows match \"{}\".", gr.filter), t.dim);
        return;
    }
    let hw = visible
        .iter()
        .map(|&i| fmt::width(&gr.rows[i].host))
        .max()
        .unwrap_or(4)
        .clamp(4, 24) as u16;
    let px = 3 + hw + 2;
    let lx = cols - 1 - 12;
    let sx = lx - 2 - 9;
    let pw = sx.saturating_sub(2 + px) as usize;
    g.put(3, y, "host", t.dim);
    g.put(px, y, "path", t.dim);
    g.put(sx, y, "snapshots", t.dim);
    g.put(lx, y, "latest", t.dim);
    let height = bottom.saturating_sub(y + 1) as usize;
    let first = top(gr.sel, height);
    for (k, &i) in visible.iter().enumerate().skip(first).take(height) {
        let r = &gr.rows[i];
        let row = y + 1 + (k - first) as u16;
        let on = k == gr.sel;
        if on {
            g.fill(row, 0, cols - 1, t.selected);
            g.put(1, row, "▶", t.accent.patch(t.bold));
        }
        let style = |s: ratatui::style::Style| if on { s.patch(t.selected) } else { s };
        let host = if r.mine { t.bold } else { t.text };
        g.put(3, row, &r.host, style(host));
        let path = fmt::path(&r.path, p.home.as_deref());
        g.put(px, row, &fmt::fit(&path, pw), style(t.text));
        g.put(sx, row, &format!("{:>9}", r.snapshots), style(t.text));
        g.put(lx, row, &fmt::time(r.latest, &p.tz), style(t.dim));
    }
}

fn status(p: &Picker, g: &mut Grid) {
    let t = g.theme.clone();
    let (cols, r) = (g.cols(), g.rows() - 1);
    g.fill(r, 0, cols - 1, t.status_bar);
    if let Some(gr) = &p.groups
        && p.level == Level::Groups
        && let Some(text) = &gr.input
    {
        let c = g.put(1, r, "filter: ", t.accent.patch(t.bold));
        let c = g.put(c, r, text, t.text);
        let c = g.put(c, r, "█", t.accent);
        let hint = "type to filter · ⏎ keep · esc clear";
        let hx = cols.saturating_sub(1 + fmt::width(hint) as u16);
        if c + 2 < hx {
            g.put(hx, r, hint, t.dim);
        }
        return;
    }
    let c = g.put(0, r, " PICK ", t.badge_magenta) + 1;
    let (hint, pos) = match p.level {
        Level::Repos => {
            let n = p.repos.len();
            (
                "⏎ open · r refresh · q quit",
                format!("{}/{n}", if n > 0 { p.repo_sel + 1 } else { 0 }),
            )
        }
        Level::Groups => {
            let n = p.visible().len();
            let sel = p.groups.as_ref().map_or(0, |g| g.sel);
            (
                if p.repos.is_empty() {
                    "⏎ open · / filter · q quit"
                } else {
                    "⏎ open · / filter · q back"
                },
                format!("{}/{n}", if n > 0 { sel + 1 } else { 0 }),
            )
        }
    };
    match &p.busy {
        Some(b) => {
            g.put(c, r, b, t.warn);
        }
        None => {
            g.put(c, r, hint, t.dim);
        }
    }
    g.put(
        cols.saturating_sub(1 + fmt::width(&pos) as u16),
        r,
        &pos,
        t.text,
    );
}
