// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned SQLite connection, file and integrity-key lifetime.
//! Business schemas and transactions are supplied by their typed owner.

#![forbid(unsafe_code)]

mod codes;
mod error;
#[cfg(test)]
#[path = "tests/gate_l1.rs"]
mod gate_l1;
mod maintenance;
pub use maintenance::*;

pub use codes::{DatabaseCode, DatabaseLocation};
pub use error::{RuntimeDatabaseError, RuntimeDatabaseResult};

use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DATABASE_FILE: &str = "runtime-state.sqlite";
pub const INTEGRITY_KEY_FILE: &str = "runtime-state.key";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Commit synchronization of the shared connection (Workflow #191 I).
/// `Full` syncs the WAL on every commit and is the connection's standing level;
/// `Normal` writes the commit without a sync and is only set by the Ledger around
/// one observational event transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitSync {
    Full,
    Normal,
}

#[cfg(test)]
thread_local! {
    /// Test plan v2.2 fake I2: a `synchronous` readback that the next `Full` call of
    /// `set_commit_sync` on this thread uses instead of the connection's (L-1 G7 gate test).
    pub(crate) static FULL_SYNC_READBACK_HOOK: std::cell::Cell<Option<i64>> =
        const { std::cell::Cell::new(None) };
}

/// One physical connection shared by Runtime-owned typed facades.
/// The enclosing Host OwnerGuard continues to arbitrate process ownership.
pub struct RuntimeDatabase {
    root: PathBuf,
    database_path: PathBuf,
    connection: Mutex<Connection>,
    integrity_key: Box<[u8]>,
    /// Set while the connection may be below `Full`; left set when restoring failed.
    sync_relaxed: AtomicBool,
    /// Workflow #381 R6: the longest `connection()` lock wait, in nanoseconds, since the last
    /// `take_connection_wait_peak`. Process-local, never persisted.
    connection_wait_peak_ns: AtomicU64,
}

/// Borrowed only for a trusted Runtime owner's work in the writer's transaction.
/// It cannot own, commit or retain the transaction beyond the call.
pub struct RuntimeTransaction<'a, 'connection> {
    database: &'a RuntimeDatabase,
    transaction: &'a rusqlite::Transaction<'connection>,
}

impl<'connection> RuntimeTransaction<'_, 'connection> {
    pub fn belongs_to(&self, database: &RuntimeDatabase) -> bool {
        std::ptr::eq(self.database, database)
    }

    pub fn sql(&self) -> &rusqlite::Transaction<'connection> {
        self.transaction
    }
}

