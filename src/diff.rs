//! Line diffs (imara-diff, histogram) and what the preview and diff views
//! build from them: margin marks, hunks with context, stats.

use std::collections::BTreeSet;

use imara_diff::{Algorithm, Diff, InternedInput};

/// Bytes as text, or `None` for binary (a NUL in the first 8 KiB).
pub fn text(bytes: &[u8]) -> Option<String> {
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Lines without their line endings.
pub fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Line `a` of the old text equals line `b` of the new (0-based).
    Same {
        a: usize,
        b: usize,
    },
    Removed {
        a: usize,
    },
    Added {
        b: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineDiff {
    pub ops: Vec<Op>,
    pub added: usize,
    pub removed: usize,
}

impl LineDiff {
    pub fn identical(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

pub fn diff(old: &str, new: &str) -> LineDiff {
    let input = InternedInput::new(old, new);
    let mut d = Diff::compute(Algorithm::Histogram, &input);
    d.postprocess_lines(&input);
    let (na, nb) = (input.before.len(), input.after.len());
    let mut ops = Vec::with_capacity(na.max(nb));
    let (mut a, mut b) = (0usize, 0usize);
    for h in d.hunks() {
        let (ha, hb) = (h.before.start as usize, h.after.start as usize);
        while a < ha && b < hb {
            ops.push(Op::Same { a, b });
            a += 1;
            b += 1;
        }
        for a in h.before.clone() {
            ops.push(Op::Removed { a: a as usize });
        }
        for b in h.after.clone() {
            ops.push(Op::Added { b: b as usize });
        }
        a = h.before.end as usize;
        b = h.after.end as usize;
    }
    while a < na && b < nb {
        ops.push(Op::Same { a, b });
        a += 1;
        b += 1;
    }
    LineDiff {
        added: d.count_additions() as usize,
        removed: d.count_removals() as usize,
        ops,
    }
}

/// Margin marks for the new text: lines that are new, and lines just after
/// a removal (0-based line numbers of the new text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub added: BTreeSet<usize>,
    pub removed: BTreeSet<usize>,
}

impl Marks {
    pub fn first(&self) -> Option<usize> {
        self.added.iter().chain(&self.removed).min().copied()
    }
}

pub fn marks(d: &LineDiff) -> Marks {
    let mut m = Marks::default();
    let mut next_b = 0;
    for op in &d.ops {
        match *op {
            Op::Same { b, .. } => next_b = b + 1,
            Op::Added { b } => {
                m.added.insert(b);
                next_b = b + 1;
            }
            Op::Removed { .. } => {
                m.removed.insert(next_b);
            }
        }
    }
    m
}

/// A line of a diff shown with context, or a `┄ line N` header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HunkLine {
    /// Starts a hunk at this (1-based) line number.
    Header(usize),
    Op(Op),
}

/// The changes with `context` lines around them.
pub fn hunks(d: &LineDiff, context: usize) -> Vec<HunkLine> {
    let n = d.ops.len();
    let mut keep = vec![false; n];
    for (k, op) in d.ops.iter().enumerate() {
        if !matches!(op, Op::Same { .. }) {
            for flag in keep
                .iter_mut()
                .take((k + context + 1).min(n))
                .skip(k.saturating_sub(context))
            {
                *flag = true;
            }
        }
    }
    let mut out = Vec::new();
    let mut prev: Option<usize> = None;
    for k in (0..n).filter(|&k| keep[k]) {
        if prev != k.checked_sub(1) || prev.is_none() {
            let line = match d.ops[k] {
                Op::Same { b, .. } | Op::Added { b } => b + 1,
                Op::Removed { a } => a + 1,
            };
            out.push(HunkLine::Header(line));
        }
        out.push(HunkLine::Op(d.ops[k]));
        prev = Some(k);
    }
    out
}

/// Most of a file the full-screen diff reads unless asked (PLAN.md §3.10).
pub const DIFF_LIMIT: u64 = 2 * 1024 * 1024;

/// What "diff it anyway" reads at most.
pub const FORCED_LIMIT: u64 = 256 * 1024 * 1024;

/// One side of a diff.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SideKey {
    /// A file in the repository, by content.
    Content([u8; 32]),
    Disk(std::path::PathBuf),
    /// No file: everything is added (or removed).
    Nothing,
}

/// A diff to show, by its two sides and how much of them to read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffKey {
    pub old: SideKey,
    pub new: SideKey,
    pub limit: u64,
}

/// The full-screen diff's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDiff {
    Text {
        old: Vec<String>,
        new: Vec<String>,
        lines: Vec<HunkLine>,
        added: usize,
        removed: usize,
    },
    /// At least one side is binary: sizes of both.
    Binary { old: u64, new: u64 },
    /// A side is bigger than the limit (its size).
    TooBig(u64),
    /// The file on disk is missing or unreadable, or isn't a file.
    Missing,
}

impl FileDiff {
    pub fn text(old: &str, new: &str) -> Self {
        let d = diff(old, new);
        FileDiff::Text {
            lines: hunks(&d, 3),
            added: d.added,
            removed: d.removed,
            old: lines(old).into_iter().map(str::to_string).collect(),
            new: lines(new).into_iter().map(str::to_string).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_marks_and_hunks() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nd\ne\n";
        let d = diff(old, new);
        assert_eq!((d.added, d.removed), (2, 1));
        assert_eq!(
            d.ops,
            [
                Op::Same { a: 0, b: 0 },
                Op::Removed { a: 1 },
                Op::Added { b: 1 },
                Op::Same { a: 2, b: 2 },
                Op::Same { a: 3, b: 3 },
                Op::Added { b: 4 },
            ]
        );
        let m = marks(&d);
        assert_eq!(m.added.iter().copied().collect::<Vec<_>>(), [1, 4]);
        assert_eq!(m.removed.iter().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(m.first(), Some(1));
        let h = hunks(&d, 1);
        assert_eq!(h[0], HunkLine::Header(1));
        assert_eq!(h.len(), 7);
    }

    #[test]
    fn binary() {
        assert_eq!(text(b"a\0b"), None);
        assert_eq!(text(b"ab").as_deref(), Some("ab"));
        assert!(diff("x\n", "x\n").identical());
    }
}
