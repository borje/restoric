//! M0 spike: a throwaway check that rustic_core can do what restoric needs.
//! See PLAN.md §5 M0. Not part of restoric.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use rustic_backend::BackendOptions;
use rustic_core::repofile::{Node, NodeType, SnapshotFile, Tree};
use rustic_core::{
    CredentialOptions, IndexedTree, LocalDestination, LsOptions, Repository, RepositoryOptions,
    RestoreOptions, TreeId,
};
use sha2::{Digest, Sha256};

#[derive(Parser)]
struct Args {
    /// Repository (as for restic)
    #[arg(short, long, env = "RESTIC_REPOSITORY")]
    repo: Option<String>,
    #[arg(long, env = "RESTIC_REPOSITORY_FILE")]
    repo_file: Option<PathBuf>,
    /// Hostname to keep snapshots of (default: this machine)
    #[arg(long)]
    host: Option<String>,
    /// rustic cache dir (default: rustic's own)
    #[arg(long)]
    cache_dir: Option<PathBuf>,
    /// Delete --cache-dir first, for a cold run
    #[arg(long, requires = "cache_dir")]
    cold: bool,
    /// Absolute folder to walk in every snapshot
    #[arg(long)]
    folder: PathBuf,
    /// Absolute file to read from the oldest snapshot and restore
    #[arg(long)]
    file: Option<PathBuf>,
    /// Skip loading the full index (and so the read and restore)
    #[arg(long)]
    no_full_index: bool,
}

fn rss() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| {
        status
            .lines()
            .find(|l| l.starts_with(k))
            .map(|l| l[k.len()..].trim().to_string())
            .unwrap_or_default()
    };
    format!("rss {} (peak {})", get("VmRSS:"), get("VmHWM:"))
}

fn ms(d: Duration) -> String {
    format!("{:.0} ms", d.as_secs_f64() * 1000.0)
}

fn components(p: &Path) -> Vec<std::ffi::OsString> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect()
}

/// Trees by id, as restoric's in-memory tree cache would hold them.
struct Trees<'a, S> {
    repo: &'a Repository<S>,
    memo: HashMap<TreeId, Arc<Tree>>,
    loads: usize,
}

impl<'a, S: IndexedTree> Trees<'a, S> {
    fn new(repo: &'a Repository<S>) -> Self {
        Self {
            repo,
            memo: HashMap::new(),
            loads: 0,
        }
    }

    fn get(&mut self, id: TreeId) -> Result<Arc<Tree>> {
        if let Some(t) = self.memo.get(&id) {
            return Ok(t.clone());
        }
        self.loads += 1;
        let t = Arc::new(self.repo.get_tree(&id)?);
        self.memo.insert(id, t.clone());
        Ok(t)
    }

    /// The node at `path` under root tree `root`, or None if it doesn't exist.
    fn walk(&mut self, root: TreeId, path: &[std::ffi::OsString]) -> Result<Option<Node>> {
        let mut id = root;
        let mut node = None;
        for (i, name) in path.iter().enumerate() {
            let tree = self.get(id)?;
            let Some(n) = tree.nodes.iter().find(|n| n.name() == name.as_os_str()) else {
                return Ok(None);
            };
            if i + 1 < path.len() {
                match n.subtree {
                    Some(s) => id = s,
                    None => return Ok(None),
                }
            }
            node = Some(n.clone());
        }
        Ok(node)
    }
}

/// Content fingerprint (PLAN.md §2.2), memoised by tree id.
struct Fingerprints {
    memo: HashMap<TreeId, [u8; 32]>,
}

impl Fingerprints {
    fn tree<S: IndexedTree>(&mut self, trees: &mut Trees<'_, S>, id: TreeId) -> Result<[u8; 32]> {
        if let Some(f) = self.memo.get(&id) {
            return Ok(*f);
        }
        let tree = trees.get(id)?;
        let mut h = Sha256::new();
        for n in &tree.nodes {
            h.update(n.name().as_encoded_bytes());
            h.update([0]);
            h.update(self.node(trees, n)?);
        }
        let f: [u8; 32] = h.finalize().into();
        self.memo.insert(id, f);
        Ok(f)
    }

