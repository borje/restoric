//! A path's versions: runs of consecutive snapshots where it stayed the
//! same, and runs where it was missing.

use anyhow::Result;

use super::{Index, NodeRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// First and last snapshot of the run (indexes into the timeline set).
    pub from: usize,
    pub to: usize,
    /// False for a run where the path didn't exist.
    pub exists: bool,
}

impl Run {
    /// How many snapshots the run covers.
    pub fn snapshots(&self) -> usize {
        self.to - self.from + 1
    }

    pub fn contains(&self, i: usize) -> bool {
        (self.from..=self.to).contains(&i)
    }
}

impl Index {
    /// Runs from the path's `refs`, oldest first. Snapshots before the path
    /// first appears aren't in any run.
    pub fn runs(&self, refs: &[NodeRef]) -> Result<Vec<Run>> {
        let mut out: Vec<Run> = Vec::new();
        for (i, r) in refs.iter().enumerate() {
            match out.last_mut() {
                None if !r.exists() => continue,
                Some(last) if last.exists == r.exists() => {
                    let same = !r.exists() || !self.refs_differ(&refs[last.from], r)?;
                    if same {
                        last.to = i;
                        continue;
                    }
                }
                _ => {}
            }
            out.push(Run {
                from: i,
                to: i,
                exists: r.exists(),
            });
        }
        Ok(out)
    }
}

/// The run holding snapshot `i`.
pub fn run_at(runs: &[Run], i: usize) -> Option<(usize, Run)> {
    runs.iter()
        .copied()
        .enumerate()
        .find(|(_, r)| r.contains(i))
}
