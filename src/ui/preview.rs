//! The preview column: the selected entry as it
//! was in the snapshot being viewed.

use std::path::Path;
use std::sync::Arc;

use ratatui::style::Style;

use super::folder::{counts_width, put_counts};
use super::{Grid, fmt, icons};
use crate::app::{Action, App, PreviewMode, Row, View};
use crate::diff::{self, HunkLine, Op};
use crate::index::Version;
use crate::index::fingerprint;
use crate::index::listing::Delta;
use crate::index::versions::run_at;
use crate::repo::{FileBytes, Node};

/// The heading, with the `⇥ content` / `⇥ vs disk` switch on the right.
pub fn head(
    app: &App,
    g: &mut Grid,
    (x0, x1, y): (u16, u16, u16),
    text: &str,
    style: Style,
    sub: Option<(&str, Style)>,
) {
    let t = g.theme.clone();
    let w = (x1 - x0 + 1) as usize;
    // Whether the version shown is the file on disk: a `−` row on the
    // disk version is the newest snapshot's file, shown as any version.
    let disk_version = match &app.view {
        View::Versions(v) => v.disk,
        _ => app.on_disk() && app.selected().is_none_or(|e| !e.is_gone()),
    };
    let mode = match app.preview {
        PreviewMode::Content => "content",
        PreviewMode::Disk if disk_version => "vs latest",
        PreviewMode::Disk => "vs disk",
    };
    let ml = mode.len() as u16;
    let room = w.saturating_sub(mode.len() + 6);
    // Drop trailing ` · parts` rather than cut the name.
    let mut text = text.to_string();
    while fmt::width(&text) > room
        && let Some(k) = text.rfind(" · ")
    {
        text.truncate(k);
    }
    g.put(x0, y, &fmt::fit(&text, room), style);
    g.put_act(x1 - ml - 3, y, "⇥", t.dim2, Action::TogglePreview);
    g.put_act(x1 - ml - 1, y, mode, t.accent, Action::TogglePreview);
    if let Some((s, style)) = sub {
        g.put(x0, y + 1, &fmt::fit(s, w), style);
    }
}

/// Where the version shown in the preview comes from.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// A version in the repository.
    Repo(&'a Node),
    /// The file on disk (the `on disk` version).
    Disk,
}

