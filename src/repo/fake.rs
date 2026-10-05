//! An in-memory [`Repo`] built from a small text DSL, for tests, UI work and
//! the demo. Trees are content-addressed like restic's, so an unchanged
//! folder keeps its tree id from one snapshot to the next.
//!
//! ```text
//! # comments and blank lines are ignored
//! host bege-laptop                 # default host for the snapshots below
//! root /home/bege/dev/project      # the backed-up folder; paths below are relative to it
//!
//! snapshot 2026-07-24 12:16        # optional: host=NAME tags=a,b root=/other/path
//!   write src/main.go package main\nfunc main() {}\n
//!   append src/main.go // more\n
//!   touch src/util.go              # new mtime, same content
//!   chmod src/util.go 755
//!   chown src/util.go 1000:1000
//!   mkdir src/empty
//!   link src/current -> main.go
//!   rm src/legacy.go               # files or whole folders
//!   mv src/a.go src/b.go
//!   fill logs/big.log 3000000      # a text file of that many bytes
//!
//! disk                             # optional: how the files look on disk now
//!   append src/main.go // unsaved\n
//! ```
//!
//! Each snapshot starts from the previous one's files, and the disk from the
//! last snapshot's. Text after `write` and `append` takes `\n` and `\t`
//! escapes, and `\0` for a NUL byte.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use sha2::{Digest, Sha256};

use super::{BlobId, Id, Node, NodeKind, Repo, SnapshotId, SnapshotInfo, Tree, TreeId};

#[derive(Clone)]
enum Entry {
    File {
        data: Vec<u8>,
        mode: u32,
        uid: u32,
        gid: u32,
        mtime: Timestamp,
    },
    Dir(BTreeMap<OsString, Entry>),
    Link(OsString),
}

pub struct FakeRepo {
    snapshots: Vec<SnapshotInfo>,
    trees: HashMap<TreeId, Arc<Tree>>,
    blobs: HashMap<BlobId, Vec<u8>>,
    disk: FakeDisk,
}

/// The files on disk after the last snapshot (and the `disk` block),
/// nested under the root path like a snapshot's tree.
#[derive(Clone)]
pub struct FakeDisk {
    top: BTreeMap<OsString, Entry>,
}

fn hash(parts: &[&[u8]]) -> Id {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    Id(h.finalize().into())
}

fn unescape(s: &str) -> Vec<u8> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('0') => out.push('\0'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out.into_bytes()
}

fn parts(p: &str) -> Vec<OsString> {
    Path::new(p)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect()
}

fn parse_time(date: &str, time: &str) -> Result<Timestamp> {
    let dt: DateTime = format!("{date}T{time}")
        .parse()
        .with_context(|| format!("bad date {date} {time}"))?;
    Ok(dt.to_zoned(TimeZone::UTC)?.timestamp())
}

/// The folder at `path`, created if missing.
fn dir_mut<'a>(
    root: &'a mut BTreeMap<OsString, Entry>,
    path: &[OsString],
) -> Result<&'a mut BTreeMap<OsString, Entry>> {
    let mut d = root;
    for p in path {
        let e = d
            .entry(p.clone())
            .or_insert_with(|| Entry::Dir(BTreeMap::new()));
        d = match e {
            Entry::Dir(m) => m,
            _ => bail!("{} is not a folder", p.to_string_lossy()),
        };
    }
    Ok(d)
}

fn entry_mut<'a>(root: &'a mut BTreeMap<OsString, Entry>, path: &str) -> Result<&'a mut Entry> {
    let p = parts(path);
    let (name, parent) = p.split_last().context("empty path")?;
    dir_mut(root, parent)?
        .get_mut(name)
        .with_context(|| format!("{path} doesn't exist"))
}

