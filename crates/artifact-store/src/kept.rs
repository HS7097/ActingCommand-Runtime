// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375 R5d: frames kept for people in `<state root>\kept\<date>\<leaf>\<file>`.
//!
//! The frame cleaner moves an error or Lab frame there once it settles. `<file>` is the local
//! capture time `<HHmmss-fff>_` followed by the object's own file name, `artifact_<hex>.png`.
//! Nothing about a move is in the ledger and the object key stays, so a read that finds no file
//! at the object key looks the artifact id (the text after the first `_`, without `.png`) up in
//! a per-process map of the kept folders.
//!
//! - The map is keyed by the canonical state root, so roots and processes never share one.
//! - The mover (actingd's cleaner) puts each of its moves into its own map before the rename,
//!   and after the rename overwrites `kept\.moves` with its process start time and a move count.
//! - A miss walks the kept folders again when that counter changed, or at most once per 60 s
//!   for changes by hand; the first miss in a process always walks. Misses of deleted frames
//!   with no move cause no walk.
//! - A walk lists the date folders and their leaves below `kept\`, and the files in each leaf.
//!   It reads no file contents and never follows a reparse point. An entry that vanishes or is
//!   being deleted meanwhile, and a file where a folder is expected, are skipped; a name that
//!   does not end in `_artifact_<hex>.png` is ignored. Any other listing error fails the read
//!   (`artifact_kept_walk_failed`) and the previous map stays. Each walk replaces the map whole,
//!   and one root has one walk at a time: readers that miss meanwhile wait for its result.

use crate::codes::StoreCode;
use crate::{ArtifactStoreError, ArtifactStoreResult};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The folder of kept frames, directly below the state root.
pub const KEPT_DIRECTORY: &str = "kept";
/// The move counter, `kept\.moves`.
const MOVES_FILE: &str = ".moves";
/// A miss walks again at most this often while the move counter is unchanged.
const REWALK_INTERVAL: Duration = Duration::from_secs(60);
/// `ERROR_DELETE_PENDING`: an entry that is being deleted.
const DELETE_PENDING: i32 = 303;

/// The mover's counter: its process start time and the number of its moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeptMoves {
    started_unix_ms: u64,
    count: u64,
}

impl KeptMoves {
    /// The counter of a process that started at `started_unix_ms`, so a restart never repeats
    /// a value.
    pub const fn new(started_unix_ms: u64) -> Self {
        Self {
            started_unix_ms,
            count: 0,
        }
    }

    /// Counts one more move and overwrites `kept\.moves` with the new value (16 bytes), so
    /// other processes walk the kept folders again on their next miss. Called after a rename
    /// into `kept\`, which therefore exists.
    pub fn record_move(&mut self, root: &Path) -> std::io::Result<()> {
        self.count = self.count.saturating_add(1);
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&self.started_unix_ms.to_le_bytes());
        bytes[8..].copy_from_slice(&self.count.to_le_bytes());
        fs::write(root.join(KEPT_DIRECTORY).join(MOVES_FILE), bytes)
    }
}

#[derive(Default)]
struct KeptMap {
    /// Artifact id → its kept file.
    paths: HashMap<String, PathBuf>,
    /// The move counter read just before the latest walk; `None` when it was unreadable.
    counter: Option<Vec<u8>>,
    walked_at: Option<Instant>,
    /// Completed walks.
    walks: u64,
    /// While a walk runs: the mover's own entries, put back into the walk's new map.
    during_walk: Option<Vec<(String, PathBuf)>>,
}

impl KeptMap {
    /// The cached kept file of `id`, if it is still there; a vanished one is dropped.
    fn present(&mut self, id: &str) -> Option<PathBuf> {
        let path = self.paths.get(id)?.clone();
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return Some(path);
        }
        // A leaf deleted, renamed or copied by hand: the read counts as a miss.
        self.paths.remove(id);
        None
    }

    /// Whether a miss walks: the first miss in this process, a changed move counter, or no walk
    /// in the last 60 s.
    fn walk_due(&self, root: &Path) -> bool {
        let Some(walked_at) = self.walked_at else {
            return true;
        };
        if walked_at.elapsed() >= REWALK_INTERVAL {
            return true;
        }
        let counter = read_counter(root);
        counter.is_none() || counter != self.counter
    }
}

struct RootMap {
    map: Mutex<KeptMap>,
    walk: Mutex<()>,
}

fn root_map(root: &Path) -> Arc<RootMap> {
    static ROOTS: OnceLock<Mutex<HashMap<PathBuf, Arc<RootMap>>>> = OnceLock::new();
    let mut roots = relock(ROOTS.get_or_init(Mutex::default));
    Arc::clone(roots.entry(root.to_path_buf()).or_insert_with(|| {
        Arc::new(RootMap {
            map: Mutex::default(),
            walk: Mutex::new(()),
        })
    }))
}