/// A file version's content from `top` to `y1`: with margin marks against
/// `prev` (`None` while it's loading), or diffed against the other end:
/// a repository version against the file on disk (`vs disk`), the file on
/// disk against `prev`, the newest snapshot's version (`vs latest`).
#[allow(clippy::too_many_arguments)]
pub fn content(
    app: &App,
    g: &mut Grid,
    (x0, x1, top, y1): (u16, u16, u16, u16),
    cur: Source,
    prev: Option<Option<&Node>>,
    disk_path: &Path,
) {
    let t = g.theme.clone();
    let w = (x1 - x0 + 1) as usize;
    let h = (y1 - top + 1) as usize;
    let bytes: Arc<FileBytes> = match cur {
        Source::Repo(node) => match app.files.get(&fingerprint::content(node)) {
            Some(b) => b.clone(),
            None => {
                g.put(x0, top, "loading…", t.dim);
                return;
            }
        },
        Source::Disk => match app.disk_files.get(disk_path) {
            None => {
                g.put(x0, top, "loading…", t.dim);
                return;
            }
            Some(None) => {
                g.put(x0, top, "(the file is missing on disk)", t.deleted);
                return;
            }
            Some(Some(d)) => d.clone(),
        },
    };
    let Some(text) = diff::text(&bytes.data) else {
        g.put(
            x0,
            top,
            &format!("(binary file, {})", fmt::size(bytes.size)),
            t.dim,
        );
        return;
    };
    let tabs = |s: &str| s.replace('\t', "  ");

    if app.preview == PreviewMode::Disk {
        let (old_text, new_text, same) = match cur {
            Source::Repo(_) => {
                let disk = match app.disk_files.get(disk_path) {
                    None => {
                        g.put(x0, top, "loading…", t.dim);
                        return;
                    }
                    Some(None) => {
                        g.put(x0, top, "(the file is missing on disk)", t.deleted);
                        return;
                    }
                    Some(Some(d)) => d,
                };
                let Some(disk_text) = diff::text(&disk.data) else {
                    g.put(x0, top, "(the file on disk is binary)", t.dim);
                    return;
                };
                (text, disk_text, "Identical to the file on disk.")
            }
            Source::Disk => {
                let p = match prev {
                    None => {
                        g.put(x0, top, "loading…", t.dim);
                        return;
                    }
                    Some(None) => {
                        g.put(x0, top, "Not in any backup.", t.live);
                        return;
                    }
                    Some(Some(p)) => p,
                };
                let Some(pb) = app.files.get(&fingerprint::content(p)) else {
                    g.put(x0, top, "loading…", t.dim);
                    return;
                };
                let Some(pt) = diff::text(&pb.data) else {
                    g.put(x0, top, "(the version in the snapshot is binary)", t.dim);
                    return;
                };
                (pt, text, "Identical to the latest snapshot.")
            }
        };
        let d = diff::diff(&old_text, &new_text);
        if d.identical() {
            g.put(x0, top, same, t.added);
            return;
        }
        let (old, new) = (diff::lines(&old_text), diff::lines(&new_text));
        let lines = diff::hunks(&d, 3);
        let off = app.scroll.unwrap_or(0).min(lines.len().saturating_sub(h));
        app.scroll_base.set(off);
        for (k, l) in lines.iter().skip(off).take(h).enumerate() {
            let y = top + k as u16;
            match l {
                HunkLine::Header(n) => {
                    g.put(x0, y, &format!("┄ line {n}"), t.changed);
                }
                HunkLine::Op(op) => {
                    let (sign, sign_style, s, style) = match *op {
                        Op::Same { a, .. } => (" ", t.dim2, old[a], t.text),
                        Op::Removed { a } => ("-", t.deleted.patch(t.bold), old[a], t.deleted),
                        Op::Added { b } => ("+", t.added.patch(t.bold), new[b], t.added),
                    };
                    g.put(x0, y, sign, sign_style);
                    g.put(x0 + 2, y, &fmt::fit(&tabs(s), w.saturating_sub(2)), style);
                }
            }
        }
        return;
    }

    let lines = diff::lines(&text);
    let marks = match prev {
        Some(Some(p)) => match app.files.get(&fingerprint::content(p)) {
            Some(pb) => diff::text(&pb.data).map(|pt| diff::marks(&diff::diff(&pt, &text))),
            None => None,
        },
        _ => None,
    }
    .unwrap_or_default();
    let auto = marks.first().map_or(0, |f| f.saturating_sub(4));
    let off = app
        .scroll
        .unwrap_or(auto)
        .min(lines.len().saturating_sub(h));
    app.scroll_base.set(off);
    for (k, s) in lines.iter().enumerate().skip(off).take(h) {
        let y = top + (k - off) as u16;
        g.put(x0, y, &format!("{:>3}", k + 1), t.dim2);
        let (m, ms, style) = if marks.added.contains(&k) {
            ("+", t.added.patch(t.bold), t.added)
        } else if marks.removed.contains(&k) {
            ("−", t.deleted.patch(t.bold), t.text)
        } else {
            (" ", t.text, t.text)
        };
        g.put(x0 + 3, y, m, ms);
        g.put(x0 + 5, y, &fmt::fit(&tabs(s), w.saturating_sub(5)), style);
    }
    let shown = lines.len().saturating_sub(off).min(h);
    if bytes.truncated() && shown < h {
        let note = format!(
            "… the first {} of {}",
            fmt::size(bytes.data.len() as u64),
            fmt::size(bytes.size)
        );
        g.put(x0, top + shown as u16, &note, t.dim);
    }
}