impl RuntimeDatabase {
    pub fn borrow_transaction<'a, 'connection>(
        &'a self,
        transaction: &'a rusqlite::Transaction<'connection>,
    ) -> RuntimeTransaction<'a, 'connection> {
        RuntimeTransaction {
            database: self,
            transaction,
        }
    }
    /// Opens the existing state database with its caller-owned schema and version.
    /// Root preparation runs before the key or connection is opened, preserving
    /// the state owner's release-file preparation and its original error order.
    pub fn open<E>(
        root: &Path,
        bootstrap_seed: &[u8],
        schema_sql: &str,
        expected_version: &str,
        prepare_root: impl FnOnce(&Path) -> Result<(), E>,
    ) -> Result<Self, E>
    where
        E: From<RuntimeDatabaseError>,
    {
        Self::open_with_initializer(root, bootstrap_seed, prepare_root, |connection| {
            connection.execute_batch(schema_sql).map_err(|_| {
                failure(
                    DatabaseCode::StateSchemaInitializeFailed.code(),
                    ContractLocation::OpenRuntimeState.location(),
                )
            })?;
            let schema_version = connection
                .query_row(
                    "SELECT value FROM state_meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| {
                    failure(
                        DatabaseCode::StateSchemaMetadataMissing.code(),
                        ContractLocation::OpenRuntimeState.location(),
                    )
                })?;
            if schema_version != expected_version {
                return Err(failure(
                    DatabaseCode::StateSchemaVersionUnsupported.code(),
                    ContractLocation::OpenRuntimeState.location(),
                )
                .into());
            }
            Ok(())
        })
    }

    /// Opens the physical owner with a component-owned initialization callback.
    /// The callback runs before quick_check and before publishing the connection.
    pub fn open_with_initializer<E>(
        root: &Path,
        bootstrap_seed: &[u8],
        prepare_root: impl FnOnce(&Path) -> Result<(), E>,
        initialize: impl FnOnce(&Connection) -> Result<(), E>,
    ) -> Result<Self, E>
    where
        E: From<RuntimeDatabaseError>,
    {
        if bootstrap_seed.len() < 16 || bootstrap_seed.len() > 1024 {
            return Err(failure(
                DatabaseCode::StateIntegrityKeyInvalid.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
            .into());
        }
        fs::create_dir_all(root).map_err(|_| {
            failure(
                SharedCode::StateRootCreateFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
        })?;
        require_regular_directory(root)?;
        // Workflow #332 H2f: a fresh root takes the canonical form `open_existing` stores, so
        // the first session compares roots (e.g. the material root) like every later one.
        let canonical_root = root.canonicalize().map_err(|error| {
            RuntimeDatabaseError::io(
                SharedCode::StateRootInspectFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
                &error,
            )
        })?;
        prepare_root(root)?;
        let database_path = root.join(DATABASE_FILE);
        let database_existed = database_path.exists();
        if database_existed {
            require_regular_file(&database_path)?;
        }
        let integrity_key = load_or_create_integrity_key(root, bootstrap_seed, database_existed)?;
        let connection = Connection::open(&database_path).map_err(|_| {
            failure(
                DatabaseCode::StateDatabaseOpenFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
        })?;
        connection.busy_timeout(BUSY_TIMEOUT).map_err(|_| {
            failure(
                DatabaseCode::StateDatabaseConfigFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
        })?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
            )
            .map_err(|_| {
                failure(
                    DatabaseCode::StateDatabaseConfigFailed.code(),
                    ContractLocation::OpenRuntimeState.location(),
                )
            })?;
        initialize(&connection)?;
        let integrity = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
            .map_err(|_| {
                failure(
                    DatabaseCode::StateIntegrityCheckFailed.code(),
                    ContractLocation::OpenRuntimeState.location(),
                )
            })?;
        if integrity != "ok" {
            return Err(failure(
                DatabaseCode::StateDatabaseCorrupt.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
            .into());
        }
        Ok(Self {
            root: canonical_root,
            database_path,
            connection: Mutex::new(connection),
            integrity_key,
            sync_relaxed: AtomicBool::new(false),
            connection_wait_peak_ns: AtomicU64::new(0),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Opens existing material without initialization, key creation, or schema writes.
    pub fn open_existing(root: &Path, read_only: bool) -> RuntimeDatabaseResult<Self> {
        Self::open_existing_storage(root, read_only, true)
    }

    /// Workflow #375 R375-3 v1.2: opens existing storage read-only exactly as
    /// `open_existing(root, true)` does, without the whole-file `PRAGMA quick_check(1)`.
    /// Only for the check-config catalog preview and the offline `actingledger events` and
    /// `export --performance` range opening (Workflow #411 A4), which authenticate every row
    /// they read, and the start's ledger format probe, which reads only the format
    /// (`user_version` and the ledger table count, Workflow #378 A3); startup and
    /// `ledger-maintenance verify` open through `open_existing`, which still runs the check.
    pub fn open_existing_unchecked(root: &Path) -> RuntimeDatabaseResult<Self> {
        Self::open_existing_storage(root, true, false)
    }

    /// The one body of `open_existing` and `open_existing_unchecked`. A read-write open
    /// always runs the whole-file check.
    fn open_existing_storage(
        root: &Path,
        read_only: bool,
        whole_file_check: bool,
    ) -> RuntimeDatabaseResult<Self> {
        require_regular_directory(root)?;
        let database_path = root.join(DATABASE_FILE);
        require_regular_file(&database_path)?;
        let key_path = root.join(INTEGRITY_KEY_FILE);
        require_regular_file(&key_path)?;
        if fs::metadata(&key_path)
            .map_err(|error| {
                RuntimeDatabaseError::io(
                    DatabaseCode::StateIntegrityKeyReadFailed.code(),
                    DatabaseLocation::InspectExistingKey.location(),
                    &error,
                )
            })?
            .len()
            != 32
        {
            return Err(failure(
                DatabaseCode::StateIntegrityKeyInvalid.code(),
                DatabaseLocation::OpenExistingDatabase.location(),
            ));
        }
        let integrity_key = fs::read(&key_path).map_err(|error| {
            RuntimeDatabaseError::io(
                DatabaseCode::StateIntegrityKeyReadFailed.code(),
                DatabaseLocation::OpenExistingDatabase.location(),
                &error,
            )
        })?;
        if integrity_key.len() != 32 {
            return Err(failure(
                DatabaseCode::StateIntegrityKeyInvalid.code(),
                DatabaseLocation::OpenExistingDatabase.location(),
            ));
        }
        let flags = if read_only {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let connection = Connection::open_with_flags(&database_path, flags).map_err(|error| {
            RuntimeDatabaseError::sql(DatabaseLocation::OpenExistingDatabase.location(), &error)
        })?;
        connection.busy_timeout(BUSY_TIMEOUT).map_err(|error| {
            RuntimeDatabaseError::sql(
                DatabaseLocation::ConfigureExistingDatabase.location(),
                &error,
            )
        })?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| {
                RuntimeDatabaseError::sql(
                    DatabaseLocation::ConfigureExistingDatabase.location(),
                    &error,
                )
            })?;
        if !read_only {
            let journal: String = connection
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::InspectExistingJournal.location(),
                        &error,
                    )
                })?;
            if journal != "wal" {
                return Err(failure(
                    DatabaseCode::StateDatabaseJournalInvalid.code(),
                    DatabaseLocation::OpenExistingDatabase.location(),
                ));
            }
            connection
                .pragma_update(None, "synchronous", "FULL")
                .map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::ConfigureExistingDatabase.location(),
                        &error,
                    )
                })?;
        }
        if whole_file_check || !read_only {
            let integrity: String = connection
                .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
                .map_err(|error| {
                    RuntimeDatabaseError::sql(
                        DatabaseLocation::VerifyExistingDatabase.location(),
                        &error,
                    )
                })?;
            if integrity != "ok" {
                return Err(failure(
                    DatabaseCode::StateDatabaseCorrupt.code(),
                    DatabaseLocation::OpenExistingDatabase.location(),
                ));
            }
        }
        Ok(Self {
            root: root.canonicalize().map_err(|error| {
                RuntimeDatabaseError::io(
                    SharedCode::StateRootInspectFailed.code(),
                    DatabaseLocation::OpenExistingDatabase.location(),
                    &error,
                )
            })?,
            database_path,
            connection: Mutex::new(connection),
            integrity_key: integrity_key.into_boxed_slice(),
            sync_relaxed: AtomicBool::new(false),
            connection_wait_peak_ns: AtomicU64::new(0),
        })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Internal owner access, held for the caller's complete operation/transaction.
    /// SQLite's busy timeout does not bound this Rust mutex acquisition.
    /// A connection whose `Full` level could not be restored is refused.
    pub fn connection(
        &self,
        operation: Location,
    ) -> RuntimeDatabaseResult<MutexGuard<'_, Connection>> {
        let waiting = Instant::now();
        let guard = self
            .connection
            .lock()
            .map_err(|_| failure(DatabaseCode::StateConnectionPoisoned.code(), operation))?;
        self.connection_wait_peak_ns.fetch_max(
            u64::try_from(waiting.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.require_full_sync(operation)?;
        Ok(guard)
    }

    /// Workflow #381 R6: the longest wait any `connection()` caller spent on the connection
    /// lock since the previous call, which opens a new window. `try_connection` never waits.
    pub fn take_connection_wait_peak(&self) -> Duration {
        Duration::from_nanos(self.connection_wait_peak_ns.swap(0, Ordering::Relaxed))
    }

    /// Workflow #381 R4b: a second connection to the same file, read-only, for one long read
    /// that must not hold the shared connection's lock (the Ledger writer takes that lock for
    /// every append). The caller runs its whole read inside one transaction on it, which in WAL
    /// mode reads one snapshot while commits on the shared connection go on. There is no
    /// `quick_check`: the shared connection's open already checked the file. Every failure is
    /// returned (`database_maintenance_sql_failed`, `state_database_journal_invalid`); the
    /// caller never falls back to the shared connection.
    pub fn read_snapshot_connection(
        &self,
        operation: Location,
    ) -> RuntimeDatabaseResult<Connection> {
        let connection = Connection::open_with_flags(
            &self.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|error| RuntimeDatabaseError::sql(operation, &error))?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| RuntimeDatabaseError::sql(operation, &error))?;
        let journal: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|error| RuntimeDatabaseError::sql(operation, &error))?;
        if journal != "wal" {
            return Err(failure(
                DatabaseCode::StateDatabaseJournalInvalid.code(),
                operation,
            ));
        }
        Ok(connection)
    }

    /// Error readback must not wait behind another owner of the connection.
    pub fn try_connection(
        &self,
        operation: Location,
    ) -> RuntimeDatabaseResult<MutexGuard<'_, Connection>> {
        let guard = self.connection.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => {
                failure(DatabaseCode::StateConnectionBusy.code(), operation)
            }
            std::sync::TryLockError::Poisoned(_) => {
                failure(DatabaseCode::StateConnectionPoisoned.code(), operation)
            }
        })?;
        self.require_full_sync(operation)?;
        Ok(guard)
    }

    /// Sets the commit synchronization of the connection behind the caller's
    /// `connection()` guard, outside any transaction. For the Ledger only, around
    /// one event transaction; it must restore `Full` before releasing the guard.
    /// Each call prepares a fresh statement (SQLite applies `synchronous` while
    /// preparing, so a cached statement would not change it) and reads the level
    /// back. Every failure is `state_database_sync_config_failed`; a connection
    /// left relaxed is refused to every later user (`state_database_sync_relaxed`).
    pub fn set_commit_sync(
        &self,
        connection: &Connection,
        sync: CommitSync,
    ) -> RuntimeDatabaseResult<()> {
        const OPERATION: Location = DatabaseLocation::SetCommitSync.location();
        let (level, expected) = match sync {
            CommitSync::Full => ("FULL", 2),
            CommitSync::Normal => {
                self.sync_relaxed.store(true, Ordering::SeqCst);
                ("NORMAL", 1)
            }
        };
        let sync_failure = |error: rusqlite::Error| {
            failure(
                DatabaseCode::StateDatabaseSyncConfigFailed.code(),
                OPERATION,
            )
            .with_detail(error.sqlite_error().map_or_else(
                || "sqlite=none".to_owned(),
                |error| format!("sqlite={}", error.extended_code),
            ))
        };
        connection
            .pragma_update(None, "synchronous", level)
            .map_err(sync_failure)?;
        let observed = connection
            .pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))
            .map_err(sync_failure)?;
        #[cfg(test)]
        let observed = match sync {
            CommitSync::Full => FULL_SYNC_READBACK_HOOK.take().unwrap_or(observed),
            CommitSync::Normal => observed,
        };
        if observed != expected {
            return Err(failure(
                DatabaseCode::StateDatabaseSyncConfigFailed.code(),
                OPERATION,
            )
            .with_detail(format!("readback={observed}")));
        }
        if sync == CommitSync::Full {
            self.sync_relaxed.store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    fn require_full_sync(&self, operation: Location) -> RuntimeDatabaseResult<()> {
        if self.sync_relaxed.load(Ordering::SeqCst) {
            return Err(failure(
                DatabaseCode::StateDatabaseSyncRelaxed.code(),
                operation,
            ));
        }
        Ok(())
    }

    /// Does not acquire the connection lock; transactions may validate keyed rows.
    pub fn integrity_tag(&self, domain: &str, fields: &[&[u8]]) -> String {
        let mut digest = Sha256::new();
        digest.update(b"actingcommand-keyed-integrity-v1\0");
        update_field(&mut digest, domain.as_bytes());
        update_field(&mut digest, &self.integrity_key);
        for field in fields {
            update_field(&mut digest, field);
        }
        format!("sha256:{:x}", digest.finalize())
    }
}

