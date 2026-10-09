//! Keys and mouse to actions: counts, prefix keys, help.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use super::{Action, App, View};

/// Where things are on screen, from the last draw.
#[derive(Clone, Debug, Default)]
pub struct Hits {
    pub areas: Vec<(Rect, Action)>,
    pub listing: Option<Rect>,
    pub versions: Option<Rect>,
}

impl Hits {
    pub fn add(&mut self, area: Rect, a: Action) {
        self.areas.push((area, a));
    }

    fn at(&self, x: u16, y: u16) -> Option<Action> {
        // Later areas are drawn on top.
        self.areas
            .iter()
            .rev()
            .find(|(r, _)| r.contains(Position { x, y }))
            .map(|(_, a)| a.clone())
    }
}

/// Keys that mean something else in the diff and versions views.
fn view_action(view: &View, prefix: Option<char>, k: &KeyEvent) -> Option<Action> {
    use Action::*;
    match view {
        View::Diff(_) => match (prefix, k.code) {
            (Some(']'), KeyCode::Char('c')) => Some(NextHunk),
            (Some('['), KeyCode::Char('c')) => Some(PrevHunk),
            (None, KeyCode::Char(']')) => Some(Prefix(']')),
            (None, KeyCode::Char('[')) => Some(Prefix('[')),
            (None, KeyCode::Char('n')) => Some(NextHunk),
            (None, KeyCode::Char('N')) => Some(PrevHunk),
            (None, KeyCode::Char(' ')) => Some(HalfDown),
            (None, KeyCode::Char('c')) => Some(Diff),
            (None, KeyCode::Char('p')) => Some(DiffPrevious),
            _ => None,
        },
        View::Versions(_) => match (prefix, k.code) {
            (None, KeyCode::Char('p')) => Some(DiffPrevious),
            (Some('z'), KeyCode::Char('i')) => Some(ZoomIn),
            (Some('z'), KeyCode::Char('o')) => Some(ZoomOut),
            _ => None,
        },
        View::Folder => match (prefix, k.code) {
            (Some('z'), KeyCode::Char('i')) => Some(ZoomIn),
            (Some('z'), KeyCode::Char('o')) => Some(ZoomOut),
            (None, KeyCode::Char('/')) => Some(Search),
            (None, KeyCode::Char('f')) => Some(FilterInput),
            (None, KeyCode::Char('s')) => Some(Find),
            (None, KeyCode::Char('n')) => Some(NextMatch),
            (None, KeyCode::Char('N')) => Some(PrevMatch),
            _ => None,
        },
        View::Find(_) => None,
    }
}

