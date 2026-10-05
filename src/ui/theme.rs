//! Colours (PLAN.md §3.17): the 16 ANSI colours, so the terminal's theme
//! applies. With `NO_COLOR`, only bold, dim, italic, strikethrough and
//! reverse video.

use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Debug)]
pub struct Theme {
    pub text: Style,
    pub bold: Style,
    pub dim: Style,
    pub dim2: Style,
    pub accent: Style,
    pub added: Style,
    pub changed: Style,
    pub deleted: Style,
    pub live: Style,
    pub dir: Style,
    /// Source-code file icons.
    pub code: Style,
    pub strike: Modifier,
    /// Background of the selected row.
    pub selected: Style,
    /// The selected snapshot's cell on the timeline.
    pub marker: Style,
    pub status_bar: Style,
    pub badge: Style,
    /// The DIFF, SEL and VIS badges.
    pub badge_blue: Style,
    /// The RST badge and warnings.
    pub badge_red: Style,
    /// The FIND badge.
    pub badge_magenta: Style,
    pub warn: Style,
    pub popup: Style,
    pub popup_border: Style,
}

impl Theme {
    pub fn new(color: bool) -> Self {
        if !color {
            let plain = Style::default();
            return Self {
                text: plain,
                bold: plain.add_modifier(Modifier::BOLD),
                dim: plain.add_modifier(Modifier::DIM),
                dim2: plain.add_modifier(Modifier::DIM),
                accent: plain.add_modifier(Modifier::BOLD),
                added: plain,
                changed: plain,
                deleted: plain,
                live: plain,
                dir: plain.add_modifier(Modifier::BOLD),
                code: plain,
                strike: Modifier::CROSSED_OUT,
                selected: plain.add_modifier(Modifier::REVERSED),
                marker: plain.add_modifier(Modifier::REVERSED),
                status_bar: plain,
                badge: plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
                badge_blue: plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
                badge_red: plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
                badge_magenta: plain.add_modifier(Modifier::REVERSED | Modifier::BOLD),
                warn: plain.add_modifier(Modifier::BOLD),
                popup: plain,
                popup_border: plain,
            };
        }
        let fg = |c| Style::default().fg(c);
        Self {
            text: Style::default(),
            bold: Style::default().add_modifier(Modifier::BOLD),
            dim: fg(Color::DarkGray),
            dim2: fg(Color::DarkGray),
            accent: fg(Color::Yellow),
            added: fg(Color::Green),
            changed: fg(Color::Blue),
            deleted: fg(Color::Red),
            live: fg(Color::Magenta),
            dir: fg(Color::Blue).add_modifier(Modifier::BOLD),
            code: fg(Color::Cyan),
            strike: Modifier::CROSSED_OUT,
            selected: Style::default().bg(Color::DarkGray),
            marker: Style::default().bg(Color::Yellow).fg(Color::Black),
            status_bar: Style::default(),
            badge: Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
            badge_blue: Style::default()
                .bg(Color::Blue)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
            badge_magenta: Style::default()
                .bg(Color::Magenta)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
            badge_red: Style::default()
                .bg(Color::Red)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
            warn: fg(Color::Red).add_modifier(Modifier::BOLD),
            popup: Style::default(),
            popup_border: fg(Color::DarkGray),
        }
    }

    /// Colour unless `NO_COLOR` is set (https://no-color.org).
    pub fn from_env() -> Self {
        Self::new(std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()))
    }
}
