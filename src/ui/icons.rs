//! File-type icons: Nerd Font glyphs, or the plain set
//! from the mockup with `--no-icons` / `icons = false`.

use ratatui::style::Style;

use super::theme::Theme;
use crate::repo::{Node, NodeKind};

pub fn icon(node: &Node, t: &Theme, nerd: bool) -> (&'static str, Style) {
    if nerd {
        return nerd_icon(node, t);
    }
    if node.kind == NodeKind::Dir {
        return ("▸", t.changed);
    }
    if let NodeKind::Symlink { .. } = node.kind {
        return ("→", t.dim);
    }
    match ext(node).as_str() {
        "go" | "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "java" | "rb" | "lua" | "kt"
        | "swift" | "zig" => ("◇", t.code),
        "md" | "markdown" | "rst" => ("¶", t.live),
        "sh" | "bash" | "zsh" | "fish" => ("$", t.added),
        "txt" | "log" => ("≡", t.dim),
        _ => ("○", t.accent),
    }
}

fn ext(node: &Node) -> String {
    let name = node.name.to_string_lossy().to_lowercase();
    name.rsplit_once('.')
        .map(|(_, e)| e.to_string())
        .unwrap_or_default()
}

fn nerd_icon(node: &Node, t: &Theme) -> (&'static str, Style) {
    match node.kind {
        NodeKind::Dir => return ("\u{f07b}", t.changed),
        NodeKind::Symlink { .. } => return ("\u{f0c1}", t.dim),
        _ => {}
    }
    match ext(node).as_str() {
        "rs" => ("\u{e7a8}", t.accent),
        "go" => ("\u{e627}", t.code),
        "py" => ("\u{e73c}", t.accent),
        "js" | "mjs" | "cjs" => ("\u{e74e}", t.accent),
        "ts" | "tsx" => ("\u{e628}", t.changed),
        "c" | "h" => ("\u{e61e}", t.changed),
        "cpp" | "cc" | "hpp" => ("\u{e61d}", t.changed),
        "java" => ("\u{e738}", t.deleted),
        "rb" => ("\u{e739}", t.deleted),
        "lua" => ("\u{e620}", t.changed),
        "html" | "htm" => ("\u{e736}", t.deleted),
        "css" | "scss" => ("\u{e749}", t.changed),
        "md" | "markdown" | "rst" => ("\u{f48a}", t.live),
        "sh" | "bash" | "zsh" | "fish" => ("\u{f489}", t.added),
        "json" => ("\u{e60b}", t.accent),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "mod" | "sum" | "lock" => {
            ("\u{e615}", t.dim)
        }
        "txt" | "log" => ("\u{f15c}", t.dim),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => ("\u{f1c5}", t.live),
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" => ("\u{f410}", t.deleted),
        "pdf" => ("\u{f1c1}", t.deleted),
        _ => ("\u{f15b}", t.text),
    }
}
