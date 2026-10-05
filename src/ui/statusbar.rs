//! The status bar (PLAN.md §3.1, last row).

use ratatui::style::Style;

use super::timeline::Axis;
use super::{Grid, fmt};
use crate::app::cmdline::InputKind;
use crate::app::{Action, App, View};

pub fn draw(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let (cols, r) = (g.cols(), g.rows() - 1);
    g.fill(r, 0, cols - 1, t.status_bar);
    if let Some(input) = &app.input {
        let (label, hint) = match input.kind {
            InputKind::Command => (
                ":",
                "sep 1 · 2026-09-01 · yesterday · 3d · find NAME · undo",
            ),
            InputKind::Search => ("/", "type to search this folder · ⏎ keep · esc cancel"),
            InputKind::Filter => ("filter: ", "type to filter · ⏎ keep · esc clear"),
        };
        let c = g.put(1, r, label, t.accent.patch(t.bold));
        let c = g.put(c, r, &input.text, t.text);
        let c = g.put(c, r, "█", t.accent);
        let hx = cols.saturating_sub(1 + fmt::width(hint) as u16);
        if c + 2 < hx {
            g.put(hx, r, hint, t.dim);
        }
        return;
    }
    if app.dialog.is_some() {
        let c = g.put(0, r, " RST ", t.badge_red) + 1;
        let mid = vec![("choose where the restored copy goes".to_string(), t.dim)];
        finish(app, g, c, mid, Vec::new());
        return;
    }
    if let View::Find(f) = &app.view {
        let c = g.put(0, r, " FIND ", t.badge_magenta) + 1;
        let mid = vec![(
            "⏎ jumps to the last snapshot that has it".to_string(),
            t.dim,
        )];
        let n = f.results.len();
        let pos = format!("{}/{n}", if n > 0 { f.sel + 1 } else { 0 });
        finish(app, g, c, mid, vec![(pos, t.text, None)]);
        return;
    }
    if let View::Diff(d) = &app.view {
        let c = g.put(0, r, " DIFF ", t.badge_blue) + 1;
        let mode = match d.mode {
            crate::app::DiffMode::Disk => "vs disk (c)",
            crate::app::DiffMode::Previous => "vs previous (p)",
        };
        let mid = vec![
            (mode.to_string(), t.dim),
            ("  ]c [c changes · H L versions".to_string(), t.dim2),
        ];
        let right = super::diffview::position(app, d)
            .map(|p| (p, t.text, None))
            .into_iter()
            .collect();
        finish(app, g, c, mid, right);
        return;
    }
    let c = if app.visual.is_some() {
        g.put(0, r, " VIS ", t.badge_blue)
    } else if !app.marks.is_empty() {
        g.put(0, r, " SEL ", t.badge_blue)
    } else {
        g.put(0, r, " NOR ", t.badge)
    } + 1;

    let mut mid: Vec<(String, Style)> = Vec::new();
    let mut right: Vec<(String, Style, Option<Action>)> = Vec::new();
    if let View::Versions(v) = &app.view {
        mid.push((fmt::path(&v.path, app.home.as_deref()), t.dim));
        if let Some(tr) = app.tracks.get(&v.path).filter(|t| t.loaded()) {
            right.push((format!("{}/{}", v.sel + 1, tr.runs.len()), t.text, None));
        }
        finish(app, g, c, mid, right);
        return;
    }
    let set = app.set();
    let i = app.idx();
    if let Some(s) = set.get(i) {
        mid.push((fmt::time(s.time, &app.tz), t.bold));
        mid.push(("  ".into(), t.text));
        mid.push((s.id.0.short(), t.accent));
        mid.push(("  ".into(), t.text));
    }
    if let Some(state) = app.state().filter(|s| s.loaded()) {
        let name = app
            .folder
            .file_name()
            .map(|n| format!("{}/ ", n.to_string_lossy()))
            .unwrap_or_else(|| "/ ".into());
        if state.versions().contains(&i) {
            mid.push((name, t.dim));
            if let Some(counts) = state.counts.get(&i) {
                let parts = [
                    (counts.added, "+", t.added),
                    (counts.changed, "~", t.changed),
                    (counts.deleted, "−", t.deleted),
                ];
                let mut first = true;
                for (n, sign, style) in parts {
                    if n > 0 {
                        mid.push((format!("{}{sign}{n}", if first { "" } else { " " }), style));
                        first = false;
                    }
                }
            }
        } else if let Some(v) = state.version_at(i) {
            mid.push((
                format!("unchanged since {}", fmt::day(state.set[v].time, &app.tz)),
                t.dim,
            ));
        }
    }
    if let Some(axis) = Axis::new(&set, i, app.zoom, cols) {
        let share = axis.share(i);
        if share > 1 {
            mid.push((format!("  {share} in column · zi"), t.dim));
        }
    }
    if !app.name_filter.is_empty() {
        mid.push((format!("  filter \"{}\"", app.name_filter), t.accent));
    }
    if app.strict {
        mid.push(("  strict".to_string(), t.dim));
    }
    if let Some(y) = &app.yanked {
        mid.push((format!("  {} yanked", y.len()), t.accent));
    }

    if let Some(e) = app.selected()
        && !e.is_dir()
        && !e.is_deleted()
        && let Some(m) = e.node.mtime
    {
        right.push((format!("{}  ", fmt::time(m, &app.tz)), t.dim, None));
    }
    let rows = app.rows().len();
    let below_root = app.folder != app.root;
    let (pos, total) = if below_root {
        (app.sel, rows.saturating_sub(1))
    } else {
        (app.sel + 1, rows)
    };
    if matches!(app.listing(), Some(Some(_))) {
        right.push((format!("{pos}/{total}"), t.text, None));
    }
    finish(app, g, c, mid, right);
}

/// The middle parts from column `c`, as many as fit, then the right parts,
/// the pending count or prefix, and `? help`.
fn finish(
    app: &App,
    g: &mut Grid,
    mut c: u16,
    mid: Vec<(String, Style)>,
    mut right: Vec<(String, Style, Option<Action>)>,
) {
    let t = g.theme.clone();
    let (cols, r) = (g.cols(), g.rows() - 1);
    right.push(("  ? help".into(), t.dim2, Some(Action::Help)));
    let rlen: u16 = right.iter().map(|(s, _, _)| fmt::width(s) as u16).sum();
    let pending = format!(
        "{}{}",
        app.count,
        app.prefix.map(String::from).unwrap_or_default()
    );
    let plen = fmt::width(&pending) as u16;

    let limit = cols.saturating_sub(rlen + plen + 3);
    for (s, style) in mid {
        if c + fmt::width(&s) as u16 > limit {
            break;
        }
        c = g.put(c, r, &s, style);
    }
    let mut rc = cols.saturating_sub(1 + rlen);
    if !pending.is_empty() {
        g.put(
            rc.saturating_sub(plen + 2),
            r,
            &pending,
            t.accent.patch(t.bold),
        );
    }
    for (s, style, a) in right {
        rc = match a {
            Some(a) => g.put_act(rc, r, &s, style, a),
            None => g.put(rc, r, &s, style),
        };
    }
}
