//! The on-disk cache (PLAN.md §4.4): one redb file per repository.
//!
//! Every key is content-addressed (snapshot ids, tree ids), so nothing ever
//! needs invalidating. Writes collect in memory and go to disk on
//! [`Cache::flush`], in one transaction. Without a file (tests, demo) the
//! cache lives in memory only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, TableDefinition};

use crate::repo::Id;

const SCHEMA: &str = "1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Table {
    /// (snapshot id, path) → what's at that path (`index::NodeRef`)
    PathRef,
    /// (mode, tree a, tree b) → whether their content differs
    Differs,
    /// (mode, tree a, tree b) → added, changed, deleted counts
    Counts,
}

impl Table {
    const ALL: [Table; 3] = [Table::PathRef, Table::Differs, Table::Counts];

    fn def(self) -> TableDefinition<'static, &'static [u8], &'static [u8]> {
        TableDefinition::new(match self {
            Table::PathRef => "path_ref",
            Table::Differs => "differs",
            Table::Counts => "counts",
        })
    }
}

const META: TableDefinition<&str, &str> = TableDefinition::new("meta");

/// Writes not yet on disk, by table and key.
type Pending = HashMap<(Table, Vec<u8>), Vec<u8>>;

pub struct Cache {
    db: Option<Database>,
    pending: Mutex<Pending>,
}

impl Cache {
    pub fn in_memory() -> Self {
        Self {
            db: None,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// `~/.cache/restoric/<repo id>.redb`
    pub fn default_path(repo: Id) -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "restoric")?;
        Some(dirs.cache_dir().join(format!("{}.redb", repo.to_hex())))
    }

    /// Opens the cache file, starting a new one if it was written by another
    /// schema or backend version. `backend` names the rustic_core version.
    /// A file bigger than `cap` bytes starts over: everything in it can be
    /// computed again.
    pub fn open(path: &Path, backend: &str, cap: u64) -> Result<Self> {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > cap) {
            tracing::info!("cache over {cap} bytes, starting a new one");
            let _ = std::fs::remove_file(path);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let db = match Self::open_matching(path, backend) {
            Ok(Some(db)) => db,
            Ok(None) | Err(_) => {
                let _ = std::fs::remove_file(path);
                Self::create(path, backend)?
            }
        };
        Ok(Self {
            db: Some(db),
            pending: Mutex::new(HashMap::new()),
        })
    }

    fn open_matching(path: &Path, backend: &str) -> Result<Option<Database>> {
        if !path.exists() {
            return Ok(None);
        }
        let db = Database::create(path)?;
        let ok = {
            let r = db.begin_read()?;
            let meta = r.open_table(META)?;
            let get =
                |k| -> Result<Option<String>> { Ok(meta.get(k)?.map(|v| v.value().to_string())) };
            get("schema")?.as_deref() == Some(SCHEMA) && get("backend")?.as_deref() == Some(backend)
        };
        Ok(ok.then_some(db))
    }

    fn create(path: &Path, backend: &str) -> Result<Database> {
        let db = Database::create(path).with_context(|| format!("creating {}", path.display()))?;
        let w = db.begin_write()?;
        {
            let mut meta = w.open_table(META)?;
            meta.insert("schema", SCHEMA)?;
            meta.insert("backend", backend)?;
            for t in Table::ALL {
                w.open_table(t.def())?;
            }
        }
        w.commit()?;
        Ok(db)
    }

    pub fn get(&self, table: Table, key: &[u8]) -> Result<Option<Vec<u8>>> {
        if let Some(v) = self.pending.lock().unwrap().get(&(table, key.to_vec())) {
            return Ok(Some(v.clone()));
        }
        let Some(db) = &self.db else { return Ok(None) };
        let r = db.begin_read()?;
        let t = r.open_table(table.def())?;
        Ok(t.get(key)?.map(|v| v.value().to_vec()))
    }

    pub fn put(&self, table: Table, key: Vec<u8>, value: Vec<u8>) {
        self.pending.lock().unwrap().insert((table, key), value);
    }

    /// Writes what's pending to disk. A no-op in memory.
    pub fn flush(&self) -> Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        if pending.is_empty() {
            return Ok(());
        }
        let w = db.begin_write()?;
        {
            let mut tables = HashMap::new();
            for t in Table::ALL {
                tables.insert(t, w.open_table(t.def())?);
            }
            for ((t, k), v) in &pending {
                tables
                    .get_mut(t)
                    .expect("all tables open")
                    .insert(k.as_slice(), v.as_slice())?;
            }
        }
        w.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_values_across_reopen_and_wipes_on_version_change() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("c.redb");
        let c = Cache::open(&path, "v1", u64::MAX)?;
        c.put(Table::Differs, vec![1, 2], vec![1]);
        assert_eq!(c.get(Table::Differs, &[1, 2])?, Some(vec![1]));
        c.flush()?;
        drop(c);

        let c = Cache::open(&path, "v1", u64::MAX)?;
        assert_eq!(c.get(Table::Differs, &[1, 2])?, Some(vec![1]));
        drop(c);

        let c = Cache::open(&path, "v1", 1)?;
        assert_eq!(c.get(Table::Differs, &[1, 2])?, None, "over the cap");
        c.put(Table::Differs, vec![1, 2], vec![1]);
        c.flush()?;
        drop(c);

        let c = Cache::open(&path, "v2", u64::MAX)?;
        assert_eq!(c.get(Table::Differs, &[1, 2])?, None);
        Ok(())
    }
}