fn require_regular_directory(path: &Path) -> RuntimeDatabaseResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        RuntimeDatabaseError::io(
            SharedCode::StateRootInspectFailed.code(),
            ContractLocation::OpenRuntimeState.location(),
            &error,
        )
    })?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(failure(
            SharedCode::StateRootUnsafe.code(),
            ContractLocation::OpenRuntimeState.location(),
        ));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> RuntimeDatabaseResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        failure(
            DatabaseCode::StateDatabaseInspectFailed.code(),
            ContractLocation::OpenRuntimeState.location(),
        )
    })?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(failure(
            DatabaseCode::StateDatabaseUnsafe.code(),
            ContractLocation::OpenRuntimeState.location(),
        ));
    }
    Ok(())
}

fn load_or_create_integrity_key(
    root: &Path,
    bootstrap_seed: &[u8],
    database_existed: bool,
) -> RuntimeDatabaseResult<Box<[u8]>> {
    let path = root.join(INTEGRITY_KEY_FILE);
    if path.exists() {
        require_regular_file(&path)?;
        let bytes = fs::read(&path).map_err(|_| {
            failure(
                DatabaseCode::StateIntegrityKeyReadFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
        })?;
        if bytes.len() != 32 {
            return Err(failure(
                DatabaseCode::StateIntegrityKeyInvalid.code(),
                ContractLocation::OpenRuntimeState.location(),
            ));
        }
        return Ok(bytes.into_boxed_slice());
    }
    if database_existed {
        return Err(failure(
            DatabaseCode::StateIntegrityKeyMissing.code(),
            ContractLocation::OpenRuntimeState.location(),
        ));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        failure(
            DatabaseCode::StateIntegrityKeyClockFailed.code(),
            ContractLocation::OpenRuntimeState.location(),
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(b"actingcommand-runtime-state-key-v1\0");
    update_field(&mut digest, bootstrap_seed);
    update_field(&mut digest, &now.as_nanos().to_be_bytes());
    update_field(&mut digest, &std::process::id().to_be_bytes());
    let key = digest.finalize().to_vec();
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|_| {
        failure(
            DatabaseCode::StateIntegrityKeyCreateFailed.code(),
            ContractLocation::OpenRuntimeState.location(),
        )
    })?;
    file.write_all(&key)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            failure(
                DatabaseCode::StateIntegrityKeyWriteFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
        })?;
    sync_state_directory(root)?;
    Ok(key.into_boxed_slice())
}

#[cfg(unix)]
fn sync_state_directory(path: &Path) -> RuntimeDatabaseResult<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            failure(
                SharedCode::StateDirectorySyncFailed.code(),
                ContractLocation::OpenRuntimeState.location(),
            )
        })
}

#[cfg(not(unix))]
fn sync_state_directory(_path: &Path) -> RuntimeDatabaseResult<()> {
    // Rust's standard library cannot open Windows directories for fsync without unsafe flags.
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn update_field(digest: &mut Sha256, field: &[u8]) {
    digest.update((field.len() as u64).to_be_bytes());
    digest.update(field);
}

fn failure(code: Code, operation: Location) -> RuntimeDatabaseError {
    RuntimeDatabaseError::new(code, operation)
}