impl FakeRepo {
    pub fn parse(text: &str) -> Result<Self> {
        let mut repo = FakeRepo {
            snapshots: Vec::new(),
            trees: HashMap::new(),
            blobs: HashMap::new(),
            disk: FakeDisk {
                top: BTreeMap::new(),
            },
        };
        // Set once the `disk` block starts: the time its changes are made.
        let mut disk_time: Option<Timestamp> = None;
        let mut host = "fake-host".to_string();
        let mut root = PathBuf::from("/data");
        let mut files: BTreeMap<OsString, Entry> = BTreeMap::new();
        // The snapshot being described: (time, host, tags, root).
        let mut current: Option<(Timestamp, String, Vec<String>, PathBuf)> = None;

        for (lineno, line) in text.lines().enumerate() {
            let err = || format!("line {}: {line}", lineno + 1);
            let line = match line.find(" #") {
                Some(i)
                    if !line.trim_start().starts_with("write")
                        && !line.trim_start().starts_with("append") =>
                {
                    &line[..i]
                }
                _ => line,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let (cmd, rest) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
            let mtime = disk_time
                .or(current.as_ref().map(|c| c.0))
                .unwrap_or_default();
            match cmd {
                "host" => host = rest.trim().to_string(),
                "root" => root = PathBuf::from(rest.trim()),
                "disk" => {
                    let last = current.as_ref().map(|c| c.0).unwrap_or_default();
                    if let Some(c) = current.take() {
                        repo.add_snapshot(&files, c)?;
                    }
                    disk_time = Some(last + jiff::SignedDuration::from_hours(1));
                }
                "snapshot" => {
                    if disk_time.is_some() {
                        bail!("{}: snapshots must come before the disk block", err());
                    }
                    if let Some(c) = current.take() {
                        repo.add_snapshot(&files, c)?;
                    }
                    let mut words = rest.split_whitespace();
                    let (date, time) = (
                        words.next().with_context(err)?,
                        words.next().with_context(err)?,
                    );
                    let mut snap = (
                        parse_time(date, time)?,
                        host.clone(),
                        Vec::new(),
                        root.clone(),
                    );
                    for w in words {
                        match w.split_once('=') {
                            Some(("host", h)) => snap.1 = h.to_string(),
                            Some(("tags", t)) => {
                                snap.2 = t.split(',').map(str::to_string).collect()
                            }
                            Some(("root", r)) => snap.3 = PathBuf::from(r),
                            _ => bail!("{}: unknown option {w}", err()),
                        }
                    }
                    current = Some(snap);
                }
                "write" | "append" => {
                    let (path, text) = rest.split_once(' ').unwrap_or((rest, ""));
                    let p = parts(path);
                    let (name, parent) = p.split_last().with_context(err)?;
                    let d = dir_mut(&mut files, parent).with_context(err)?;
                    let data = unescape(text);
                    match d.get_mut(name) {
                        Some(Entry::File {
                            data: old,
                            mtime: m,
                            ..
                        }) => {
                            if cmd == "write" {
                                *old = data;
                            } else {
                                old.extend(data);
                            }
                            *m = mtime;
                        }
                        _ => {
                            d.insert(
                                name.clone(),
                                Entry::File {
                                    data,
                                    mode: 0o644,
                                    uid: 1000,
                                    gid: 1000,
                                    mtime,
                                },
                            );
                        }
                    }
                }
                "fill" => {
                    let (path, size) = rest.split_once(' ').with_context(err)?;
                    let size: usize = size.trim().parse().with_context(err)?;
                    let mut data = Vec::with_capacity(size);
                    let mut n = 0;
                    while data.len() < size {
                        data.extend(format!("line {n}\n").bytes());
                        n += 1;
                    }
                    data.truncate(size);
                    let p = parts(path);
                    let (name, parent) = p.split_last().with_context(err)?;
                    dir_mut(&mut files, parent).with_context(err)?.insert(
                        name.clone(),
                        Entry::File {
                            data,
                            mode: 0o644,
                            uid: 1000,
                            gid: 1000,
                            mtime,
                        },
                    );
                }
                "touch" => match entry_mut(&mut files, rest.trim()).with_context(err)? {
                    Entry::File { mtime: m, .. } => *m = mtime,
                    _ => bail!("{}: not a file", err()),
                },
                "chmod" => {
                    let (path, mode) = rest.split_once(' ').with_context(err)?;
                    let mode = u32::from_str_radix(mode.trim(), 8).with_context(err)?;
                    match entry_mut(&mut files, path).with_context(err)? {
                        Entry::File { mode: m, .. } => *m = mode,
                        _ => bail!("{}: not a file", err()),
                    }
                }
                "chown" => {
                    let (path, owner) = rest.split_once(' ').with_context(err)?;
                    let (u, g) = owner.trim().split_once(':').with_context(err)?;
                    match entry_mut(&mut files, path).with_context(err)? {
                        Entry::File { uid, gid, .. } => {
                            *uid = u.parse().with_context(err)?;
                            *gid = g.parse().with_context(err)?;
                        }
                        _ => bail!("{}: not a file", err()),
                    }
                }
                "mkdir" => {
                    dir_mut(&mut files, &parts(rest.trim())).with_context(err)?;
                }
                "link" => {
                    let (path, target) = rest.split_once(" -> ").with_context(err)?;
                    let p = parts(path);
                    let (name, parent) = p.split_last().with_context(err)?;
                    dir_mut(&mut files, parent)
                        .with_context(err)?
                        .insert(name.clone(), Entry::Link(target.trim().into()));
                }
                "rm" => {
                    let p = parts(rest.trim());
                    let (name, parent) = p.split_last().with_context(err)?;
                    dir_mut(&mut files, parent)
                        .with_context(err)?
                        .remove(name)
                        .with_context(|| format!("{}: doesn't exist", err()))?;
                }
                "mv" => {
                    let (from, to) = rest.split_once(' ').with_context(err)?;
                    let p = parts(from);
                    let (name, parent) = p.split_last().with_context(err)?;
                    let e = dir_mut(&mut files, parent)
                        .with_context(err)?
                        .remove(name)
                        .with_context(|| format!("{}: doesn't exist", err()))?;
                    let p = parts(to.trim());
                    let (name, parent) = p.split_last().with_context(err)?;
                    dir_mut(&mut files, parent)
                        .with_context(err)?
                        .insert(name.clone(), e);
                }
                _ => bail!("{}: unknown command", err()),
            }
        }
        if let Some(c) = current.take() {
            repo.add_snapshot(&files, c)?;
        }
        repo.snapshots.sort_by_key(|s| s.time);
        let mut whole = Entry::Dir(files);
        for p in parts(root.to_str().context("root must be UTF-8")?)
            .into_iter()
            .rev()
        {
            whole = Entry::Dir(BTreeMap::from([(p, whole)]));
        }
        let Entry::Dir(top) = whole else {
            unreachable!()
        };
        repo.disk = FakeDisk { top };
        Ok(repo)
    }

    pub fn disk(&self) -> FakeDisk {
        self.disk.clone()
    }

    fn add_snapshot(
        &mut self,
        files: &BTreeMap<OsString, Entry>,
        (time, host, tags, root): (Timestamp, String, Vec<String>, PathBuf),
    ) -> Result<()> {
        // Nest the files under the root path, as restic does.
        let mut whole = Entry::Dir(files.clone());
        for p in parts(root.to_str().context("root must be UTF-8")?)
            .into_iter()
            .rev()
        {
            whole = Entry::Dir(BTreeMap::from([(p, whole)]));
        }
        let Entry::Dir(top) = whole else {
            unreachable!()
        };
        let tree = self.store_tree(&top);
        let id = SnapshotId(hash(&[
            &time.as_nanosecond().to_le_bytes(),
            host.as_bytes(),
            &tree.0.0,
        ]));
        self.snapshots.push(SnapshotInfo {
            id,
            time,
            host,
            paths: vec![root],
            tags,
            tree,
        });
        Ok(())
    }

    fn store_tree(&mut self, entries: &BTreeMap<OsString, Entry>) -> TreeId {
        let mut nodes = Vec::new();
        for (name, e) in entries {
            let node = match e {
                Entry::File {
                    data,
                    mode,
                    uid,
                    gid,
                    mtime,
                } => {
                    let blob = BlobId(hash(&[data]));
                    self.blobs.insert(blob, data.clone());
                    Node {
                        name: name.clone(),
                        kind: NodeKind::File,
                        size: data.len() as u64,
                        mode: Some(*mode),
                        uid: Some(*uid),
                        gid: Some(*gid),
                        mtime: Some(*mtime),
                        content: if data.is_empty() { vec![] } else { vec![blob] },
                        subtree: None,
                        raw: hash(&[
                            b"file",
                            name.as_encoded_bytes(),
                            &blob.0.0,
                            &mode.to_le_bytes(),
                            &uid.to_le_bytes(),
                            &gid.to_le_bytes(),
                            &mtime.as_nanosecond().to_le_bytes(),
                        ]),
                    }
                }
                Entry::Dir(children) => {
                    let sub = self.store_tree(children);
                    Node {
                        name: name.clone(),
                        kind: NodeKind::Dir,
                        size: 0,
                        mode: Some(0o755),
                        uid: Some(1000),
                        gid: Some(1000),
                        mtime: None,
                        content: vec![],
                        subtree: Some(sub),
                        raw: hash(&[b"dir", name.as_encoded_bytes(), &sub.0.0]),
                    }
                }
                Entry::Link(target) => Node {
                    name: name.clone(),
                    kind: NodeKind::Symlink {
                        target: target.clone(),
                    },
                    size: 0,
                    mode: Some(0o777),
                    uid: Some(1000),
                    gid: Some(1000),
                    mtime: None,
                    content: vec![],
                    subtree: None,
                    raw: hash(&[b"link", name.as_encoded_bytes(), target.as_encoded_bytes()]),
                },
            };
            nodes.push(node);
        }
        let raws: Vec<&[u8]> = nodes.iter().map(|n| &n.raw.0[..]).collect();
        let id = TreeId(hash(&raws));
        self.trees
            .entry(id)
            .or_insert_with(|| Arc::new(Tree { nodes }));
        id
    }
}

impl Repo for FakeRepo {
    fn id(&self) -> Id {
        let ids: Vec<&[u8]> = self.snapshots.iter().map(|s| &s.id.0.0[..]).collect();
        hash(&ids)
    }

    fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        Ok(self.snapshots.clone())
    }

    fn tree(&self, id: &TreeId) -> Result<Arc<Tree>> {
        self.trees
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("tree {id} not found"))
    }

    fn read_at(&self, node: &Node, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        for b in &node.content {
            data.extend(self.blobs.get(b).context("blob not found")?);
        }
        let start = (offset as usize).min(data.len());
        let end = start.saturating_add(len as usize).min(data.len());
        Ok(data[start..end].to_vec())
    }

    fn restore(&self, snap: &SnapshotInfo, path: &Path, dest: &Path) -> Result<()> {
        if dest.symlink_metadata().is_ok() {
            bail!("{} already exists", dest.display());
        }
        let mut node = None;
        let mut tree = self.tree(&snap.tree)?;
        for c in parts(path.to_str().context("path must be UTF-8")?) {
            let n = tree
                .get(&c)
                .with_context(|| format!("{} not found", path.display()))?
                .clone();
            if let Some(sub) = n.subtree {
                tree = self.tree(&sub)?;
            }
            node = Some(n);
        }
        self.write_node(&node.context("nothing to restore")?, dest)
    }
}

impl FakeRepo {
    /// Writes a node (and what's under it) to `dest`, like a restore.
    fn write_node(&self, node: &Node, dest: &Path) -> Result<()> {
        match &node.kind {
            NodeKind::Dir => {
                std::fs::create_dir(dest)?;
                if let Some(sub) = node.subtree {
                    for n in &self.tree(&sub)?.nodes {
                        self.write_node(n, &dest.join(&n.name))?;
                    }
                }
            }
            NodeKind::Symlink { target } => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, dest)?;
            }
            NodeKind::File => {
                let data = self.read_at(node, 0, node.size)?;
                std::fs::write(dest, data)?;
                #[cfg(unix)]
                if let Some(mode) = node.mode {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode & 0o7777))?;
                }
                if let Some(t) = node.mtime {
                    let f = std::fs::File::options().write(true).open(dest)?;
                    f.set_modified(std::time::SystemTime::from(t))?;
                }
            }
            NodeKind::Other(_) => {}
        }
        Ok(())
    }
}

