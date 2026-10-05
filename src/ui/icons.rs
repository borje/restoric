//! File-type icons. The plain set from the mockup for now; Nerd Font
//! glyphs come in M7 (PLAN.md §3.17).

use ratatui::style::Style;

use super::theme::Theme;
use crate::repo::{Node, NodeKind};

pub fn icon(node: &Node, t: &Theme) -> (&'static str, Style) {
    if node.kind == NodeKind::Dir {
        return ("▸", t.changed);
    }
    if let NodeKind::Symlink { .. } = node.kind {
        return ("→", t.dim);
    }
    let name = node.name.to_string_lossy().to_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "go" | "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "java" | "rb" | "lua" | "kt"
        | "swift" | "zig" => ("◇", t.code),
        "md" | "markdown" | "rst" => ("¶", t.live),
        "sh" | "bash" | "zsh" | "fish" => ("$", t.added),
        "txt" | "log" => ("≡", t.dim),
        _ => ("○", t.accent),
    }
}
