//! `:find` results: one row per path, with when it was first
//! and last seen and whether it's on disk now.

use super::{Grid, fmt};
use crate::app::{Action, App, FindView};

pub fn draw(app: &App, g: &mut Grid, f: &FindView) {
    let t = g.theme.clone();
    let (cols, bottom) = (g.cols(), g.rows() - 2);
    let c = g.put(1, 0, "restoric", t.accent.patch(t.bold)) + 2;
    let c = g.put(c, 0, "Find  ", t.bold);
    g.put(c, 0, &format!("\"{}\"", f.query), t.accent);
    let n = f.results.len();
    let right = match f.progress {
        Some((done, total)) => format!("searching {done}/{total}"),
        None => format!(
            "{n} match{} in {} snapshots",
            if n == 1 { "" } else { "es" },
            f.set.len()
        ),
    };
    g.put(
        cols.saturating_sub(1 + fmt::width(&right) as u16),
        0,
        &right,
        t.dim,
    );

    let (px, fx, lx, sx) = (
        2,
        cols.saturating_sub(44),
        cols.saturating_sub(30),
        cols.saturating_sub(16),
    );
    g.put(px, 2, "PATH", t.dim2);
    g.put(fx, 2, "FIRST SEEN", t.dim2);
    g.put(lx, 2, "LAST SEEN", t.dim2);
    g.put(sx, 2, "NOW", t.dim2);
    if n == 0 && f.progress.is_none() {
        g.put(px, 5, "Nothing with that name in any snapshot.", t.dim);
        return;
    }
    let top = 3;
    let height = (bottom - top + 1) as usize;
    let off = f
        .sel
        .saturating_sub(height / 2)
        .min(n.saturating_sub(height));
    for (k, (found, on_disk)) in f.results.iter().enumerate().skip(off).take(height) {
        let y = top + (k - off) as u16;
        let here = k == f.sel;
        if here {
            g.fill(y, 0, cols - 1, t.selected);
            g.put(0, y, "▶", t.accent);
        }
        g.hit(0, cols, y, Action::ClickFound(k));
        let name = found
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            + if found.is_dir { "/" } else { "" };
        let prefix = found
            .path
            .parent()
            .map(|p| p.display().to_string())
            .filter(|p| !p.is_empty())
            .map(|p| p + "/")
            .unwrap_or_default();
        let room = (fx - px).saturating_sub(2) as usize;
        let pre = fmt::fit(&prefix, room.saturating_sub(fmt::width(&name)));
        let c = g.put(px, y, &pre, t.dim);
        let style = if found.is_dir {
            t.dir
        } else if *on_disk {
            t.bold
        } else {
            t.deleted
        };
        g.put_to(c, y, &name, style, fx - 1);
        g.put(fx, y, &fmt::time(f.set[found.first].time, &app.tz), t.text);
        g.put(lx, y, &fmt::time(f.set[found.last].time, &app.tz), t.text);
        if *on_disk {
            g.put(sx, y, "on disk", t.added);
        } else if let Some(next) = f.set.get(found.last + 1) {
            g.put(
                sx,
                y,
                &format!("gone {}", fmt::day(next.time, &app.tz)),
                t.deleted,
            );
        } else {
            g.put(sx, y, "missing", t.deleted);
        }
    }
}