/// Look a name up in a tree's entries; handy in tests.
pub fn name(s: &str) -> &OsStr {
    OsStr::new(s)
}

impl FakeDisk {
    fn find(&self, path: &Path) -> Option<&Entry> {
        let p = parts(path.to_str()?);
        let (name, parent) = p.split_last()?;
        let mut d = &self.top;
        for c in parent {
            match d.get(c)? {
                Entry::Dir(m) => d = m,
                _ => return None,
            }
        }
        d.get(name)
    }
}

fn disk_entry(e: &Entry) -> crate::disk::DiskEntry {
    use crate::disk::{DiskEntry, DiskKind};
    match e {
        Entry::File {
            data, mode, mtime, ..
        } => DiskEntry {
            kind: DiskKind::File,
            size: data.len() as u64,
            mtime: Some(*mtime),
            mode: Some(*mode),
        },
        Entry::Dir(_) => DiskEntry {
            kind: DiskKind::Dir,
            size: 0,
            mtime: None,
            mode: None,
        },
        Entry::Link(_) => DiskEntry {
            kind: DiskKind::Symlink,
            size: 0,
            mtime: None,
            mode: None,
        },
    }
}

impl crate::disk::Disk for FakeDisk {
    fn stat(&self, path: &Path) -> Option<crate::disk::DiskEntry> {
        self.find(path).map(disk_entry)
    }