/// The action for a key with prefix `prefix` (or none).
fn action(view: &View, prefix: Option<char>, k: &KeyEvent) -> Option<Action> {
    use Action::*;
    if let Some(a) = view_action(view, prefix, k) {
        return Some(a);
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    if let Some(p) = prefix {
        return match (p, k.code) {
            ('g', KeyCode::Char('g')) => Some(Top),
            ('g', KeyCode::Char('h')) => Some(Root),
            ('z', KeyCode::Char('d')) => Some(ToggleDeleted),
            ('z', KeyCode::Char('n')) => Some(HideNew),
            ('c', KeyCode::Char(c @ ('c' | 'd' | 'f'))) => Some(Copy(c)),
            _ => None,
        };
    }
    Some(match k.code {
        KeyCode::Char('d') if ctrl => HalfDown,
        KeyCode::Char('u') if ctrl => HalfUp,
        KeyCode::Char('c') if ctrl => Quit,
        KeyCode::Char('j') | KeyCode::Down => Down(1),
        KeyCode::Char('k') | KeyCode::Up => Up(1),
        KeyCode::PageDown => HalfDown,
        KeyCode::PageUp => HalfUp,
        KeyCode::Char('G') => Bottom,
        KeyCode::Char('g') => Prefix('g'),
        KeyCode::Char('z') => Prefix('z'),
        KeyCode::Left if shift => OlderSnapshot,
        KeyCode::Right if shift => NewerSnapshot,
        KeyCode::Char('h') | KeyCode::Char('-') | KeyCode::Backspace | KeyCode::Left => Parent,
        KeyCode::Char('l') | KeyCode::Enter | KeyCode::Right => Open,
        KeyCode::Char('H') => OlderChange,
        KeyCode::Char('L') => NewerChange,
        KeyCode::Char('[') => OlderSnapshot,
        KeyCode::Char(']') => NewerSnapshot,
        KeyCode::Char('{') => OlderItemChange,
        KeyCode::Char('}') => NewerItemChange,
        KeyCode::Home => OldestChange,
        KeyCode::End => NewestChange,
        KeyCode::Tab => TogglePreview,
        KeyCode::Char('J') => Scroll(3),
        KeyCode::Char('K') => Scroll(-3),
        KeyCode::Char('.') => ToggleDeleted,
        KeyCode::Char('d') => Diff,
        KeyCode::Char(' ') => ToggleMark,
        KeyCode::Char('v') => Visual,
        KeyCode::Char('y') => Yank,
        KeyCode::Char('p') => Paste,
        KeyCode::Char('P') => PasteOver,
        KeyCode::Char('r') => RestoreDialog,
        KeyCode::Char('o') => Pager,
        KeyCode::Char('c') => Prefix('c'),
        KeyCode::Char(':') => CommandLine,
        KeyCode::Char('?') | KeyCode::Char('~') => Help,
        KeyCode::Char('q') => Leave,
        _ => return None,
    })
}

impl App {
    /// Typing on the `:` line.
    fn input_key(&mut self, k: KeyEvent) {
        let Some(input) = &mut self.input else { return };
        match k.code {
            KeyCode::Esc => self.input_cancel(),
            KeyCode::Enter => self.input_enter(),
            KeyCode::Backspace => {
                if input.text.pop().is_none() {
                    self.input_cancel();
                } else {
                    self.input_changed();
                }
            }
            KeyCode::Char(c) => {
                input.text.push(c);
                self.input_changed();
            }
            _ => {}
        }
    }

    pub fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        // Help and messages close on any key.
        if self.help {
            self.help = false;
            return;
        }
        self.message = None;
        if self.input.is_some() {
            self.input_key(k);
            return;
        }
        if self.confirm.is_some() {
            match k.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.act(Action::ConfirmYes);
                }
                KeyCode::Char('n') | KeyCode::Char('q') | KeyCode::Esc => {
                    self.act(Action::ConfirmNo);
                }
                _ => {}
            }
            return;
        }
        if let Some(d) = &mut self.dialog {
            match k.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    d.sel = (d.sel + 1) % d.options();
                    d.confirm = false;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    d.sel = (d.sel + d.options() - 1) % d.options();
                    d.confirm = false;
                }
                KeyCode::Char(c @ '1'..='4') if (c as usize - '1' as usize) < d.options() => {
                    d.sel = c as usize - '1' as usize;
                    d.confirm = false;
                }
                KeyCode::Enter => self.dialog_enter(),
                KeyCode::Esc | KeyCode::Char('q') => self.dialog = None,
                _ => {}
            }
            return;
        }
        if self.prefix.is_none()
            && let KeyCode::Char(c @ '0'..='9') = k.code
            && (c != '0' || !self.count.is_empty())
        {
            if self.count.len() < 4 {
                self.count.push(c);
            }
            return;
        }
        if k.code == KeyCode::Esc {
            let pending = self.prefix.take().is_some() || !self.count.is_empty();
            self.count.clear();
            if !pending {
                self.act(Action::Escape);
            }
            return;
        }
        let prefix = self.prefix.take();
        let n = self.count.parse().unwrap_or(1);
        self.count.clear();
        if prefix.is_none()
            && let Some(a) = self
                .keymap
                .get(&(k.code, k.modifiers & KeyModifiers::CONTROL))
                .cloned()
        {
            self.act_n(a, n);
            return;
        }
        if let Some(a) = action(&self.view.clone(), prefix, &k) {
            self.act_n(a, n);
        }
    }

    pub fn mouse(&mut self, m: MouseEvent, hits: &Hits) {
        let inside = |r: Option<Rect>| {
            r.is_some_and(|r| {
                r.contains(Position {
                    x: m.column,
                    y: m.row,
                })
            })
        };
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.help {
                    self.help = false;
                    return;
                }
                self.message = None;
                if let Some(a) = hits.at(m.column, m.row) {
                    self.prefix = None;
                    self.act(a);
                }
            }
            MouseEventKind::ScrollDown if inside(hits.listing) => self.act_n(Action::Down(3), 1),
            MouseEventKind::ScrollUp if inside(hits.listing) => self.act_n(Action::Up(3), 1),
            MouseEventKind::ScrollDown if inside(hits.versions) => {
                self.act(Action::OlderChange);
            }
            MouseEventKind::ScrollUp if inside(hits.versions) => {
                self.act(Action::NewerChange);
            }
            _ => {}
        }
    }
}