    fn node<S: IndexedTree>(&mut self, trees: &mut Trees<'_, S>, n: &Node) -> Result<[u8; 32]> {
        let mut h = Sha256::new();
        match &n.node_type {
            NodeType::Dir => {
                h.update(b"d");
                if let Some(s) = n.subtree {
                    h.update(self.tree(trees, s)?);
                }
            }
            NodeType::Symlink { linktarget, .. } => {
                h.update(b"l");
                h.update(linktarget.as_bytes());
            }
            other => {
                h.update(other.to_string().as_bytes());
                h.update(n.meta.size.to_le_bytes());
                h.update(n.meta.mode.unwrap_or(0).to_le_bytes());
                h.update(n.meta.uid.unwrap_or(0).to_le_bytes());
                h.update(n.meta.gid.unwrap_or(0).to_le_bytes());
                for c in n.content.iter().flatten() {
                    h.update(c.to_string().as_bytes());
                }
            }
        }
        Ok(h.finalize().into())
    }
}

/// The other way to find change points: diff two trees, going only into
/// subtrees whose ids differ, and stop at the first content change.
fn content_differs<S: IndexedTree>(
    trees: &mut Trees<'_, S>,
    fps: &mut Fingerprints,
    a: TreeId,
    b: TreeId,
) -> Result<bool> {
    if a == b {
        return Ok(false);
    }
    let (ta, tb) = (trees.get(a)?, trees.get(b)?);
    if ta.nodes.len() != tb.nodes.len() {
        return Ok(true);
    }
    for (na, nb) in ta.nodes.iter().zip(tb.nodes.iter()) {
        if na.name != nb.name {
            return Ok(true);
        }
        let differs = match (&na.node_type, &nb.node_type, na.subtree, nb.subtree) {
            (NodeType::Dir, NodeType::Dir, Some(sa), Some(sb)) => {
                content_differs(trees, fps, sa, sb)?
            }
            (NodeType::Dir, _, _, _) | (_, NodeType::Dir, _, _) => true,
            _ => fps.node(trees, na)? != fps.node(trees, nb)?,
        };
        if differs {
            return Ok(true);
        }
    }
    Ok(false)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let repo_str = match (&args.repo, &args.repo_file) {
        (Some(r), _) => r.clone(),
        (None, Some(f)) => std::fs::read_to_string(f)?.trim().to_string(),
        _ => bail!("no repository: use --repo or RESTIC_REPOSITORY"),
    };
    let host = args
        .host
        .clone()
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());

    // Same environment as restic.
    let mut cred_opts = CredentialOptions::default();
    if let Ok(p) = std::env::var("RESTIC_PASSWORD") {
        cred_opts = cred_opts.password(p);
    } else if let Ok(f) = std::env::var("RESTIC_PASSWORD_FILE") {
        cred_opts = cred_opts.password_file(PathBuf::from(f));
    } else if let Ok(c) = std::env::var("RESTIC_PASSWORD_COMMAND") {
        cred_opts = cred_opts.password_command(c.parse::<rustic_core::CommandInput>()?);
    }
    let creds = cred_opts
        .credentials()?
        .context("no password: set RESTIC_PASSWORD, _FILE or _COMMAND")?;

    if args.cold {
        let dir = args.cache_dir.as_ref().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
    let mut repo_opts = RepositoryOptions::default();
    if let Some(d) = &args.cache_dir {
        repo_opts = repo_opts.cache_dir(d.clone());
    }
    let backends = BackendOptions::default()
        .repository(repo_str)
        .to_backends()?;

    println!("start: {}", rss());
    let t = Instant::now();
    let repo = Repository::new(&repo_opts, &backends)?.open(&creds)?;
    println!("open: {} — {}", ms(t.elapsed()), rss());

    let t = Instant::now();
    let all = repo.get_all_snapshots()?;
    let mut hosts: Vec<_> = all.iter().map(|s| s.hostname.clone()).collect();
    hosts.sort();
    hosts.dedup();
    let mut snaps: Vec<SnapshotFile> = all
        .into_iter()
        .filter(|s| s.hostname == host)
        .filter(|s| {
            s.paths
                .iter()
                .any(|p| args.folder.starts_with(Path::new(p)))
        })
        .collect();
    snaps.sort_by(|a, b| a.time.cmp(&b.time));
    println!(
        "snapshots: {} for host {host:?} covering {} ({} hosts in repo: {hosts:?}) — {}",
        snaps.len(),
        args.folder.display(),
        hosts.len(),
        ms(t.elapsed())
    );
    if snaps.is_empty() {
        bail!("no snapshots to walk");
    }

    let t = Instant::now();
    let repo = repo.to_indexed_ids()?;
    println!("tree index (to_indexed_ids): {} — {}", ms(t.elapsed()), rss());

    let path = components(&args.folder);

    // Pass 1: walk with an empty in-memory cache (cold if --cold).
    // Pass 2: walk again with a fresh in-memory cache (rustic's disk cache warm).
    // Pass 3: walk again with the in-memory cache of pass 2 (hot).
    let mut ids: Vec<Option<TreeId>> = Vec::new();
    for pass in 1..=2 {
        let mut trees = Trees::new(&repo);
        let t = Instant::now();
        ids = snaps
            .iter()
            .map(|s| Ok(trees.walk(s.tree, &path)?.and_then(|n| n.subtree)))
            .collect::<Result<_>>()?;
        println!(
            "walk pass {pass}: {} snapshots, {} tree loads — {} — {}",
            snaps.len(),
            trees.loads,
            ms(t.elapsed()),
            rss()
        );
        if pass == 2 {
            let t = Instant::now();
            for s in &snaps {
                trees.walk(s.tree, &path)?;
            }
            println!("walk pass 3 (in-memory cache): {}", ms(t.elapsed()));

            // Change points by diffing consecutive trees, with a fresh in-memory cache.
            let mut dtrees = Trees::new(&repo);
            let mut dfps = Fingerprints {
                memo: HashMap::new(),
            };
            let t = Instant::now();
            let mut diff_changes = 0;
            for w in ids.windows(2) {
                let differs = match (w[0], w[1]) {
                    (Some(a), Some(b)) => content_differs(&mut dtrees, &mut dfps, a, b)?,
                    (None, None) => false,
                    _ => true,
                };
                diff_changes += usize::from(differs);
            }
            println!(
                "change points by tree diff: {diff_changes}, {} tree loads — {}",
                dtrees.loads,
                ms(t.elapsed())
            );

            // Change points: tree id vs fingerprint.
            let mut fps = Fingerprints {
                memo: HashMap::new(),
            };
            let loads_before = trees.loads;
            let t = Instant::now();
            let fp: Vec<Option<[u8; 32]>> = ids
                .iter()
                .map(|id| id.map(|id| fps.tree(&mut trees, id)).transpose())
                .collect::<Result<_>>()?;
            println!(
                "fingerprints: {} distinct folder trees, {} more tree loads — {}",
                fps.memo.len(),
                trees.loads - loads_before,
                ms(t.elapsed())
            );
            let id_changes = ids.windows(2).filter(|w| w[0] != w[1]).count();
            let fp_changes = fp.windows(2).filter(|w| w[0] != w[1]).count();
            println!(
                "change points over {} snapshot pairs: tree id changed in {id_changes}, fingerprint changed in {fp_changes}",
                snaps.len() - 1
            );
        }
    }
    let present = ids.iter().filter(|i| i.is_some()).count();
    println!("folder present in {present}/{} snapshots", snaps.len());
    drop(repo);

    if args.no_full_index {
        return Ok(());
    }
    let Some(file) = &args.file else {
        return Ok(());
    };

    let t = Instant::now();
    let repo = Repository::new(&repo_opts, &backends)?
        .open(&creds)?
        .to_indexed()?;
    println!("full index (to_indexed): {} — {}", ms(t.elapsed()), rss());

    let file_path = components(file);
    let mut trees = Trees::new(&repo);
    let (snap, node) = snaps
        .iter()
        .find_map(|s| match trees.walk(s.tree, &file_path) {
            Ok(Some(n)) => Some(Ok((s, n))),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        })
        .context("file not in any snapshot")??;
    println!(
        "oldest snapshot with the file: {} at {}",
        snap.id,
        snap.time
    );

    let t = Instant::now();
    let open = repo.open_file(&node)?;
    let bytes = repo.read_file_at(&open, 0, node.meta.size as usize)?;
    let read_sha = Sha256::digest(&bytes);
    println!(
        "read {} bytes — {} — sha256 {:x}",
        bytes.len(),
        ms(t.elapsed()),
        read_sha
    );

    let dir = tempfile::tempdir()?;
    let dest_path = dir.path().join(node.name());
    let t = Instant::now();
    let opts = RestoreOptions::default();
    let ls = repo.ls(&node, &LsOptions::default())?;
    let dest = LocalDestination::new(dest_path.to_str().unwrap(), true, true)?;
    let plan = repo.prepare_restore(&opts, ls.clone(), &dest, false)?;
    repo.restore(plan, &opts, ls, &dest)?;
    let restored = std::fs::read(&dest_path)?;
    let restored_sha = Sha256::digest(&restored);
    println!(
        "restored to {} — {} — sha256 {:x} — {}",
        dest_path.display(),
        ms(t.elapsed()),
        restored_sha,
        if restored_sha == read_sha {
            "matches read"
        } else {
            "DIFFERS from read"
        }
    );
    println!("end: {}", rss());
    Ok(())
}
