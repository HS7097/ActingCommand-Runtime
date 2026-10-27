// SPDX-License-Identifier: AGPL-3.0-only

//! `install\writer.lock` (Workflow #421 INST-1; `contracts/install-cli.md`, "Locks and the
//! watchdog"): held exclusively for every change of `runtime\`, `tools\`, the configuration or
//! a game root, created when missing. While it is held the launcher runs nothing, a watchdog tick
//! answers `watchdog_installer_busy` (row 7a) and `actingctl start` / `restart` refuse. A free
//! lock is held shared by a tick until the tick ends, so the installer waits for it, at most
//! `install.lock_wait_ms`. The OS releases the lock when the holder ends.

use super::files;
use super::state::Paths;
use crate::codes::{InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::{IoOp, Outcome};
use std::fs::{File, OpenOptions, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

/// How often a held lock is tried again (the contract's 500 ms probe period).
const RETRY: Duration = Duration::from_millis(500);

/// The writer lock, held exclusively until dropped.
pub(crate) struct WriterLock {
    _file: File,
}

impl WriterLock {
    /// Takes `install\writer.lock` exclusively, waiting up to `wait_ms` while another holds it;
    /// the refusal `writer_lock_failed` of the command `operation` past the wait or when the OS
    /// refuses.
    pub(crate) fn take(
        paths: &Paths,
        wait_ms: u64,
        operation: InstallLocation,
    ) -> Result<Self, Outcome> {
        let path = paths.install().join("writer.lock");
        let failed = |cause: Option<Outcome>| {
            let refusal = super::refusal(operation, InstallRefusal::WriterLockFailed).path(&path);
            match cause {
                Some(cause) => refusal.caused_by(cause),
                None => refusal,
            }
        };
        files::create_dir_all(&paths.install()).map_err(|cause| failed(Some(cause)))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| failed(Some(files::os_failure(&path, &error, IoOp::Open))))?;
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => thread::sleep(RETRY),
                Err(TryLockError::WouldBlock) => return Err(failed(None)),
                Err(TryLockError::Error(error)) => {
                    return Err(failed(Some(files::os_failure(&path, &error, IoOp::Lock))));
                }
            }
        }
    }
}
