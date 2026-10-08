//! Popups (PLAN.md §3.4, §3.12, §3.16): which-key, messages, help.

use super::{Grid, TOP, fmt};
use crate::app::selection::Confirm;
use crate::app::{Action, App, View};

/// What can follow a prefix key.
fn which(view: &View, prefix: char) -> &'static [(&'static str, &'static str, Action)] {
    match (view, prefix) {
        (View::Versions(_), 'g') => &[("gg", "newest version", Action::Top)],
        (View::Diff(_), 'g') => &[("gg", "top", Action::Top)],
        (View::Diff(_), ']') => &[("]c", "next change", Action::NextHunk)],
        (View::Diff(_), '[') => &[("[c", "previous change", Action::PrevHunk)],
        (_, 'z') => &[
            ("zh", "show / hide deleted", Action::ToggleDeleted),
            ("zi", "zoom timeline in", Action::ZoomIn),
            ("zo", "zoom timeline out", Action::ZoomOut),
        ],
        (_, 'c') => &[
            ("cc", "copy snapshot:path", Action::Copy('c')),
            ("cd", "copy folder path", Action::Copy('d')),
            ("cf", "copy file name", Action::Copy('f')),
        ],
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
    ("h l  ← →  ⏎", "parent / open (a file opens its versions)"),
    ("H L", "older / newer change in this folder"),
    ("[ ]   { }", "every snapshot / changes of the selected item"),
    ("⇥  J K", "preview: content or diff vs disk, scroll"),
    ("␣  v", "select / visual select"),
    ("y  p  P", "yank, restore next to it, overwrite"),
    ("r  d  o", "restore options / full-screen diff / $PAGER"),
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

/// The restore dialog (PLAN.md §3.11).
pub fn restore_dialog(app: &App, g: &mut Grid) {
    let Some(d) = &app.dialog else { return };
    let t = g.theme.clone();
    let (cols, rows) = (g.cols(), g.rows());
    let w = 78.min(cols.saturating_sub(2));
    let h = if d.confirm { 15 } else { 14 };
    let x = (cols - w) / 2;
    let y = ((rows.saturating_sub(h)) / 2).saturating_sub(1);
    g.rbox(x, y, w, h, "Restore", t.bold);
    let tg = &d.target;
    let mut name = tg.name();
    if tg.node.is_dir() {
        name.push('/');
    }
    let c = g.put(x + 3, y + 2, &name, t.bold);
    let c = g.put(
        c,
        y + 2,
        &format!("  @ {}  ", fmt::time(tg.snapshot.time, &app.tz)),
        t.text,
    );
    g.put(c, y + 2, &tg.snapshot.id.0.short(), t.accent);
    let from = format!("from {}", tg.path.display());
    g.put(x + 3, y + 3, &fmt::fit(&from, (w - 6) as usize), t.dim);
    for (k, (label, detail)) in app.dialog_options(d).iter().enumerate() {
        let r = y + 5 + k as u16;
        let on = d.sel == k;
        g.put(x + 3, r, &format!("{} ", k + 1), t.dim);
        let radio = if on { "(•) " } else { "( ) " };
        g.put(
            x + 5,
            r,
            radio,
            if on { t.accent.patch(t.bold) } else { t.dim },
        );
        g.put_to(x + 9, r, label, if on { t.bold } else { t.text }, x + 39);
        g.put(x + 40, r, &fmt::fit(detail, (w - 43) as usize), t.dim);
        g.hit(x + 1, x + w - 1, r, Action::DialogOption(k));
    }
    if d.confirm {
        let msg = "This replaces what is on disk now. Press ⏎ again to confirm.";
        g.put_to(x + 3, y + 10, msg, t.warn, x + w - 1);
    }
    let by = y + h - 2;
    g.put_act(
        x + 3,
        by,
        "[ Restore ]",
        t.accent.patch(t.bold),
        Action::DialogRestore,
    );
    g.put_act(x + 17, by, "[ Cancel ]", t.dim, Action::DialogCancel);
    g.put(x + w - 29, by, "j k choose  ⏎ restore  esc", t.dim);
}

/// A y/n popup: before overwriting, and before stopping a restore.
pub fn confirm(app: &App, g: &mut Grid) {
    let Some(c) = &app.confirm else { return };
    let t = g.theme.clone();
    let stop = |quit| app.restoring.as_ref().map(|r| r.stop_text(quit));
    let (title, text, yes, no) = match c {
        Confirm::Overwrite { text, .. } => {
            ("Overwrite?", text.clone(), "[y] Overwrite", "[n] Cancel")
        }
        Confirm::StopRestore => (
            "Stop restoring?",
            stop(false).unwrap_or_default(),
            "[y] Stop",
            "[n] Keep going",
        ),
        Confirm::QuitRestore if app.to_picker => (
            "Back to the picker?",
            stop(true).unwrap_or_default(),
            "[y] Stop and go back",
            "[n] Keep going",
        ),
        Confirm::QuitRestore => (
            "Quit?",
            stop(true).unwrap_or_default(),
            "[y] Stop and quit",
            "[n] Keep going",
        ),
    };
    let lines = fmt::wrap(&text, 50);
    let w = 56.min(g.cols());
    let h = lines.len() as u16 + 5;
    let x = (g.cols() - w) / 2;
    let y = ((g.rows().saturating_sub(h)) / 2).saturating_sub(2);
    g.rbox(x, y, w, h, title, t.warn);
    for (k, l) in lines.iter().enumerate() {
        g.put(x + 3, y + 2 + k as u16, l, t.text);
    }
    let by = y + h - 2;
    let nx = g.put_act(x + 3, by, yes, t.warn, Action::ConfirmYes) + 3;
    g.put_act(nx, by, no, t.dim, Action::ConfirmNo);
}
