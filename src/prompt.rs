//! Asks for a repository's password on the terminal, when nothing else
//! gives it.
//!
//! Everything goes through `/dev/tty`, so stdout stays clean for `--json`.
//! Keys are read in raw mode: nothing is echoed, and Ctrl-C arrives as a key
//! instead of a signal, so the terminal is always put back.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal;
use zeroize::Zeroizing;

fn tty() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("/dev/tty")
}

/// Whether there's a terminal to ask on: not under cron or CI.
pub fn available() -> bool {
    tty().is_ok()
}

/// Writes `text` to the terminal as it is.
pub fn show(text: &str) {
    if let Ok(mut t) = tty() {
        let _ = t.write_all(text.as_bytes());
        let _ = t.flush();
    }
}

/// Clears the terminal's current line.
pub fn clear_line() {
    show("\r\x1b[K");
}

/// What a key does to the password being typed.
#[derive(Debug, PartialEq, Eq)]
pub enum Feed {
    More,
    Done,
    Cancel,
}

/// Applies `k` to the password typed so far.
pub fn feed(buf: &mut String, k: KeyEvent) -> Feed {
    if k.kind == KeyEventKind::Release {
        return Feed::More;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Enter => Feed::Done,
        // A pasted line ends in LF, which raw mode reads as Ctrl-J.
        KeyCode::Char('j') if ctrl => Feed::Done,
        KeyCode::Esc => Feed::Cancel,
        KeyCode::Char('c') if ctrl => Feed::Cancel,
        KeyCode::Char('d') if ctrl && buf.is_empty() => Feed::Cancel,
        KeyCode::Char('u') if ctrl => {
            buf.clear();
            Feed::More
        }
        KeyCode::Backspace => {
            buf.pop();
            Feed::More
        }
        KeyCode::Char(c) if !ctrl && !c.is_control() => {
            buf.push(c);
            Feed::More
        }
        _ => Feed::More,
    }
}

/// Raw mode until dropped.
struct Raw;

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Asks for a password after `label`, without echo. None when cancelled.
pub fn ask(label: &str) -> io::Result<Option<Zeroizing<String>>> {
    let mut tty = tty()?;
    write!(tty, "\r\x1b[K{label} (input hidden): ")?;
    tty.flush()?;
    terminal::enable_raw_mode()?;
    let raw = Raw;
    // Keys typed before the prompt showed would count toward the password,
    // and a stray Enter would use up a try.
    unsafe { libc::tcflush(tty.as_raw_fd(), libc::TCIFLUSH) };
    while event::poll(Duration::ZERO)? {
        event::read()?;
    }
    // ponytail: room up front, as a String that grows leaves copies behind
    // that can't be zeroed; a longer password still works, less tidily.
    let mut buf = Zeroizing::new(String::with_capacity(1024));
    let done = loop {
        if let Event::Key(k) = event::read()? {
            match feed(&mut buf, k) {
                Feed::More => {}
                Feed::Done => break true,
                Feed::Cancel => break false,
            }
        }
    };
    drop(raw);
    writeln!(tty)?;
    Ok(done.then_some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(keys: &[KeyEvent]) -> (String, Feed) {
        let mut buf = String::new();
        for k in keys {
            match feed(&mut buf, *k) {
                Feed::More => {}
                end => return (buf, end),
            }
        }
        (buf, Feed::More)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn chars(s: &str) -> Vec<KeyEvent> {
        s.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    #[test]
    fn keys_build_the_password() {
        let enter = key(KeyCode::Enter);
        let bs = key(KeyCode::Backspace);
        let mut k = chars("ab");
        k.extend([bs, key(KeyCode::Char('c')), key(KeyCode::Char(' ')), enter]);
        assert_eq!(typed(&k), ("ac ".into(), Feed::Done));
        // Backspace on nothing, Ctrl-U clears, UTF-8 and capitals as typed.
        let mut k = vec![bs];
        k.extend(chars("junk"));
        k.push(ctrl('u'));
        k.extend(chars("pässwörD"));
        k.push(enter);
        assert_eq!(typed(&k), ("pässwörD".into(), Feed::Done));
        // A pasted line's LF ends it too.
        let mut k = chars("pasted");
        k.push(ctrl('j'));
        assert_eq!(typed(&k), ("pasted".into(), Feed::Done));
        // Other control keys and arrows are ignored.
        let mut k = chars("a");
        k.extend([ctrl('x'), key(KeyCode::Left), key(KeyCode::Tab), enter]);
        assert_eq!(typed(&k), ("a".into(), Feed::Done));
    }

    #[test]
    fn keys_that_cancel() {
        assert_eq!(
            typed(&[key(KeyCode::Char('a')), key(KeyCode::Esc)]).1,
            Feed::Cancel
        );
        assert_eq!(typed(&[key(KeyCode::Char('a')), ctrl('c')]).1, Feed::Cancel);
        assert_eq!(typed(&[ctrl('d')]).1, Feed::Cancel);
        // Ctrl-D only on an empty line.
        let k = [key(KeyCode::Char('a')), ctrl('d'), key(KeyCode::Enter)];
        assert_eq!(typed(&k), ("a".into(), Feed::Done));
    }
}
