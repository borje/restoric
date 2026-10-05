//! Keys and mouse to actions (PLAN.md §3.14): counts, prefix keys, help.

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use super::{Action, App};

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

/// The action for a key with prefix `prefix` (or none).
fn action(prefix: Option<char>, k: &KeyEvent) -> Option<Action> {
    use Action::*;
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    if let Some(p) = prefix {
        return match (p, k.code) {
            ('g', KeyCode::Char('g')) => Some(Top),
            ('g', KeyCode::Char('h')) => Some(Root),
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
        KeyCode::Char('h') | KeyCode::Char('-') | KeyCode::Backspace => Parent,
        KeyCode::Char('l') | KeyCode::Char('i') | KeyCode::Enter => Open,
        KeyCode::Left if shift => OlderSnapshot,
        KeyCode::Right if shift => NewerSnapshot,
        KeyCode::Char('H') | KeyCode::Left => OlderChange,
        KeyCode::Char('L') | KeyCode::Right => NewerChange,
        KeyCode::Char('[') => OlderSnapshot,
        KeyCode::Char(']') => NewerSnapshot,
        KeyCode::Home => OldestChange,
        KeyCode::End => NewestChange,
        KeyCode::Char('?') | KeyCode::Char('~') => Help,
        KeyCode::Char('q') => Quit,
        _ => return None,
    })
}

impl App {
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
            self.prefix = None;
            self.count.clear();
            return;
        }
        let prefix = self.prefix.take();
        let n = self.count.parse().unwrap_or(1);
        self.count.clear();
        if let Some(a) = action(prefix, &k) {
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
