//! The folder view's panes (PLAN.md §3.1): the Versions column, the
//! listing and the preview.

use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{Grid, TOP, fmt, icons, panes, preview};
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

/// Width of what `put_counts` writes.
pub fn counts_width(c: &Counts, gap: &str) -> u16 {
    let parts: Vec<String> = [(c.added, "+"), (c.changed, "~"), (c.deleted, "−")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, s)| format!("{s}{n}"))
        .collect();
    fmt::width(&parts.join(gap)) as u16
}

enum VRow {
    /// What changed on disk since the newest snapshot.
    Now,
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
                let mut rows = vec![VRow::Now];
                for &v in s.versions().iter().rev() {
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
                    VRow::Now => false,
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
                        VRow::Now => {
                            let c = g.put(v0 + 1, y, "now  ", t.live);
                            match app.live.get(&app.folder) {
                                Some(lc) if !lc.is_empty() => {
                                    let c = put_counts(g, c, y, lc, "", v1 + 1);
                                    g.put_to(c, y, " unsaved", t.dim, v1 + 1);
                                }
                                Some(_) => {
                                    g.put(c, y, "= latest", t.dim);
                                }
                                None => {
                                    g.put(c, y, "…", t.dim);
                                }
                            }
                        }
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
    let ghost = t.dim.add_modifier(Modifier::ITALIC);
    for (k, row) in rows.iter().enumerate().skip(off).take(height) {
        let y = TOP + (k - off) as u16;
        if k == app.sel {
            g.fill(y, l0, l1, t.selected);
        }
        g.hit(l0, l1 + 1, y, Action::ClickRow(k));
        if app.is_marked(k, *row) {
            g.put(l0, y, "┃", t.accent.patch(t.bold));
        }
        let Some(e) = app.entry(*row) else {
            if *row == Row::Up {
                g.put(l0 + 3, y, "..", t.dir);
            }
            continue;
        };
        let (icon, icon_style) = icons::icon(&e.node, &t, app.icons);
        g.put(l0 + 1, y, icon, icon_style);
        let mut name = e.node.name.to_string_lossy().into_owned();
        if e.is_dir() {
            name.push('/');
        }
        let mut style = match e.delta {
            Delta::Deleted => t.deleted.add_modifier(t.strike),
            Delta::Gone(_) => ghost,
            _ if e.is_dir() => t.dir,
            _ => t.text,
        };
        if app.matches(*row) {
            style = style.patch(t.accent).add_modifier(Modifier::UNDERLINED);
        }
        g.put(l0 + 3, y, &fmt::fit(&name, nw), style);
        if !e.is_dir() {
            let s = fmt::size(e.node.size);
            let size_style = if e.is_gone() { t.dim } else { t.text };
            g.put(se - fmt::width(&s) as u16, y, &s, size_style);
        }
        match &e.delta {
            Delta::Deleted => {
                g.put(dx, y, "−", t.deleted);
            }
            Delta::Gone(_) => {
                g.put_to(dx, y, "gone", ghost, l1 + 1);
            }
            Delta::Added => {
                g.put(dx, y, "+", t.added);
            }
            Delta::Changed => {
                g.put(dx, y, "~", t.changed);
            }
            Delta::Counts(c) => {
                put_counts(g, dx, y, c, "", l1 + 1);
            }
            Delta::Same => {}
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
    let hidden = app.hidden();
    if hidden > 0 && rows.len() < height {
        let label = format!("{hidden} deleted · . to show");
        g.put_act(l0 + 3, bottom, &label, t.dim2, Action::ToggleDeleted);
    }

    if let Some((p0, p1)) = p.preview {
        g.vline(p0 - 1, TOP, bottom);
        preview::folder_view(app, g, p0 + 1, p1, TOP, bottom);
    }
}