/// A poisoned lock only means a panic elsewhere; the map is a cache that a walk rebuilds.
fn relock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The kept file of the object `object_file` (`artifact_<hex>.png`) of the canonical `root`,
/// after a read found nothing at its object key; `Ok(None)` when no kept folder holds it.
pub(crate) fn find(root: &Path, object_file: &str) -> ArtifactStoreResult<Option<PathBuf>> {
    let Some(id) = object_id(object_file) else {
        return Ok(None);
    };
    let shared = root_map(root);
    let walks = {
        let mut map = relock(&shared.map);
        if let Some(path) = map.present(id) {
            return Ok(Some(path));
        }
        if !map.walk_due(root) {
            return Ok(None);
        }
        map.walks
    };
    let _walk = relock(&shared.walk);
    {
        let mut map = relock(&shared.map);
        if map.walks != walks {
            // Another reader walked while this one waited: its map answers.
            return Ok(map.present(id));
        }
        map.during_walk = Some(Vec::new());
    }
    let counter = read_counter(root);
    let walked = walk(root);
    let mut map = relock(&shared.map);
    let own = map.during_walk.take().unwrap_or_default();
    let mut paths = walked?;
    for (own_id, path) in own {
        paths.insert(own_id, path);
    }
    map.paths = paths;
    map.counter = counter;
    map.walked_at = Some(Instant::now());
    map.walks = map.walks.saturating_add(1);
    Ok(map.present(id))
}

/// The mover's own entry, put in before its rename, so this process never misses its move.
pub(crate) fn remember(root: &Path, object_file: &str, path: PathBuf) {
    let Some(id) = object_id(object_file) else {
        return;
    };
    let shared = root_map(root);
    let mut map = relock(&shared.map);
    if let Some(own) = map.during_walk.as_mut() {
        own.push((id.to_owned(), path.clone()));
    }
    map.paths.insert(id.to_owned(), path);
}

/// Takes the mover's entry out again after a failed rename.
pub(crate) fn forget(root: &Path, object_file: &str) {
    let Some(id) = object_id(object_file) else {
        return;
    };
    let shared = root_map(root);
    let mut map = relock(&shared.map);
    if let Some(own) = map.during_walk.as_mut() {
        own.retain(|(own_id, _)| own_id != id);
    }
    map.paths.remove(id);
}

/// The artifact id of an object file name `artifact_<hex>.png`; any other object is never kept.
pub(crate) fn object_id(object_file: &str) -> Option<&str> {
    let id = object_file.strip_suffix(".png")?;
    let hex = id.strip_prefix("artifact_")?;
    (!hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(id)
}

/// The artifact id of a kept file name `<HHmmss-fff>_artifact_<hex>.png`: the text after the
/// first `_`, without `.png`.
fn kept_id(file_name: &str) -> Option<&str> {
    let (_, object_file) = file_name.split_once('_')?;
    object_id(object_file)
}

/// `kept\.moves` as bytes, empty when absent; `None` when unreadable, which counts as changed.
fn read_counter(root: &Path) -> Option<Vec<u8>> {
    match fs::read(root.join(KEPT_DIRECTORY).join(MOVES_FILE)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

fn walk(root: &Path) -> ArtifactStoreResult<HashMap<String, PathBuf>> {
    let mut paths = HashMap::new();
    let kept = root.join(KEPT_DIRECTORY);
    match fs::symlink_metadata(&kept) {
        // Never followed.
        Ok(metadata) if crate::usage::is_link(&metadata) => return Ok(paths),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(paths),
        Err(error) => return Err(walk_failed(&error)),
    }
    for date in entries(&kept, true)?
        .into_iter()
        .filter(|entry| entry.folder)
    {
        for leaf in entries(&date.path, false)?
            .into_iter()
            .filter(|entry| entry.folder)
        {
            for file in entries(&leaf.path, false)?
                .into_iter()
                .filter(|entry| entry.file)
            {
                if let Some(id) = file.name.to_str().and_then(kept_id) {
                    paths.entry(id.to_owned()).or_insert(file.path);
                }
            }
        }
    }
    Ok(paths)
}

struct Entry {
    path: PathBuf,
    name: OsString,
    folder: bool,
    file: bool,
}

/// The plain folders and files in `dir`, never a reparse point. Below `kept\`, a folder or an
/// entry that vanished or is being deleted is skipped; any other error fails the walk.
fn entries(dir: &Path, top: bool) -> ArtifactStoreResult<Vec<Entry>> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if !top && vanished(&error) => return Ok(Vec::new()),
        Err(error) => return Err(walk_failed(&error)),
    };
    let mut found = Vec::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if vanished(&error) => continue,
            Err(error) => return Err(walk_failed(&error)),
        };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if vanished(&error) => continue,
            Err(error) => return Err(walk_failed(&error)),
        };
        if crate::usage::is_link(&metadata) {
            continue;
        }
        found.push(Entry {
            path: entry.path(),
            name: entry.file_name(),
            folder: metadata.is_dir(),
            file: metadata.is_file(),
        });
    }
    Ok(found)
}

fn vanished(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::NotFound || error.raw_os_error() == Some(DELETE_PENDING)
}

fn walk_failed(error: &std::io::Error) -> ArtifactStoreError {
    let mut detail = format!(
        "io_kind={}",
        actingcommand_contract::outcome::IoKind::from_error_kind(error.kind()).as_str()
    );
    if let Some(code) = error.raw_os_error() {
        detail.push_str(&format!(" os_error={code}"));
    }
    ArtifactStoreError::fatal(
        StoreCode::ArtifactKeptWalkFailed.as_str(),
        "read_projected_artifact",
        detail,
    )
    .with_io_error(error)
    .non_fatal()
}

#[cfg(test)]
mod tests;

/// Completed walks of `root` in this process.
#[cfg(test)]
pub(crate) fn walks(root: &Path) -> u64 {
    relock(&root_map(root).map).walks
}

/// Makes the latest walk of `root` 60 s old, as if a minute had passed.
#[cfg(test)]
pub(crate) fn age_walk(root: &Path) {
    let shared = root_map(root);
    let mut map = relock(&shared.map);
    map.walked_at = map
        .walked_at
        .and_then(|walked_at| walked_at.checked_sub(REWALK_INTERVAL));
}
