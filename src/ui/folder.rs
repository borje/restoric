//! The Versions column and the listing (PLAN.md §3.1). The preview column
//! comes in M3.

use ratatui::layout::Rect;

use super::{Grid, TOP, fmt, icons, panes};
use crate::app::{Action, App, Row};
use crate::index::folder::Counts;
use crate::index::listing::Delta;

/// Writes `+1~3−1` (parts joined by `gap`) at (x, y), stopping before `end`.
pub fn put_counts(g: &mut Grid, x: u16, y: u16, c: &Counts, gap: &str, end: u16) -> u16 {
    let t = g.theme.clone();
    let parts = [
        (c.added, "+", t.added),
        (c.changed, "~", t.changed),
        (c.deleted, "−", t.deleted),
    ];
    let mut x = x;
    let mut first = true;
    for (n, sign, style) in parts {
        if n == 0 {
            continue;
        }
        let s = format!("{}{sign}{n}", if first { "" } else { gap });
        x = g.put_to(x, y, &s, style, end);
        first = false;
    }
    x
}

enum VRow {
    /// A version: the snapshot index where the folder changed.
    Version(usize),
    /// Snapshots `from..=to` where nothing changed.
    Fold { from: usize, to: usize },
}

pub fn draw(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let p = panes(g.cols());
    let bottom = g.rows() - 2;
    let height = (bottom - TOP + 1) as usize;
    let i = app.idx();

    // Versions column.
    if let Some((v0, v1)) = p.versions {
        g.vline(v1 + 1, TOP, bottom);
        g.hits.versions = Some(Rect::new(
            g.area.x + v0,
            g.area.y + TOP,
            v1 - v0 + 1,
            height as u16,
        ));
        match app.state() {
            Some(s) if s.loaded() => {
                let mut rows = Vec::new();
                let versions = s.versions();
                for &v in versions.iter().rev() {
                    let next = s
                        .points
                        .iter()
                        .map(|p| p.index)
                        .find(|&x| x > v)
                        .unwrap_or(s.set.len());
                    if next > v + 1 {
                        rows.push(VRow::Fold {
                            from: v + 1,
                            to: next - 1,
                        });
                    }
                    rows.push(VRow::Version(v));
                }
                let on = |r: &VRow| match r {
                    VRow::Version(v) => *v == i,
                    VRow::Fold { from, to } => (*from..=*to).contains(&i),
                };
                let sr = rows.iter().position(on).unwrap_or(0);
                let off = sr
                    .saturating_sub(height / 2)
                    .min(rows.len().saturating_sub(height));
                for (k, row) in rows.iter().skip(off).take(height).enumerate() {
                    let y = TOP + k as u16;
                    let here = on(row);
                    match row {
                        VRow::Fold { from, to } => {
                            if here {
                                g.put(v0, y, "▶", t.accent);
                            }
                            let label = format!("┄ {} unchanged ┄", to - from + 1);
                            let style = if here { t.accent } else { t.dim2 };
                            g.put_to(v0 + 3, y, &label, style, v1 + 1);
                            g.hit(v0, v1 + 1, y, Action::GoSnapshot(*to));
                        }
                        VRow::Version(v) => {
                            if here {
                                g.fill(y, v0, v1, t.selected);
                                g.put(v0, y, "▶", t.accent);
                            }
                            let date = fmt::time(s.set[*v].time, &app.tz);
                            g.put(v0 + 1, y, &date, if here { t.bold } else { t.text });
                            if let Some(c) = s.counts.get(v) {
                                put_counts(g, v0 + 14, y, c, "", v1 + 1);
                            }
                            g.hit(v0, v1 + 1, y, Action::GoSnapshot(*v));
                        }
                    }
                }
            }
            _ => {
                g.put(v0 + 1, TOP, "indexing…", t.dim);
            }
        }
    }

    // Listing.
    let (l0, l1) = p.listing;
    g.hits.listing = Some(Rect::new(
        g.area.x + l0,
        g.area.y + TOP,
        l1 - l0 + 1,
        height as u16,
    ));
    let dx = l1 - 5;
    let se = l1 - 7;
    let nw = se.saturating_sub(6 + l0 + 3) as usize;
    let rows = app.rows();
    let off = app
        .sel
        .saturating_sub(height / 2)
        .min(rows.len().saturating_sub(height));
    for (k, row) in rows.iter().enumerate().skip(off).take(height) {
        let y = TOP + (k - off) as u16;
        if k == app.sel {
            g.fill(y, l0, l1, t.selected);
        }
        g.hit(l0, l1 + 1, y, Action::ClickRow(k));
        let Some(e) = app.entry(*row) else {
            if *row == Row::Up {
                g.put(l0 + 3, y, "..", t.dir);
            }
            continue;
        };
        let (icon, icon_style) = icons::icon(&e.node, &t);
        g.put(l0 + 1, y, icon, icon_style);
        let gone = e.is_deleted();
        let mut name = e.node.name.to_string_lossy().into_owned();
        if e.is_dir() {
            name.push('/');
        }
        let style = if gone {
            t.deleted.add_modifier(t.strike)
        } else if e.is_dir() {
            t.dir
        } else {
            t.text
        };
        g.put(l0 + 3, y, &fmt::fit(&name, nw), style);
        if e.is_dir() {
            match &e.delta {
                Delta::Deleted => {
                    g.put(dx, y, "−", t.deleted);
                }
                Delta::Counts(c) => {
                    put_counts(g, dx, y, c, "", l1 + 1);
                }
                _ => {}
            }
        } else {
            let s = fmt::size(e.node.size);
            g.put(
                se - fmt::width(&s) as u16,
                y,
                &s,
                if gone { t.dim } else { t.text },
            );
            let (mark, style) = match e.delta {
                Delta::Added => ("+", t.added),
                Delta::Changed => ("~", t.changed),
                Delta::Deleted => ("−", t.deleted),
                _ => ("", t.text),
            };
            g.put(dx, y, mark, style);
        }
    }
    let y = TOP + (rows.len() - off).min(height) as u16;
    match app.listing() {
        None if y <= bottom => {
            g.put(l0 + 3, y, "loading…", t.dim);
        }
        Some(None) if y <= bottom => {
            g.put(l0 + 3, y, "not in this snapshot", t.dim);
        }
        _ => {}
    }
    let hidden = app
        .deleted
        .get(&(app.snap, app.folder.clone()))
        .copied()
        .unwrap_or(0);
    if hidden > 0 && rows.len() < height {
        g.put(
            l0 + 3,
            bottom,
            &format!("{hidden} deleted · . to show"),
            t.dim2,
        );
    }

    // Preview column: its divider only, for now.
    if let Some((p0, _)) = p.preview {
        g.vline(p0 - 1, TOP, bottom);
    }
}
