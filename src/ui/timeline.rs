//! The header and the timeline (PLAN.md §3.1 rows 0–4, §3.9).

use std::collections::BTreeMap;

use jiff::ToSpan;
use jiff::civil::Weekday;
use ratatui::style::Style;

use super::{Grid, fmt};
use crate::app::{Action, App, Track, View};
use crate::repo::SnapshotInfo;

fn secs(t: jiff::Timestamp) -> f64 {
    t.as_second() as f64 + f64::from(t.subsec_nanosecond()) / 1e9
}

/// Where snapshots fall on the timeline's columns.
pub struct Axis<'a> {
    pub set: &'a [SnapshotInfo],
    pub x0: u16,
    pub width: u16,
    a: f64,
    b: f64,
    t0: f64,
    t1: f64,
}

impl<'a> Axis<'a> {
    /// The axis over `set`, centred on `set[sel]` when zoomed in.
    pub fn new(set: &'a [SnapshotInfo], sel: usize, zoom: u32, cols: u16) -> Option<Self> {
        let (first, last) = (set.first()?, set.last()?);
        let (t0, t1) = (secs(first.time), secs(last.time));
        let span = (t1 - t0) / f64::from(zoom);
        let at = secs(set.get(sel)?.time);
        let a = (at - span / 2.0).max(t0).min(t1 - span);
        Some(Axis {
            set,
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

    pub fn col_of(&self, i: usize) -> u16 {
        self.col(secs(self.set[i].time))
    }

    fn visible(&self, t: f64) -> bool {
        t >= self.a - 1.0 && t <= self.b + 1.0
    }

    /// Snapshot indices by column.
    pub fn columns(&self) -> BTreeMap<u16, Vec<usize>> {
        let mut by: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
        for (i, s) in self.set.iter().enumerate() {
            let t = secs(s.time);
            if self.visible(t) {
                by.entry(self.col(t)).or_default().push(i);
            }
        }
        by
    }

    /// How many snapshots share snapshot `i`'s column.
    pub fn share(&self, i: usize) -> usize {
        self.columns().get(&self.col_of(i)).map_or(0, Vec::len)
    }
}

/// One row of dots.
pub struct TrackRow {
    /// Per snapshot of the axis: changed here, exists here.
    pub change: Vec<bool>,
    pub exists: Vec<bool>,
    pub on: &'static str,
    pub style: Style,
    /// The selected snapshot's cell is marked only where it changed.
    pub mark_only_changes: bool,
    /// Whether it changed on disk since the newest snapshot.
    pub live: Option<bool>,
    pub label: String,
    pub label_style: Style,
    /// Shown instead of dots when there's nothing to track.
    pub empty: Option<&'static str>,
}

/// A track's dots on the axis of `set`, mapped by snapshot id.
pub fn track_row(set: &[SnapshotInfo], t: Option<&Track>) -> (Vec<bool>, Vec<bool>) {
    let Some(t) = t.filter(|t| t.loaded()) else {
        return (vec![false; set.len()], vec![true; set.len()]);
    };
    let same = t.set.len() == set.len();
    let at = |j: usize| -> Option<usize> { if same { Some(j) } else { t.index_of(set[j].id) } };
    let change = (0..set.len())
        .map(|j| at(j).is_some_and(|k| t.is_change(k) && t.exists(k)))
        .collect();
    let exists = (0..set.len())
        .map(|j| at(j).is_some_and(|k| t.exists(k)))
        .collect();
    (change, exists)
}

/// Row 0: `restoric`, the breadcrumb, and the version or indexing progress.
pub fn header(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let cols = g.cols();
    let mut c = g.put(1, 0, "restoric", t.accent.patch(t.bold)) + 2;
    let mut right: Vec<(String, Style, Option<Action>)> = Vec::new();

    if let View::Versions(v) = &app.view {
        c = g.put(c, 0, "Versions  ", t.bold);
        c = g.put(c, 0, &fmt::path(&v.path, app.home.as_deref()), t.accent);
        if let Some(tr) = app.tracks.get(&v.path).filter(|t| t.loaded()) {
            let n = tr.runs.iter().filter(|r| r.exists).count();
            let s = if n == 1 { "" } else { "s" };
            right.push((
                format!("{n} version{s} in {} snapshots", tr.set.len()),
                t.dim,
                None,
            ));
        }
    } else {
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
                let name = p.as_os_str().to_string_lossy();
                c = if k + 1 == parts.len() {
                    g.put(c, 0, &name, t.bold)
                } else {
                    g.put_act(c, 0, &name, t.accent, Action::GoFolder(path.clone()))
                };
            }
        }
        match app.state() {
            Some(s) if !s.loaded() => {
                let p = match s.progress {
                    Some((d, n)) => format!("indexing {d}/{n}"),
                    None => "indexing…".to_string(),
                };
                right.push((p, t.dim, None));
            }
            Some(s) => {
                let versions = s.versions();
                let i = app.idx();
                let label = match versions.iter().position(|&v| v == i) {
                    Some(k) => format!("version {} of {}", k + 1, versions.len()),
                    None => "between versions".to_string(),
                };
                let arrows = t.accent.patch(t.bold);
                right.push(("◀ ".into(), arrows, Some(Action::OlderChange)));
                right.push((label, t.text, None));
                right.push((" ▶".into(), arrows, Some(Action::NewerChange)));
            }
            None => {}
        }
    }

    let len: usize = right.iter().map(|(s, _, _)| fmt::width(s)).sum();
    let mut rc = (cols as usize).saturating_sub(1 + len).max(c as usize + 1) as u16;
    for (s, style, a) in right {
        rc = match a {
            Some(a) => g.put_act(rc, 0, &s, style, a),
            None => g.put(rc, 0, &s, style),
        };
    }
}

/// Rows 1 to 2 + tracks: date labels, one row per track, and the caret
/// with the zoom control. Clicking a dot does `pick(snapshot)`.
pub fn draw(
    g: &mut Grid,
    axis: &Axis,
    zoom: u32,
    sel: usize,
    tracks: &[TrackRow],
    tz: &jiff::tz::TimeZone,
    pick: &dyn Fn(usize) -> Action,
) {
    let t = g.theme.clone();
    let cols = g.cols();
    let (x0, tw) = (axis.x0, axis.width);
    let r = 1;

    // Labels: the first month (with the year), then each new month that fits.
    let a = jiff::Timestamp::from_second(axis.a as i64).unwrap_or(axis.set[0].time);
    let za = a.to_zoned(tz.clone());
    let first = if zoom == 1 {
        za.strftime("%b %Y").to_string()
    } else {
        fmt::day(a, tz)
    };
    let mut end = g.put(x0, r, &first, t.dim) + 1;
    if let Ok(mut d) = za.start_of_day() {
        while let Ok(next) = d.checked_add(1.day()) {
            d = next;
            if secs(d.timestamp()) > axis.b {
                break;
            }
            let m1 = d.day() == 1;
            if !(m1 || (zoom > 1 && d.weekday() == Weekday::Monday)) {
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

    let columns = axis.columns();
    for (k, tr) in tracks.iter().enumerate() {
        let rr = r + 1 + k as u16;
        if axis.a > axis.t0 {
            g.put(0, rr, "‹", t.accent);
        }
        if axis.b < axis.t1 {
            g.put(x0 + tw, rr, "›", t.accent);
        }
        g.put(x0 + tw + 2, rr, "┊", t.dim2);
        if let Some(empty) = tr.empty {
            g.put_to(x0, rr, empty, t.dim2, x0 + tw);
            continue;
        }
        for (&c, list) in &columns {
            let ch = list.iter().any(|&i| tr.change[i]);
            let exists = list.iter().any(|&i| tr.exists[i]);
            let style = if list.contains(&sel) && (!tr.mark_only_changes || ch) {
                t.marker
            } else if ch {
                tr.style
            } else {
                t.dim2
            };
            let sym = if ch {
                tr.on
            } else if exists {
                "·"
            } else {
                " "
            };
            let target = list
                .iter()
                .copied()
                .find(|&i| tr.change[i])
                .unwrap_or(list[0]);
            g.put_act(c, rr, sym, style, pick(target));
        }
        match tr.live {
            Some(true) => g.put(x0 + tw + 4, rr, tr.on, t.live),
            _ => g.put(x0 + tw + 4, rr, "·", t.dim2),
        };
        let lx = x0 + tw + 6;
        let w = cols.saturating_sub(lx + 1) as usize;
        g.put(lx, rr, &fmt::fit(&tr.label, w), tr.label_style);
    }

    let cr = r + 1 + tracks.len() as u16;
    g.put(axis.col_of(sel), cr, "▲", t.accent);
    let zc = cols.saturating_sub(9);
    let c = g.put_act(zc, cr, "−", t.accent.patch(t.bold), Action::ZoomOut) + 1;
    let c = g.put(c, cr, &format!("{zoom}×"), t.dim) + 1;
    g.put_act(c, cr, "+", t.accent.patch(t.bold), Action::ZoomIn);
}

/// The folder view's timeline: the folder's changes and the selected item's.
pub fn draw_folder(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let set = app.set();
    let sel = app.idx();
    let Some(axis) = Axis::new(&set, sel, app.zoom, g.cols()) else {
        return;
    };
    let name = app
        .folder
        .file_name()
        .map(|n| format!("{}/", n.to_string_lossy()))
        .unwrap_or_else(|| "/".to_string());
    let (change, exists) = track_row(&set, app.state());
    let folder = TrackRow {
        change,
        exists,
        on: "●",
        style: t.text,
        mark_only_changes: false,
        live: app.live.get(&app.folder).map(|c| !c.is_empty()),
        label: name,
        label_style: t.bold,
        empty: None,
    };
    let item = match (app.selected(), app.selected_path()) {
        (Some(e), Some(path)) => {
            let (change, exists) = track_row(&set, app.tracks.get(&path));
            let mut label = e.node.name.to_string_lossy().into_owned();
            if e.is_dir() {
                label.push('/');
            }
            TrackRow {
                change,
                exists,
                on: "•",
                style: t.changed,
                mark_only_changes: true,
                live: app.live.get(&path).map(|c| !c.is_empty()),
                label,
                label_style: t.changed,
                empty: None,
            }
        }
        _ => TrackRow {
            change: Vec::new(),
            exists: Vec::new(),
            on: "•",
            style: t.changed,
            mark_only_changes: true,
            live: None,
            label: String::new(),
            label_style: t.dim,
            empty: Some("select a file or folder to see its changes here"),
        },
    };
    draw(
        g,
        &axis,
        app.zoom,
        sel,
        &[folder, item],
        &app.tz,
        &Action::GoSnapshot,
    );
}
