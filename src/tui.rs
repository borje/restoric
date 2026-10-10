//! Terminal setup and teardown, and the event loop.

use std::io::{Write, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{Receiver, select};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;

use crate::app::keys::Hits;
use crate::app::{App, Effect};
use crate::picker::{Pick, Picker, Step};
use crate::repo::SnapshotInfo;
use crate::ui;
use crate::ui::theme::Theme;
use crate::worker::{Response, Worker};

fn restore() {
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
}

/// Copies to the clipboard: the system one, or the terminal's through the
/// OSC 52 escape sequence over SSH or without a display.
fn copy(text: &str) {
    let ssh = std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    if !ssh
        && let Ok(mut c) = arboard::Clipboard::new()
        && c.set_text(text).is_ok()
    {
        return;
    }
    let b64 = base64(text.as_bytes());
    let mut out = stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Shows bytes in `$PAGER` (default `less`). The TUI must be suspended.
fn page(bytes: &[u8]) -> Result<()> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".into());
    let mut words = pager.split_whitespace();
    let result = match words.next() {
        Some(cmd) => std::process::Command::new(cmd)
            .args(words)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    // The pager may quit before reading everything.
                    let _ = stdin.write_all(bytes);
                }
                child.wait().map(|_| ())
            }),
        None => Ok(()),
    };
    result.map_err(|e| anyhow::anyhow!("running {pager}: {e}"))
}

/// What the picker's driver is asked to do between keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Io {
    /// Open config repository `i` and list its snapshots.
    Open(usize),
    /// List config repository `i`'s snapshots again.
    Refresh(usize),
}

/// The terminal, set up for the TUI and restored when dropped. The picker
/// and the folder view run in the same one.
pub struct Term {
    terminal: ratatui::DefaultTerminal,
    events: Receiver<Event>,
    /// The reader pauses while `suspended` hands the terminal over, so it
    /// doesn't take a pager's or a password prompt's keys.
    paused: Arc<AtomicBool>,
}

impl Term {
    pub fn new() -> Result<Self> {
        // ratatui::init restores the terminal on panic; mouse capture too.
        let terminal = ratatui::init();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(stdout(), DisableMouseCapture);
            hook(info);
        }));
        execute!(stdout(), EnableMouseCapture)?;

        let paused = Arc::new(AtomicBool::new(false));
        let (ev_tx, events) = crossbeam_channel::unbounded();
        let reader_paused = paused.clone();
        std::thread::spawn(move || {
            loop {
                if reader_paused.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => break,
                }
                let Ok(ev) = event::read() else { break };
                if ev_tx.send(ev).is_err() {
                    break;
                }
            }
        });
        Ok(Term {
            terminal,
            events,
            paused,
        })
    }

    /// Runs `f` with the terminal handed back: the TUI torn down and the
    /// reader paused, so a pager or a password prompt gets the keys.
    pub fn suspended<R>(&mut self, f: impl FnOnce() -> R) -> Result<R> {
        self.paused.store(true, Ordering::Release);
        // Let a poll in progress finish before `f` starts.
        std::thread::sleep(Duration::from_millis(60));
        restore();
        let r = f();
        self.terminal = ratatui::init();
        let back = execute!(stdout(), EnableMouseCapture).and_then(|_| self.terminal.clear());
        self.paused.store(false, Ordering::Release);
        back?;
        Ok(r)
    }

    /// Runs the picker until a pick or a quit. `io` opens repositories on
    /// request, and can suspend the terminal to ask for a password; its
    /// error is shown above the rows.
    pub fn pick(
        &mut self,
        picker: &mut Picker,
        theme: &Theme,
        io: &mut dyn FnMut(Io, &mut Term) -> Result<Vec<SnapshotInfo>>,
    ) -> Result<Option<Pick>> {
        loop {
            self.terminal.draw(|f| {
                let area = f.area();
                ui::picker::draw(picker, f.buffer_mut(), area, theme);
            })?;
            let Event::Key(k) = self.events.recv()? else {
                continue;
            };
            let (i, what) = match picker.key(k) {
                Step::Stay => continue,
                Step::Quit => return Ok(None),
                Step::Picked(p) => return Ok(Some(p)),
                Step::Open(i) => (i, Io::Open(i)),
                Step::Refresh(i) => (i, Io::Refresh(i)),
            };
            let location = picker
                .repos
                .iter()
                .find(|r| r.index == i)
                .map(|r| r.location.clone())
                .unwrap_or_default();
            picker.busy = Some(format!("opening {location}…"));
            self.terminal.draw(|f| {
                let area = f.area();
                ui::picker::draw(picker, f.buffer_mut(), area, theme);
            })?;
            let result = io(what, self);
            picker.busy = None;
            match (result, what) {
                (Ok(snaps), Io::Open(_)) => picker.opened(i, &snaps),
                (Ok(snaps), Io::Refresh(_)) => picker.refreshed(i, &snaps),
                (Err(e), _) => picker.failed(i, &format!("{e:#}")),
            }
        }
    }

    /// The folder view's event loop. True when `q` asks to go back to the
    /// picker.
    pub fn run(
        &mut self,
        mut app: App,
        worker: Worker,
        responses: Receiver<Response>,
        theme: Theme,
    ) -> Result<bool> {
        let mut hits = Hits::default();
        // New snapshots (a backup finished) are looked for every 5 minutes.
        let mut last_reload = std::time::Instant::now();
        loop {
            if app.bumped {
                worker.bump();
                app.bumped = false;
            }
            for req in app.outbox.drain(..) {
                worker.send(req);
            }
            for effect in std::mem::take(&mut app.effects) {
                match effect {
                    Effect::Clipboard(text) => copy(&text),
                    Effect::Pager { bytes, .. } => {
                        if let Err(e) = self.suspended(|| page(&bytes)).and_then(|r| r) {
                            app.message = Some(format!("{e:#}"));
                        }
                    }
                }
            }
            if last_reload.elapsed() > Duration::from_secs(300) {
                last_reload = std::time::Instant::now();
                worker.send(crate::worker::Request::Reload { quiet: true });
            }
            self.terminal.draw(|f| {
                let area = f.area();
                hits = ui::draw(&mut app, f.buffer_mut(), area, &theme);
            })?;
            if app.quit {
                return Ok(app.to_picker);
            }
            select! {
                recv(self.events) -> ev => match ev? {
                    Event::Key(k) => app.key(k),
                    Event::Mouse(m) => app.mouse(m, &hits),
                    _ => {}
                },
                recv(responses) -> r => {
                    app.apply(r?);
                    // Take everything that's ready before drawing again.
                    while let Ok(r) = responses.try_recv() {
                        app.apply(r);
                    }
                },
                default(Duration::from_millis(500)) => {}
            }
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        restore();
    }
}
