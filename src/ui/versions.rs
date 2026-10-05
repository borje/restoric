//! The versions view (PLAN.md §3.9): one row per distinct version of a
//! file, newest first, with deleted periods as their own rows.

use super::preview::{content, head};
use super::timeline::{self, Axis, TrackRow};
use super::{Grid, fmt};
use crate::app::{Action, App, VersionsView};
use crate::diff;
use crate::index::fingerprint;
use crate::repo::Node;

/// Lines added and removed going from `node` to the file on disk, cached.
fn vs_disk(app: &App, node: &Node, v: &VersionsView) -> Option<Option<(usize, usize)>> {
    let key = (fingerprint::content(node), v.path.clone());
    if let Some(s) = app.disk_stats.borrow().get(&key) {
        return Some(*s);
    }
    let ours = app.files.get(&key.0)?;
    let disk = app.disk_files.get(&v.path)?;
    let stat = match disk {
        None => diff::text(&ours.data).map(|t| (0, diff::lines(&t).len())),
        Some(d) => match (diff::text(&ours.data), diff::text(&d.data)) {
            (Some(a), Some(b)) => {
                let d = diff::diff(&a, &b);
                Some((d.added, d.removed))
            }
            _ => None,
        },
    };
    app.disk_stats.borrow_mut().insert(key, stat);
    Some(stat)
}

pub fn draw(app: &App, g: &mut Grid, v: &VersionsView) {
    let t = g.theme.clone();
    let (cols, bottom) = (g.cols(), g.rows() - 2);
    let name = v
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(track) = app.tracks.get(&v.path).filter(|t| t.loaded()) else {
        g.put(1, 5, "indexing…", t.dim);
        return;
    };
    let runs: Vec<_> = track.runs.iter().rev().copied().collect();
    let Some(&sel) = runs.get(v.sel) else { return };

    // Timeline: the file's changes.
    if let Some(axis) = Axis::new(&track.set, sel.from, app.zoom, cols) {
        let (change, exists) = timeline::track_row(&track.set, Some(track));
        let row = TrackRow {
            change,
            exists,
            on: "●",
            style: t.text,
            mark_only_changes: false,
            live: app.live.get(&v.path).map(|c| !c.is_empty()),
            label: name.clone(),
            label_style: t.bold,
            empty: None,
        };
        timeline::draw(
            g,
            &axis,
            app.zoom,
            sel.from,
            &[row],
            &app.tz,
            &Action::GoSnapshot,
        );
    }

    // The table.
    let top = 5;
    let (l1, preview) = if cols >= 100 {
        (47, true)
    } else if cols >= 80 {
        (43, true)
    } else {
        (cols - 1, false)
    };
    if preview {
        g.vline(l1 + 1, top, bottom);
    }
    g.put(1, top, "VERSION", t.dim2);
    g.put(17, top, "SIZE", t.dim2);
    g.put(23, top, "SNAPSHOTS", t.dim2);
    g.put(34, top, "VS DISK", t.dim2);
    g.put(1, top + 1, "on disk", t.live);
    match app.disk_files.get(&v.path) {
        Some(Some(d)) => {
            let s = fmt::size(d.size);
            g.put(21 - fmt::width(&s) as u16, top + 1, &s, t.dim);
        }
        Some(None) => {
            g.put(14, top + 1, "missing", t.deleted);
        }
        None => {}
    }
    let height = (bottom - (top + 2) + 1) as usize;
    let off = v
        .sel
        .saturating_sub(height / 2)
        .min(runs.len().saturating_sub(height));
    for (k, run) in runs.iter().enumerate().skip(off).take(height) {
        let y = top + 2 + (k - off) as u16;
        let here = k == v.sel;
        if here {
            g.fill(y, 0, l1, t.selected);
            g.put(0, y, "▶", t.accent);
        }
        g.hit(0, l1 + 1, y, Action::ClickVersion(k));
        let date = fmt::time(track.set[run.from].time, &app.tz);
        g.put(1, y, &date, if here { t.bold } else { t.text });
        let n = run.snapshots();
        let snaps = format!("{n} snap{}", if n > 1 { "s" } else { "" });
        g.put(23, y, &snaps, t.dim);
        if !run.exists {
            g.put(14, y, "deleted", t.deleted);
            continue;
        }
        let Some(Some(node)) = app.nodes.get(&(v.path.clone(), run.from)) else {
            continue;
        };
        let s = fmt::size(node.size);
        g.put(21 - fmt::width(&s) as u16, y, &s, t.text);
        match vs_disk(app, node, v) {
            Some(Some((0, 0))) => {
                g.put(34, y, "identical", t.added);
            }
            Some(Some((a, d))) => {
                let c = g.put_to(34, y, &format!("+{a}"), t.added, l1 + 1);
                g.put_to(c + 1, y, &format!("−{d}"), t.deleted, l1 + 1);
            }
            Some(None) => {
                g.put(34, y, "binary", t.dim);
            }
            None => {}
        }
    }

    // The preview: the selected version, its changes marked.
    if !preview {
        return;
    }
    let (x0, x1) = (l1 + 2, cols - 1);
    let snapshot = &track.set[sel.from];
    let n = sel.snapshots();
    if !sel.exists {
        let sub = format!("in {n} snapshot{}", if n > 1 { "s" } else { "" });
        head(
            app,
            g,
            (x0, x1, top),
            &format!("{name} · deleted"),
            t.deleted.patch(t.bold),
            Some((&sub, t.deleted)),
        );
        return;
    }
    let sub = format!(
        "kept in {n} snapshot{} · {}",
        if n > 1 { "s" } else { "" },
        snapshot.id.0.short()
    );
    head(
        app,
        g,
        (x0, x1, top),
        &format!("{name} · {}", fmt::time(snapshot.time, &app.tz)),
        t.bold,
        Some((&sub, t.dim)),
    );
    let Some(Some(node)) = app.nodes.get(&(v.path.clone(), sel.from)) else {
        g.put(x0, top + 3, "loading…", t.dim);
        return;
    };
    let older = runs[v.sel + 1..].iter().find(|r| r.exists);
    let prev = match older {
        None => Some(None),
        Some(r) => app.nodes.get(&(v.path.clone(), r.from)).map(Option::as_ref),
    };
    content(app, g, (x0, x1, top + 3, bottom), node, prev, &v.path);
}
