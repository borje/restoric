//! The header: row 0.

use super::{Grid, fmt};
use crate::app::{Action, App, View};

/// Row 0: `restoric`, the breadcrumb, and the version count or indexing progress.
pub fn draw(app: &App, g: &mut Grid) {
    let t = g.theme.clone();
    let cols = g.cols();
    let mut c = g.put(1, 0, "restoric", t.accent.patch(t.bold)) + 2;
    let mut right = None;

    if let View::Versions(v) = &app.view {
        c = g.put(c, 0, "Versions  ", t.bold);
        c = g.put(c, 0, &fmt::path(&v.path, app.home.as_deref()), t.accent);
        if let Some(tr) = app.tracks.get(&v.path).filter(|t| t.loaded()) {
            let n = tr.runs.iter().filter(|r| r.exists).count();
            let s = if n == 1 { "" } else { "s" };
            right = Some(format!("{n} version{s} in {} snapshots", tr.set.len()));
        }
    } else {
        // The backup root is one part; folders below it are one part each.
        let at_root = app.folder == app.root;
        if let Some(h) = &app.shown_host {
            c = g.put(c, 0, &format!("{h}:"), t.bold);
        }
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
                right = Some(p);
            }
            _ => {}
        }
    }

    if let Some(r) = right {
        let x = (cols as usize)
            .saturating_sub(1 + fmt::width(&r))
            .max(c as usize + 1);
        g.put(x as u16, 0, &r, t.dim);
    }
}
