// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned SQLite connection, file and integrity-key lifetime.
//! Business schemas and transactions are supplied by their typed owner.

#![forbid(unsafe_code)]

mod error;
mod maintenance;
pub use maintenance::*;

pub use error::{RuntimeDatabaseError, RuntimeDatabaseResult};

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DATABASE_FILE: &str = "runtime-state.sqlite";
pub const INTEGRITY_KEY_FILE: &str = "runtime-state.key";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One physical connection shared by Runtime-owned typed facades.
/// The enclosing Host OwnerGuard continues to arbitrate process ownership.
pub struct RuntimeDatabase {
    root: PathBuf,
    database_path: PathBuf,
    connection: Mutex<Connection>,
    integrity_key: Box<[u8]>,
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
            connection
                .execute_batch(schema_sql)
                .map_err(|_| failure("state_schema_initialize_failed", "open_runtime_state"))?;
            let schema_version = connection
                .query_row(
                    "SELECT value FROM state_meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| failure("state_schema_metadata_missing", "open_runtime_state"))?;
            if schema_version != expected_version {
                return Err(
                    failure("state_schema_version_unsupported", "open_runtime_state").into(),
                );
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
            return Err(failure("state_integrity_key_invalid", "open_runtime_state").into());
        }
        fs::create_dir_all(root)
            .map_err(|_| failure("state_root_create_failed", "open_runtime_state"))?;
        require_regular_directory(root)?;
        prepare_root(root)?;
        let database_path = root.join(DATABASE_FILE);
        let database_existed = database_path.exists();
        if database_existed {
            require_regular_file(&database_path)?;
        }
        let integrity_key = load_or_create_integrity_key(root, bootstrap_seed, database_existed)?;
        let connection = Connection::open(&database_path)
            .map_err(|_| failure("state_database_open_failed", "open_runtime_state"))?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|_| failure("state_database_config_failed", "open_runtime_state"))?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
            )
            .map_err(|_| failure("state_database_config_failed", "open_runtime_state"))?;
        initialize(&connection)?;
        let integrity = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
            .map_err(|_| failure("state_integrity_check_failed", "open_runtime_state"))?;
        if integrity != "ok" {
            return Err(failure("state_database_corrupt", "open_runtime_state").into());
        }
        Ok(Self {
            root: root.to_path_buf(),
            database_path,
            connection: Mutex::new(connection),
            integrity_key,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Opens existing material without initialization, key creation, or schema writes.
    pub fn open_existing(root: &Path, read_only: bool) -> RuntimeDatabaseResult<Self> {
        require_regular_directory(root)?;
        let database_path = root.join(DATABASE_FILE);
        require_regular_file(&database_path)?;
        let key_path = root.join(INTEGRITY_KEY_FILE);
        require_regular_file(&key_path)?;
        if fs::metadata(&key_path)
            .map_err(|error| {
                RuntimeDatabaseError::io(
                    "state_integrity_key_read_failed",
                    "inspect_existing_key",
                    &error,
                )
            })?
            .len()
            != 32
        {
            return Err(failure(
                "state_integrity_key_invalid",
                "open_existing_database",
            ));
        }
        let integrity_key = fs::read(&key_path).map_err(|error| {
            RuntimeDatabaseError::io(
                "state_integrity_key_read_failed",
                "open_existing_database",
                &error,
            )
        })?;
        if integrity_key.len() != 32 {
            return Err(failure(
                "state_integrity_key_invalid",
                "open_existing_database",
            ));
        }
        let flags = if read_only {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let connection = Connection::open_with_flags(&database_path, flags)
            .map_err(|error| RuntimeDatabaseError::sql("open_existing_database", &error))?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| RuntimeDatabaseError::sql("configure_existing_database", &error))?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| RuntimeDatabaseError::sql("configure_existing_database", &error))?;
        if !read_only {
            let journal: String = connection
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .map_err(|error| RuntimeDatabaseError::sql("inspect_existing_journal", &error))?;
            if journal != "wal" {
                return Err(failure(
                    "state_database_journal_invalid",
                    "open_existing_database",
                ));
            }
            connection
                .pragma_update(None, "synchronous", "FULL")
                .map_err(|error| {
                    RuntimeDatabaseError::sql("configure_existing_database", &error)
                })?;
        }
        let integrity: String = connection
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(|error| RuntimeDatabaseError::sql("verify_existing_database", &error))?;
        if integrity != "ok" {
            return Err(failure("state_database_corrupt", "open_existing_database"));
        }
        Ok(Self {
            root: root.canonicalize().map_err(|error| {
                RuntimeDatabaseError::io(
                    "state_root_inspect_failed",
                    "open_existing_database",
                    &error,
                )
            })?,
            database_path,
            connection: Mutex::new(connection),
            integrity_key: integrity_key.into_boxed_slice(),
        })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Internal owner access, held for the caller's complete operation/transaction.
    /// SQLite's busy timeout does not bound this Rust mutex acquisition.
    pub fn connection(
        &self,
        operation: &'static str,
    ) -> RuntimeDatabaseResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| failure("state_connection_poisoned", operation))
    }

    /// Error readback must not wait behind another owner of the connection.
    pub fn try_connection(
        &self,
        operation: &'static str,
    ) -> RuntimeDatabaseResult<MutexGuard<'_, Connection>> {
        self.connection.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => failure("state_connection_busy", operation),
            std::sync::TryLockError::Poisoned(_) => failure("state_connection_poisoned", operation),
        })
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
        RuntimeDatabaseError::io("state_root_inspect_failed", "open_runtime_state", &error)
    })?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(failure("state_root_unsafe", "open_runtime_state"));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> RuntimeDatabaseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| failure("state_database_inspect_failed", "open_runtime_state"))?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(failure("state_database_unsafe", "open_runtime_state"));
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
        let bytes = fs::read(&path)
            .map_err(|_| failure("state_integrity_key_read_failed", "open_runtime_state"))?;
        if bytes.len() != 32 {
            return Err(failure("state_integrity_key_invalid", "open_runtime_state"));
        }
        return Ok(bytes.into_boxed_slice());
    }
    if database_existed {
        return Err(failure("state_integrity_key_missing", "open_runtime_state"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| failure("state_integrity_key_clock_failed", "open_runtime_state"))?;
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
    let mut file = options
        .open(&path)
        .map_err(|_| failure("state_integrity_key_create_failed", "open_runtime_state"))?;
    file.write_all(&key)
        .and_then(|()| file.sync_all())
        .map_err(|_| failure("state_integrity_key_write_failed", "open_runtime_state"))?;
    sync_state_directory(root)?;
    Ok(key.into_boxed_slice())
}

#[cfg(unix)]
fn sync_state_directory(path: &Path) -> RuntimeDatabaseResult<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| failure("state_directory_sync_failed", "open_runtime_state"))
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

fn failure(code: &'static str, operation: &'static str) -> RuntimeDatabaseError {
    RuntimeDatabaseError::new(code, operation)
}
