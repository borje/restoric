//! Terminal setup and teardown, and the event loop (PLAN.md §4.5).

use std::io::stdout;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{Receiver, select};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;

use crate::app::App;
use crate::app::keys::Hits;
use crate::ui;
use crate::ui::theme::Theme;
use crate::worker::{Response, Worker};

fn restore() {
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
}

pub fn run(mut app: App, worker: Worker, responses: Receiver<Response>) -> Result<()> {
    // ratatui::init restores the terminal on panic; mouse capture too.
    let mut terminal = ratatui::init();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        hook(info);
    }));
    execute!(stdout(), EnableMouseCapture)?;

    let (ev_tx, events) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if ev_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let theme = Theme::from_env();
    let mut hits = Hits::default();
    let result = (|| -> Result<()> {
        loop {
            if app.bumped {
                worker.bump();
                app.bumped = false;
            }
            for req in app.outbox.drain(..) {
                worker.send(req);
            }
            terminal.draw(|f| {
                let area = f.area();
                hits = ui::draw(&mut app, f.buffer_mut(), area, &theme);
            })?;
            if app.quit {
                return Ok(());
            }
            select! {
                recv(events) -> ev => match ev? {
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
    })();
    restore();
    result
}
