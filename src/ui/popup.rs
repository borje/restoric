//! Popups (PLAN.md §3.4, §3.12, §3.16): which-key, messages, help.

use super::{Grid, TOP, fmt};
use crate::app::{Action, App, View};

/// What can follow a prefix key.
fn which(view: &View, prefix: char) -> &'static [(&'static str, &'static str, Action)] {
    match (view, prefix) {
        (View::Versions(_), 'g') => &[("gg", "newest version", Action::Top)],
        (_, 'z') => &[("zh", "show / hide deleted", Action::ToggleDeleted)],
        (_, 'g') => &[
            ("gg", "top of list", Action::Top),
            ("gh", "backup root", Action::Root),
            ("G", "bottom (no prefix)", Action::Bottom),
        ],
        _ => &[],
    }
}

pub fn which_key(app: &App, g: &mut Grid) {
    let Some(p) = app.prefix else { return };
    let items = which(&app.view, p);
    if items.is_empty() {
        return;
    }
    let t = g.theme.clone();
    let (w, h) = (34, items.len() as u16 + 2);
    let (x, y) = (
        g.cols().saturating_sub(w + 1),
        g.rows().saturating_sub(2 + h),
    );
    g.rbox(x, y, w, h, &p.to_string(), t.accent.patch(t.bold));
    for (n, (k, label, a)) in items.iter().enumerate() {
        let r = y + 1 + n as u16;
        g.put(x + 2, r, &format!("{k:<4}"), t.accent.patch(t.bold));
        g.put(x + 7, r, label, t.text);
        g.hit(x + 1, x + w - 1, r, a.clone());
    }
}

/// A message at the top right of the panes, gone at the next key.
pub fn message(app: &App, g: &mut Grid) {
    let Some(text) = &app.message else { return };
    let t = g.theme.clone();
    let w = 50.min(g.cols().saturating_sub(4));
    let lines = fmt::wrap(text, w.saturating_sub(4) as usize);
    let h = lines.len() as u16 + 2;
    let (x, y) = (g.cols().saturating_sub(w + 1), TOP);
    g.rbox(x, y, w, h, "", t.bold);
    for (k, l) in lines.iter().enumerate() {
        g.put(x + 2, y + 1 + k as u16, l, t.text);
    }
}

const HELP: &[(&str, &str)] = &[
    ("Folder view", ""),
    ("j k  gg G  C-d C-u", "move, top, bottom, half page"),
    ("h l  - ⏎", "parent / open (a file opens its versions)"),
    ("H L  ← →", "older / newer change in this folder"),
    ("[ ]   { }", "every snapshot / changes of the selected item"),
    ("⇥  J K", "preview: content or diff vs disk, scroll"),
    ("␣  v", "select / visual select"),
    ("y  p  P", "yank, restore next to it, overwrite"),
    ("r  d", "restore options / full-screen diff"),
    ("cc cd cf", "copy snapshot:path, folder, name"),
    ("/ n N   f", "search / next, previous / filter"),
    (".  zh   zi zo", "show deleted items / zoom timeline"),
    ("gh   3H 5j", "backup root / counts with motions"),
    ("", ""),
    ("Commands", ""),
    (":sep 1  :2026-09-01", "jump to a date (:yesterday :3d :2w)"),
    (":find NAME   s", "search every snapshot for a name"),
    (":latest :oldest :undo", ""),
    ("", ""),
    ("Diff", ""),
];

pub fn help(app: &App, g: &mut Grid) {
    if !app.help {
        return;
    }
    let t = g.theme.clone();
    let w = 74.min(g.cols().saturating_sub(4));
    let h = (HELP.len() as u16 + 4).min(g.rows());
    let x = (g.cols() - w) / 2;
    let y = (g.rows() - h) / 2;
    g.rbox(x, y, w, h, "Help", t.bold);
    let end = x + w - 1;
    for (n, (k, l)) in HELP.iter().enumerate() {
        let r = y + 2 + n as u16;
        if r >= y + h - 2 {
            break;
        }
        if k.is_empty() {
            continue;
        }
        if l.is_empty() && !k.starts_with(':') {
            g.put_to(x + 3, r, k, t.bold, end);
        } else {
            g.put_to(x + 3, r, k, t.accent.patch(t.bold), end);
            g.put_to(x + 26, r, l, t.text, end);
        }
    }
    let diff = y + 2 + HELP.len() as u16 - 1;
    if diff < y + h - 2 {
        g.put_to(
            x + 9,
            diff,
            "]c [c or n N changes · c vs disk · p vs previous",
            t.text,
            end,
        );
    }
    g.put_to(x + 3, y + h - 2, "Press any key to close", t.dim, end);
}
