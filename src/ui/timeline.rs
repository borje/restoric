//! The header and the timeline (PLAN.md §3.1 rows 0–4).

use std::collections::BTreeMap;

use jiff::ToSpan;
use jiff::civil::Weekday;

use super::{Grid, fmt};
use crate::app::{Action, App};

/// Where snapshots fall on the timeline's columns.
pub struct Axis {
    pub x0: u16,
    pub width: u16,
    a: f64,
    b: f64,
    t0: f64,
    t1: f64,
}

fn secs(t: jiff::Timestamp) -> f64 {
    t.as_second() as f64 + f64::from(t.subsec_nanosecond()) / 1e9
}

impl Axis {
    pub fn new(app: &App, cols: u16) -> Option<Self> {
        let set = app.set();
        let (first, last) = (set.first()?, set.last()?);
        let (t0, t1) = (secs(first.time), secs(last.time));
        let span = (t1 - t0) / f64::from(app.zoom);
        let sel = secs(set[app.idx()].time);
        let a = (sel - span / 2.0).max(t0).min(t1 - span);
        Some(Axis {
            x0: 2,
            width: cols.saturating_sub(26).max(10),
            a,
            b: a + span,
            t0,
            t1,
        })
    }

    pub fn col(&self, t: f64) -> u16 {
        if self.b <= self.a {
            return self.x0;
        }
        let f = ((t - self.a) / (self.b - self.a)).clamp(0.0, 1.0);
        self.x0 + (f * f64::from(self.width - 1)).round() as u16
    }

    fn visible(&self, t: f64) -> bool {
        t >= self.a - 1.0 && t <= self.b + 1.0
    }

    /// Snapshot indices by column.
    pub fn columns(&self, app: &App) -> BTreeMap<u16, Vec<usize>> {
        let mut by: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
        for (i, s) in app.set().iter().enumerate() {
            let t = secs(s.time);
            if self.visible(t) {
                by.entry(self.col(t)).or_default().push(i);
            }
        }
        by
    }

    /// How many snapshots share the viewed snapshot's column.
    pub fn share(&self, app: &App) -> usize {
        let c = self.col(secs(app.set()[app.idx()].time));
        self.columns(app).get(&c).map_or(0, Vec::len)
    }
}

/// Row 0: `restoric`, the breadcrumb, and the version or indexing progress.
pub fn header(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let cols = g.cols();
    let mut c = g.put(1, 0, "restoric", t.accent.patch(t.bold)) + 2;

    // The backup root is one part; folders below it are one part each.
    let at_root = app.folder == app.root;
    let root = fmt::path(&app.root, app.home.as_deref());
    let style = if at_root { t.bold } else { t.accent };
    c = g.put_act(c, 0, &root, style, Action::GoFolder(app.root.clone()));
    if let Ok(rest) = app.folder.strip_prefix(&app.root) {
        let parts: Vec<_> = rest.components().collect();
        let mut path = app.root.clone();
        for (k, p) in parts.iter().enumerate() {
            path.push(p);
            c = g.put(c, 0, "/", t.dim);
            let last = k + 1 == parts.len();
            let name = p.as_os_str().to_string_lossy();
            c = if last {
                g.put(c, 0, &name, t.bold)
            } else {
                g.put_act(c, 0, &name, t.accent, Action::GoFolder(path.clone()))
            };
        }
    }

    let state = app.state();
    let right: Vec<(String, ratatui::style::Style, Option<Action>)> = match state {
        Some(s) if !s.loaded() => {
            let p = match s.progress {
                Some((d, n)) => format!("indexing {d}/{n}"),
                None => "indexing…".to_string(),
            };
            vec![(p, t.dim, None)]
        }
        Some(s) => {
            let versions = s.versions();
            let i = app.idx();
            let label = match versions.iter().position(|&v| v == i) {
                Some(k) => format!("version {} of {}", k + 1, versions.len()),
                None => "between versions".to_string(),
            };
            let arrows = t.accent.patch(t.bold);
            vec![
                ("◀ ".to_string(), arrows, Some(Action::OlderChange)),
                (label, t.text, None),
                (" ▶".to_string(), arrows, Some(Action::NewerChange)),
            ]
        }
        None => vec![],
    };
    let len: usize = right.iter().map(|(s, _, _)| fmt::width(s)).sum();
    let mut rc = (cols as usize).saturating_sub(1 + len).max(c as usize + 1) as u16;
    for (s, style, a) in right {
        rc = match a {
            Some(a) => g.put_act(rc, 0, &s, style, a),
            None => g.put(rc, 0, &s, style),
        };
    }
}