/// The folder view's preview of the selected row.
pub fn folder_view(app: &App, g: &mut Grid, x0: u16, x1: u16, y0: u16, y1: u16) {
    let t = g.theme.clone();
    let w = (x1 - x0 + 1) as usize;
    let Some(&row) = app.rows().get(app.sel) else {
        return;
    };
    if row == Row::Up {
        g.put(x0, y0, "Parent folder", t.dim);
        let parent = app.folder.parent().unwrap_or(&app.folder);
        let p = format!("{}/", fmt::path(parent, app.home.as_deref()));
        g.put(x0, y0 + 1, &fmt::fit(&p, w), t.dir);
        return;
    }
    let Some(e) = app.entry(row) else { return };
    let path = app.folder.join(&e.node.name);
    let name = e.node.name.to_string_lossy();
    let s = app.entry_snapshot(e);
    let set = app.set();
    let Some(snap) = set.get(s) else { return };

    if e.is_dir() {
        g.put(
            x0,
            y0,
            &fmt::fit(&format!("{name}/"), w.saturating_sub(12)),
            t.dir,
        );
        if e.is_gone() {
            g.put(x1 - 10, y0, "deleted", t.deleted);
            let last = format!("last seen {}", fmt::time(snap.time, &app.tz));
            g.put(x0, y0 + 1, &last, t.deleted);
        }
        let at = if app.on_disk() && !e.is_gone() {
            Version::Disk
        } else {
            Version::Snapshot(snap.id)
        };
        let Some(Some(entries)) = app.listings.get(&(at, path)) else {
            g.put(x0, y0 + 2, "loading…", t.dim);
            return;
        };
        for (k, c) in entries.iter().take((y1 - y0 - 1) as usize).enumerate() {
            let y = y0 + 2 + k as u16;
            let (icon, icon_style) = icons::icon(&c.node, &t, app.icons);
            g.put(x0, y, icon, icon_style);
            let mut n = c.node.name.to_string_lossy().into_owned();
            if c.is_dir() {
                n.push('/');
            }
            let style = if c.is_deleted() {
                t.deleted.add_modifier(t.strike)
            } else if c.is_dir() {
                t.dir
            } else {
                t.text
            };
            g.put(x0 + 2, y, &fmt::fit(&n, w.saturating_sub(10)), style);
            match &c.delta {
                Delta::Added => {
                    g.put(x1 - 1, y, "+", t.added);
                }
                Delta::Changed => {
                    g.put(x1 - 1, y, "~", t.changed);
                }
                Delta::Counts(cn) | Delta::New(cn) if !cn.is_empty() => {
                    let cw = counts_width(cn, "");
                    put_counts(g, x1 + 1 - cw, y, cn, "", x1 + 1);
                }
                _ => {}
            }
        }
        return;
    }

    let track = app.tracks.get(&path).filter(|t| t.loaded());
    let sp = track.and_then(|t| t.index_of(snap.id));
    let run = track.zip(sp).and_then(|(t, sp)| run_at(&t.runs, sp));
    if app.on_disk() && !e.is_gone() {
        // The file on disk, against the newest snapshot (`snap`), or for a
        // file that isn't in it, its latest version if there was one.
        let size = fmt::size(e.node.size);
        let (sub, style) = match (e.delta.clone(), app.latest_run(&path)) {
            (Delta::Added, Some(None)) => ("not in any backup".to_string(), t.live),
            (Delta::Added, Some(Some(_))) => ("not in the latest snapshot".to_string(), t.live),
            (Delta::Added, None) => (String::new(), t.dim),
            (Delta::Changed, _) => (
                format!("changed since {}", fmt::time(snap.time, &app.tz)),
                t.changed,
            ),
            _ => match run.zip(track) {
                Some(((_, r), tr)) => (
                    format!(
                        "unchanged since {}",
                        fmt::time(tr.set[r.from].time, &app.tz)
                    ),
                    t.dim,
                ),
                None => (String::new(), t.dim),
            },
        };
        head(
            app,
            g,
            (x0, x1, y0),
            &format!("{name} · on disk · {size}"),
            t.bold,
            Some((&sub, style)),
        );
        if e.node.kind != crate::repo::NodeKind::File {
            return;
        }
        let prev = app.latest_version(&path);
        content(app, g, (x0, x1, y0 + 3, y1), Source::Disk, prev, &path);
        return;
    }
    if e.is_gone() {
        let gone_at = match e.delta {
            Delta::Gone(last) => last + 1,
            _ => app.idx(),
        };
        let from = run.map(|(_, r)| r.from).and_then(|f| track?.set.get(f));
        let sub = match (from, set.get(gone_at)) {
            (Some(f), Some(gone)) => format!(
                "last version {}, gone {}",
                fmt::day(f.time, &app.tz),
                fmt::day(gone.time, &app.tz)
            ),
            // Deleted on disk: in the newest snapshot, not on disk.
            (Some(f), None) if app.on_disk() => {
                format!("last version {}, not on disk", fmt::day(f.time, &app.tz))
            }
            _ => String::new(),
        };
        head(
            app,
            g,
            (x0, x1, y0),
            &format!("{name} · deleted"),
            t.deleted.patch(t.bold),
            Some((&sub, t.deleted)),
        );
    } else {
        let size = fmt::size(e.node.size);
        match (track, run) {
            (Some(tr), Some((k, r))) => {
                let n = tr.runs.iter().filter(|r| r.exists).count();
                let v = tr.runs[..=k].iter().filter(|r| r.exists).count();
                let here = Some(r.from) == sp;
                let has_prev = tr.runs[..k].iter().any(|r| r.exists);
                let sub = if here && has_prev {
                    "changed here · + new line · − removed".to_string()
                } else if here {
                    "new in this snapshot".to_string()
                } else {
                    format!(
                        "unchanged since {}",
                        fmt::time(tr.set[r.from].time, &app.tz)
                    )
                };
                let sub_style = if here { t.changed } else { t.dim };
                head(
                    app,
                    g,
                    (x0, x1, y0),
                    &format!("{name} · v{v}/{n} · {size}"),
                    t.bold,
                    Some((&sub, sub_style)),
                );
            }
            _ => head(
                app,
                g,
                (x0, x1, y0),
                &format!("{name} · {size}"),
                t.bold,
                None,
            ),
        }
    }
    if e.node.kind != crate::repo::NodeKind::File {
        return;
    }
    let prev = sp.and_then(|sp| app.previous_version(&path, sp));
    content(
        app,
        g,
        (x0, x1, y0 + 3, y1),
        Source::Repo(&e.node),
        prev,
        &path,
    );
}
