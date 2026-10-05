//! The status bar (PLAN.md §3.1, last row).

use ratatui::style::Style;

use super::timeline::Axis;
use super::{Grid, fmt};
use crate::app::{Action, App};

pub fn draw(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let (cols, r) = (g.cols(), g.rows() - 1);
    g.fill(r, 0, cols - 1, t.status_bar);
    let mut c = g.put(0, r, " NOR ", t.badge) + 1;

    let set = app.set();
    let i = app.idx();
    let mut mid: Vec<(String, Style)> = Vec::new();
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
    if let Some(axis) = Axis::new(app, cols) {
        let share = axis.share(app);
        if share > 1 {
            mid.push((format!("  {share} in column · zi"), t.dim));
        }
    }

    let mut right: Vec<(String, Style, Option<Action>)> = Vec::new();
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
