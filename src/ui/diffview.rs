//! The full-screen diff (PLAN.md §3.10): a unified diff with 3 lines of
//! context, both line numbers, and `┄┄ around line N ┄┄` between changes.

use super::{Grid, fmt};
use crate::app::{App, DiffMode, DiffView};
use crate::diff::{FileDiff, HunkLine, Op};

pub fn draw(app: &App, g: &mut Grid, d: &DiffView) {
    let t = g.theme.clone();
    let (cols, bottom) = (g.cols(), g.rows() - 2);
    let name = d
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut c = g.put(1, 0, "restoric", t.accent.patch(t.bold)) + 2;
    c = g.put(c, 0, &format!("{name}  "), t.bold);
    g.put(
        1,
        1,
        match d.mode {
            DiffMode::Disk => "what changed since this version",
            DiffMode::Previous => "what this version changed",
        },
        t.dim,
    );
    let Some((key, _, _, old_label, new_label)) = app.diff_sides(d) else {
        g.put(1, 3, "loading…", t.dim);
        return;
    };
    c = g.put(c, 0, &old_label, t.deleted);
    c = g.put(c, 0, "  →  ", t.dim);
    g.put(c, 0, &new_label, t.added);

    let Some(diff) = app.diffs.get(&key) else {
        g.put(1, 3, "loading…", t.dim);
        return;
    };
    let top = 2;
    let height = (bottom - top + 1) as usize;
    match diff.as_ref() {
        FileDiff::Missing => {
            g.put(1, top + 1, "The file is missing on disk.", t.deleted);
        }
        FileDiff::Binary { old, new } => {
            let msg = format!("binary file, {} → {}", fmt::size(*old), fmt::size(*new));
            g.put(1, top + 1, &msg, t.dim);
        }
        FileDiff::TooBig(size) => {
            let msg = format!(
                "This file is {}, more than the {} limit. Press ⏎ to diff it anyway.",
                fmt::size(*size),
                fmt::size(crate::diff::DIFF_LIMIT)
            );
            g.put_to(1, top + 1, &msg, t.dim, cols);
        }
        FileDiff::Text {
            old,
            new,
            lines,
            added,
            removed,
        } => {
            let stat = format!("+{added} −{removed}");
            let x = cols.saturating_sub(1 + fmt::width(&stat) as u16);
            let x = g.put(x, 0, &format!("+{added}"), t.added) + 1;
            g.put(x, 0, &format!("−{removed}"), t.deleted);
            if lines.is_empty() {
                let msg = "No differences. The two versions are identical.";
                g.put((cols / 2).saturating_sub(24), top + 5, msg, t.dim);
                return;
            }
            let off = d.scroll.min(lines.len().saturating_sub(height));
            // Line numbers take 4 columns, or more for long files.
            let nw = old.len().max(new.len()).to_string().len().max(4);
            let (bx, sx, tx) = (2 + nw as u16, 4 + 2 * nw as u16, 6 + 2 * nw as u16);
            let w = cols.saturating_sub(tx + 2) as usize;
            let num = |n: Option<usize>| n.map_or(" ".repeat(nw), |n| format!("{:>nw$}", n + 1));
            for (k, l) in lines.iter().skip(off).take(height).enumerate() {
                let y = top + k as u16;
                let (a, b, sign, text, style, sign_style) = match l {
                    HunkLine::Header(n) => {
                        g.put(1, y, &format!("┄┄ around line {n} ┄┄"), t.changed);
                        continue;
                    }
                    HunkLine::Op(Op::Same { a, b }) => {
                        (Some(*a), Some(*b), " ", &old[*a], t.text, t.dim)
                    }
                    HunkLine::Op(Op::Removed { a }) => (
                        Some(*a),
                        None,
                        "-",
                        &old[*a],
                        t.deleted,
                        t.deleted.patch(t.bold),
                    ),
                    HunkLine::Op(Op::Added { b }) => (
                        None,
                        Some(*b),
                        "+",
                        &new[*b],
                        t.added,
                        t.added.patch(t.bold),
                    ),
                };
                g.put(1, y, &num(a), t.dim2);
                g.put(bx, y, &num(b), t.dim2);
                g.put(sx, y, sign, sign_style);
                g.put(tx, y, &fmt::fit(&text.replace('\t', "    "), w), style);
            }
        }
    }
}

/// `scroll/lines` for the status bar.
pub fn position(app: &App, d: &DiffView) -> Option<String> {
    let (key, ..) = app.diff_sides(d)?;
    match app.diffs.get(&key)?.as_ref() {
        FileDiff::Text { lines, .. } => Some(format!(
            "{}/{}",
            (d.scroll + 1).min(lines.len()),
            lines.len()
        )),
        _ => None,
    }
}
