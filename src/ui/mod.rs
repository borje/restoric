//! Drawing (PLAN.md §3). Everything is drawn from [`App`] state; nothing
//! here reads the repository. Layout follows the mockup cell by cell, so
//! drawing works on a grid of cells rather than with ratatui widgets.

pub mod diffview;
pub mod findview;
pub mod fmt;
pub mod folder;
pub mod icons;
pub mod popup;
pub mod preview;
pub mod statusbar;
pub mod theme;
pub mod timeline;
pub mod versions;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

use crate::app::keys::Hits;
use crate::app::{Action, App, View};
use theme::Theme;

/// A buffer with the mockup's drawing helpers, recording clickable areas.
pub struct Grid<'a> {
    pub buf: &'a mut Buffer,
    pub area: Rect,
    pub theme: &'a Theme,
    pub hits: Hits,
}

impl Grid<'_> {
    pub fn cols(&self) -> u16 {
        self.area.width
    }

    pub fn rows(&self) -> u16 {
        self.area.height
    }

    /// Writes `text` at (x, y), stopping before column `end`. Returns the
    /// column after the text.
    pub fn put_to(&mut self, x: u16, y: u16, text: &str, style: Style, end: u16) -> u16 {
        let end = end.min(self.cols());
        let mut x = x;
        if y >= self.rows() {
            return x;
        }
        for c in text.chars() {
            let w = c.width().unwrap_or(0) as u16;
            if w == 0 {
                continue;
            }
            if x + w > end {
                break;
            }
            let pos = (self.area.x + x, self.area.y + y);
            if let Some(cell) = self.buf.cell_mut(pos) {
                cell.set_char(c);
                cell.set_style(style);
            }
            x += w;
        }
        x
    }

    pub fn put(&mut self, x: u16, y: u16, text: &str, style: Style) -> u16 {
        self.put_to(x, y, text, style, u16::MAX)
    }

    /// Like `put`, and clicking the text does `a`.
    pub fn put_act(&mut self, x: u16, y: u16, text: &str, style: Style, a: Action) -> u16 {
        let end = self.put(x, y, text, style);
        self.hit(x, end, y, a);
        end
    }

    /// Clicking columns x0..x1 of row y does `a`.
    pub fn hit(&mut self, x0: u16, x1: u16, y: u16, a: Action) {
        if x1 > x0 {
            self.hits
                .add(Rect::new(self.area.x + x0, self.area.y + y, x1 - x0, 1), a);
        }
    }

    /// Sets the style of columns x0..=x1 of row y, keeping the text.
    pub fn fill(&mut self, y: u16, x0: u16, x1: u16, style: Style) {
        for x in x0..=x1.min(self.cols().saturating_sub(1)) {
            if let Some(cell) = self.buf.cell_mut((self.area.x + x, self.area.y + y)) {
                cell.set_style(style);
            }
        }
    }

    pub fn vline(&mut self, x: u16, y0: u16, y1: u16) {
        for y in y0..=y1 {
            self.put(x, y, "│", self.theme.dim2);
        }
    }

    /// A rounded box, cleared inside, with a title in the top border.
    pub fn rbox(&mut self, x: u16, y: u16, w: u16, h: u16, title: &str, title_style: Style) {
        let t = self.theme.clone();
        for r in y..y + h {
            for c in x..x + w {
                if let Some(cell) = self.buf.cell_mut((self.area.x + c, self.area.y + r)) {
                    cell.reset();
                    cell.set_style(t.popup);
                }
            }
        }
        let inner = "─".repeat(w.saturating_sub(2) as usize);
        self.put(x, y, &format!("╭{inner}╮"), t.popup_border);
        self.put(x, y + h - 1, &format!("╰{inner}╯"), t.popup_border);
        for r in y + 1..y + h - 1 {
            self.put(x, r, "│", t.popup_border);
            self.put(x + w - 1, r, "│", t.popup_border);
        }
        if !title.is_empty() {
            self.put(x + 2, y, &format!(" {title} "), title_style);
        }
    }
}

/// Columns of the three panes (§3.13): Versions, listing, preview.
pub struct Panes {
    pub versions: Option<(u16, u16)>,
    pub listing: (u16, u16),
    pub preview: Option<(u16, u16)>,
}

pub fn panes(cols: u16) -> Panes {
    // The listing takes 40% of what's left after the Versions column,
    // within these bounds; the preview gets the rest.
    let listing = |room: u16, min: u16| (room * 40 / 100).clamp(min, 50);
    if cols >= 100 {
        let l1 = 23 + listing(cols - 23, 40) - 1;
        Panes {
            versions: Some((0, 21)),
            listing: (23, l1),
            preview: Some((l1 + 2, cols - 1)),
        }
    } else if cols >= 80 {
        let l1 = listing(cols, 38) - 1;
        Panes {
            versions: None,
            listing: (0, l1),
            preview: Some((l1 + 2, cols - 1)),
        }
    } else {
        Panes {
            versions: None,
            listing: (0, cols.saturating_sub(1)),
            preview: None,
        }
    }
}

/// First row of the panes.
pub const TOP: u16 = 5;

/// Draws the whole screen and returns where things can be clicked.
pub fn draw(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) -> Hits {
    let mut g = Grid {
        buf,
        area,
        theme,
        hits: Hits::default(),
    };
    if g.rows() < TOP + 3 || g.cols() < 40 {
        g.put(0, 0, "restoric: the terminal is too small", theme.text);
        return g.hits;
    }
    app.page = (g.rows() - TOP - 1) as usize;
    match app.view.clone() {
        View::Folder => {
            timeline::header(app, &mut g);
            timeline::draw_folder(app, &mut g);
            folder::draw(app, &mut g);
        }
        View::Versions(v) => {
            timeline::header(app, &mut g);
            versions::draw(app, &mut g, &v);
        }
        View::Diff(d) => diffview::draw(app, &mut g, &d),
        View::Find(f) => findview::draw(app, &mut g, &f),
    }
    statusbar::draw(app, &mut g);
    popup::which_key(app, &mut g);
    popup::restore_dialog(app, &mut g);
    popup::confirm(app, &mut g);
    popup::message(app, &mut g);
    popup::help(app, &mut g);
    g.hits
}

/// The screen as plain text, one line per row (for tests).
pub fn text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let s = buf[(x, y)].symbol();
            skip = fmt::width(s).saturating_sub(1);
            line.push_str(s);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}
