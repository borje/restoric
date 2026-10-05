//! The `:` command line (PLAN.md §3.15). Dates, `:find`, `:host`, `:tag`
//! and `:set` come in M6.

use super::{Action, App};
use crate::worker::Request;

/// What's being typed in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
}

impl App {
    /// Runs a command typed after `:`.
    pub(super) fn run_command(&mut self, text: &str) {
        let cmd = text.trim();
        let set = self.set();
        let exists = |app: &App, i: usize| app.state().is_none_or(|s| s.exists(i));
        match cmd {
            "" => {}
            "undo" => self.outbox.push(Request::Undo),
            "q" | "quit" => self.quit = true,
            "help" => self.help = true,
            "deleted" => {
                self.act(Action::ToggleDeleted);
            }
            "latest" | "now" => {
                if let Some(i) = (0..set.len()).rev().find(|&i| exists(self, i)) {
                    self.act(Action::GoSnapshot(i));
                }
            }
            "oldest" | "first" => {
                if let Some(i) = (0..set.len()).find(|&i| exists(self, i)) {
                    self.act(Action::GoSnapshot(i));
                }
            }
            _ => {
                self.message = Some(format!(
                    "Unknown command \":{cmd}\". Try :sep 1, :yesterday, :3d, :find NAME, :undo"
                ));
            }
        }
    }
}