    fn read_dir(&self, path: &Path) -> Option<Vec<(OsString, crate::disk::DiskEntry)>> {
        let d = if components_empty(path) {
            &self.top
        } else {
            match self.find(path)? {
                Entry::Dir(m) => m,
                _ => return None,
            }
        };
        Some(d.iter().map(|(n, e)| (n.clone(), disk_entry(e))).collect())
    }

    fn read(&self, path: &Path, limit: u64) -> Option<Vec<u8>> {
        match self.find(path)? {
            Entry::File { data, .. } => Some(data[..data.len().min(limit as usize)].to_vec()),
            _ => None,
        }
    }
}

fn components_empty(path: &Path) -> bool {
    path.to_str().is_some_and(|p| parts(p).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DSL: &str = "
host laptop
root /home/me/proj
snapshot 2026-07-24 12:16
  write src/main.go v1\\n
  write src/util.go u
  write README.md readme  # not a comment: part of the text
snapshot 2026-07-25 12:00
  touch src/util.go
snapshot 2026-07-26 12:00
  append src/main.go v2\\n
  rm README.md
";

    #[test]
    fn builds_nested_trees_and_shares_unchanged_ones() -> Result<()> {
        let repo = FakeRepo::parse(DSL)?;
        let snaps = repo.snapshots()?;
        assert_eq!(snaps.len(), 3);
        assert_eq!(snaps[0].host, "laptop");
        assert_eq!(snaps[0].paths, vec![PathBuf::from("/home/me/proj")]);

        let walk = |s: &SnapshotInfo, path: &[&str]| -> Result<Node> {
            let mut tree = repo.tree(&s.tree)?;
            let mut node = None;
            for p in path {
                let n = tree.get(name(p)).context("missing")?.clone();
                if let Some(sub) = n.subtree {
                    tree = repo.tree(&sub)?;
                }
                node = Some(n);
            }
            node.context("empty")
        };
        let src = ["home", "me", "proj", "src"];
        let s0 = walk(&snaps[0], &src)?;
        let s1 = walk(&snaps[1], &src)?;
        let s2 = walk(&snaps[2], &src)?;
        // touch changes the tree id (mtime is stored), like restic
        assert_ne!(s0.subtree, s1.subtree);
        assert_ne!(s1.subtree, s2.subtree);

        let main = walk(&snaps[2], &["home", "me", "proj", "src", "main.go"])?;
        assert_eq!(repo.read_file(&main, 100)?.data, b"v1\nv2\n");
        assert_eq!(repo.read_file(&main, 2)?.data, b"v1");
        let readme = walk(&snaps[0], &["home", "me", "proj", "README.md"])?;
        assert_eq!(
            repo.read_file(&readme, 100)?.data,
            b"readme  # not a comment: part of the text"
        );
        Ok(())
    }
}