/// Rows 1–4: date labels, the folder's track, the item's track (M3), the caret.
pub fn draw(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let cols = g.cols();
    let Some(axis) = Axis::new(app, cols) else {
        return;
    };
    let set = app.set();
    let tz = &app.tz;
    let (x0, tw) = (axis.x0, axis.width);
    let r = 1;

    // Labels: the first month (with the year), then each new month that fits.
    let a = jiff::Timestamp::from_second(axis.a as i64).unwrap_or(set[0].time);
    let za = a.to_zoned(tz.clone());
    let first = if app.zoom == 1 {
        za.strftime("%b %Y").to_string()
    } else {
        fmt::day(a, tz)
    };
    let mut end = g.put(x0, r, &first, t.dim) + 1;
    if let Ok(start) = za.start_of_day() {
        let mut d = start;
        loop {
            d = match d.checked_add(1.day()) {
                Ok(d) => d,
                Err(_) => break,
            };
            if secs(d.timestamp()) > axis.b {
                break;
            }
            let m1 = d.day() == 1;
            let monday = d.weekday() == Weekday::Monday;
            if !(m1 || (app.zoom > 1 && monday)) {
                continue;
            }
            let txt = if m1 {
                d.strftime("%b").to_string()
            } else {
                d.day().to_string()
            };
            let c = axis.col(secs(d.timestamp()));
            if c > end && c + (fmt::width(&txt) as u16) < x0 + tw {
                g.put(c, r, &txt, if m1 { t.dim } else { t.dim2 });
                end = c + fmt::width(&txt) as u16 + 1;
            }
        }
    }
    g.put(x0 + tw + 3, r, "now", t.dim);

    // Row 1: changes in this folder.
    let state = app.state();
    let rr = r + 1;
    if axis.a > axis.t0 {
        g.put(0, rr, "‹", t.accent);
    }
    if axis.b < axis.t1 {
        g.put(x0 + tw, rr, "›", t.accent);
    }
    g.put(x0 + tw + 2, rr, "┊", t.dim2);
    let sel = app.idx();
    for (c, list) in axis.columns(app) {
        let change = |i: &usize| state.is_some_and(|s| s.is_change(*i) && s.exists(*i));
        let ch = list.iter().any(change);
        let exists = list.iter().any(|&i| state.is_none_or(|s| s.exists(i)));
        let style = if list.contains(&sel) {
            t.marker
        } else if ch {
            t.text
        } else {
            t.dim2
        };
        let sym = if ch {
            "●"
        } else if exists {
            "·"
        } else {
            " "
        };
        let target = list.iter().copied().find(change).unwrap_or(list[0]);
        g.put_act(c, rr, sym, style, Action::GoSnapshot(target));
    }
    g.put(x0 + tw + 4, rr, "·", t.dim2);
    let name = app
        .folder
        .file_name()
        .map(|n| format!("{}/", n.to_string_lossy()))
        .unwrap_or_else(|| "/".to_string());
    let lx = x0 + tw + 6;
    g.put(
        lx,
        rr,
        &fmt::fit(&name, cols.saturating_sub(lx + 1) as usize),
        t.bold,
    );

    // Row 2 (the selected item's changes) arrives in M3. Then the caret and zoom.
    let cr = r + 3;
    g.put(axis.col(secs(set[sel].time)), cr, "▲", t.accent);
    let zc = cols.saturating_sub(9);
    let c = g.put(zc, cr, "−", t.accent.patch(t.bold)) + 1;
    let c = g.put(c, cr, &format!("{}×", app.zoom), t.dim) + 1;
    g.put(c, cr, "+", t.accent.patch(t.bold));
}
