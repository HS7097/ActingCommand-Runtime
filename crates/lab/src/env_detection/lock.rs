// SPDX-License-Identifier: AGPL-3.0-only

//! Stale recovery holds a per-owner claim and re-reads ownership before rename so delayed
//! contenders cannot replace a newly acquired result lock. Host identity must match before
//! process liveness can prove staleness; timestamps never prove staleness. Windows process
//! proofs have an executable deadline, independent from bounded lock and claim attempts.

use actingcommand_contract::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "windows")]
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOCK_SCHEMA_VERSION: &str = "env-result-lock.v2";
const MAX_LOCK_ACQUIRE_ATTEMPTS: usize = 4;
const MAX_RECLAIM_CLAIM_ATTEMPTS: usize = 4;
const RECORD_READ_TIMEOUT: Duration = Duration::from_secs(1);
const RECORD_READ_DELAY: Duration = Duration::from_millis(5);
#[cfg(target_os = "windows")]
const WINDOWS_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(target_os = "windows")]
const WINDOWS_PROBE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(target_os = "windows")]
const WINDOWS_PROBE_POLL_DELAY: Duration = Duration::from_millis(10);
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static TOKEN_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static RANDOM_SEED: OnceLock<Result<[u8; 32], String>> = OnceLock::new();
static CURRENT_PROCESS_START: OnceLock<Result<String, String>> = OnceLock::new();
static CURRENT_HOST_IDENTITY: OnceLock<Result<String, String>> = OnceLock::new();

type LockResult<T> = LabResult<T>;

#[derive(Debug)]
pub(super) struct EnvResultLock {
    path: PathBuf,
    result_path: PathBuf,
    owner_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    start_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcessStatus {
    Missing,
    Alive { start_token: String },
}

trait LockEnvironment: Send + Sync {
    fn host_identity(&self) -> Result<String, String>;
    fn current_process(&self) -> Result<ProcessIdentity, String>;
    fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String>;
    fn next_owner_token(&self) -> Result<String, String>;
    fn now_unix_ms(&self) -> Result<u64, String>;
    fn after_reclaim_claim_persisted(&self) {}
}

struct SystemLockEnvironment;

impl LockEnvironment for SystemLockEnvironment {
    fn host_identity(&self) -> Result<String, String> {
        CURRENT_HOST_IDENTITY
            .get_or_init(system_host_identity)
            .clone()
    }

    fn current_process(&self) -> Result<ProcessIdentity, String> {
        let pid = std::process::id();
        let start_token = CURRENT_PROCESS_START
            .get_or_init(|| match inspect_system_process(pid) {
                Ok(ProcessStatus::Alive { start_token }) => Ok(start_token),
                Ok(ProcessStatus::Missing) => {
                    Err("current process disappeared during lock acquisition".to_string())
                }
                Err(error) => Err(error),
            })
            .clone()?;
        Ok(ProcessIdentity { pid, start_token })
    }

    fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String> {
        inspect_system_process(pid)
    }

    fn next_owner_token(&self) -> Result<String, String> {
        let seed = RANDOM_SEED.get_or_init(system_random_seed).clone()?;
        let sequence = TOKEN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
            .as_nanos();
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update(std::process::id().to_le_bytes());
        hasher.update(sequence.to_le_bytes());
        hasher.update(now.to_le_bytes());
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn now_unix_ms(&self) -> Result<u64, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
            .as_millis()
            .try_into()
            .map_err(|_| "current Unix timestamp does not fit u64".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvResultLockRecord {
    schema_version: String,
    host_identity: String,
    owner_token: String,
    pid: u32,
    process_start_token: String,
    acquired_at_unix_ms: u64,
    normalized_result_path: String,
}

impl EnvResultLockRecord {
    fn new(environment: &impl LockEnvironment, normalized_result_path: &Path) -> LockResult<Self> {
        let host_identity = environment.host_identity().map_err(|error| {
            lock_error(
                normalized_result_path,
                format!("host identity unavailable: {error}"),
            )
        })?;
        let identity = environment.current_process().map_err(|error| {
            lock_error(
                normalized_result_path,
                format!("owner identity unavailable: {error}"),
            )
        })?;
        let owner_token = environment.next_owner_token().map_err(|error| {
            lock_error(
                normalized_result_path,
                format!("owner token unavailable: {error}"),
            )
        })?;
        let acquired_at_unix_ms = environment.now_unix_ms().map_err(|error| {
            lock_error(
                normalized_result_path,
                format!("acquisition time unavailable: {error}"),
            )
        })?;
        let record = Self {
            schema_version: LOCK_SCHEMA_VERSION.to_string(),
            host_identity,
            owner_token,
            pid: identity.pid,
            process_start_token: identity.start_token,
            acquired_at_unix_ms,
            normalized_result_path: normalized_result_path.display().to_string(),
        };
        record.validate(normalized_result_path)?;
        Ok(record)
    }

    fn validate(&self, expected_result_path: &Path) -> LockResult<()> {
        if self.schema_version != LOCK_SCHEMA_VERSION {
            return Err(lock_error(
                expected_result_path,
                format!(
                    "unsupported lock schema '{}'; expected {LOCK_SCHEMA_VERSION}",
                    self.schema_version
                ),
            ));
        }
        if self.host_identity.trim().is_empty() {
            return Err(lock_error(
                expected_result_path,
                "lock host identity is empty",
            ));
        }
        if self.owner_token.len() != 64
            || !self
                .owner_token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(lock_error(
                expected_result_path,
                "lock owner_token is not a 32-byte hexadecimal token",
            ));
        }
        if self.pid == 0 {
            return Err(lock_error(expected_result_path, "lock owner PID is zero"));
        }
        if self.process_start_token.trim().is_empty() {
            return Err(lock_error(
                expected_result_path,
                "lock process start token is empty",
            ));
        }
        if self.normalized_result_path != expected_result_path.display().to_string() {
            return Err(lock_error(
                expected_result_path,
                format!(
                    "lock result path '{}' does not match target",
                    self.normalized_result_path
                ),
            ));
        }
        Ok(())
    }
}

enum CreateLockError {
    AlreadyExists,
    Fatal(LabError),
}

enum RecoveryOutcome {
    Acquired(EnvResultLock),
    Retry(String),
}

struct ReclaimClaim {
    path: PathBuf,
    result_path: PathBuf,
    owner_token: String,
}

impl EnvResultLock {
    pub(super) fn acquire(result_path: &Path) -> LockResult<Self> {
        Self::acquire_with(result_path, &SystemLockEnvironment)
    }

    fn acquire_with(result_path: &Path, environment: &impl LockEnvironment) -> LockResult<Self> {
        let normalized_result_path = normalize_result_path(result_path)?;
        let lock_path = normalized_result_path.with_extension("json.lock");
        let proposed = EnvResultLockRecord::new(environment, &normalized_result_path)?;
        let mut last_retry_reason = "initial lock conflict".to_string();

        for attempt in 1..=MAX_LOCK_ACQUIRE_ATTEMPTS {
            match create_lock_file(&lock_path, &proposed, &normalized_result_path) {
                Ok(()) => {
                    return Ok(Self {
                        path: lock_path,
                        result_path: normalized_result_path,
                        owner_token: proposed.owner_token,
                    });
                }
                Err(CreateLockError::Fatal(error)) => return Err(error),
                Err(CreateLockError::AlreadyExists) => {}
            }

            let observed = read_lock_record_if_present(&lock_path, &normalized_result_path)
                .map_err(|error| {
                    lock_conflict(
                        &normalized_result_path,
                        None,
                        attempt,
                        format!(
                            "existing lock is unparseable or unverifiable: {}",
                            error.message
                        ),
                    )
                })?;
            let Some(observed) = observed else {
                last_retry_reason =
                    "lock disappeared before its owner record could be read".to_string();
                continue;
            };
            ensure_matching_host(
                &normalized_result_path,
                &observed,
                &proposed,
                attempt,
                "lock owner",
            )?;
            let stale_reason = match environment.inspect_process(observed.pid) {
                Ok(ProcessStatus::Missing) => "owner_pid_absent".to_string(),
                Ok(ProcessStatus::Alive { start_token })
                    if start_token != observed.process_start_token =>
                {
                    "owner_pid_reused_with_different_start_time".to_string()
                }
                Ok(ProcessStatus::Alive { .. }) => {
                    return Err(lock_conflict(
                        &normalized_result_path,
                        Some(&observed),
                        attempt,
                        "owner process is still active",
                    ));
                }
                Err(error) => {
                    return Err(lock_conflict(
                        &normalized_result_path,
                        Some(&observed),
                        attempt,
                        format!("owner liveness could not be proven: {error}"),
                    ));
                }
            };

            let claim = ReclaimClaim::acquire(
                &lock_path,
                &normalized_result_path,
                &observed,
                &proposed,
                attempt,
                environment,
            )?;
            let recovery = reclaim_under_claim(
                &lock_path,
                &normalized_result_path,
                &observed,
                &proposed,
                attempt,
                &stale_reason,
                environment,
            );
            match (recovery, claim.release()) {
                (Ok(RecoveryOutcome::Acquired(lock)), Ok(())) => return Ok(lock),
                (Ok(RecoveryOutcome::Retry(reason)), Ok(())) => {
                    last_retry_reason = reason;
                }
                (Err(error), Ok(())) => return Err(error),
                (Ok(RecoveryOutcome::Retry(_)), Err(claim_error)) => return Err(claim_error),
                (Ok(RecoveryOutcome::Acquired(lock)), Err(claim_error)) => {
                    return match lock.release() {
                        Ok(()) => Err(claim_error),
                        Err(release_error) => Err(combine_lock_errors(
                            &normalized_result_path,
                            &claim_error,
                            "newly acquired lock release also failed",
                            &release_error,
                        )),
                    };
                }
                (Err(error), Err(claim_error)) => {
                    return Err(combine_lock_errors(
                        &normalized_result_path,
                        &error,
                        "reclaim claim release also failed",
                        &claim_error,
                    ));
                }
            }
        }

        Err(reclaim_exhausted(
            &normalized_result_path,
            MAX_LOCK_ACQUIRE_ATTEMPTS,
            &last_retry_reason,
        ))
    }

    pub(super) fn release(self) -> LockResult<()> {
        let record = read_lock_record(&self.path, &self.result_path)?;
        if record.owner_token != self.owner_token {
            return Err(lock_error(
                &self.result_path,
                format!(
                    "lock release refused because owner token changed: expected {} actual {}",
                    self.owner_token, record.owner_token
                ),
            ));
        }
        fs::remove_file(&self.path).map_err(|error| {
            lock_error(
                &self.result_path,
                format!("failed to release {}: {error}", self.path.display()),
            )
        })
    }
}

impl ReclaimClaim {
    fn acquire(
        lock_path: &Path,
        result_path: &Path,
        observed: &EnvResultLockRecord,
        proposed: &EnvResultLockRecord,
        attempt: usize,
        environment: &impl LockEnvironment,
    ) -> LockResult<Self> {
        let path = reclaim_claim_path(lock_path, observed);
        // Owner proofs are independently deadline-bound; claim arbitration is bounded by attempts.
        for claim_attempt in 1..=MAX_RECLAIM_CLAIM_ATTEMPTS {
            match create_lock_file(&path, proposed, result_path) {
                Ok(()) => {
                    environment.after_reclaim_claim_persisted();
                    return Ok(Self {
                        path,
                        result_path: result_path.to_path_buf(),
                        owner_token: proposed.owner_token.clone(),
                    });
                }
                Err(CreateLockError::Fatal(error)) => return Err(error),
                Err(CreateLockError::AlreadyExists) => {}
            }

            let claimed = read_lock_record(&path, result_path).map_err(|error| {
                lock_conflict(
                    result_path,
                    None,
                    attempt,
                    format!(
                        "existing reclaim claim is unparseable or unverifiable: {}; claim_retry={claim_attempt}/{MAX_RECLAIM_CLAIM_ATTEMPTS}",
                        error.message
                    ),
                )
            })?;
            confirm_reclaim_claim_owner_is_stale(
                result_path,
                &claimed,
                proposed,
                attempt,
                claim_attempt,
                environment,
            )?;

            let confirmed = read_lock_record(&path, result_path).map_err(|error| {
                lock_conflict(
                    result_path,
                    Some(&claimed),
                    attempt,
                    format!(
                        "reclaim claim re-read failed: {}; claim_retry={claim_attempt}/{MAX_RECLAIM_CLAIM_ATTEMPTS}",
                        error.message
                    ),
                )
            })?;
            if confirmed.owner_token != claimed.owner_token {
                continue;
            }
            confirm_reclaim_claim_owner_is_stale(
                result_path,
                &confirmed,
                proposed,
                attempt,
                claim_attempt,
                environment,
            )?;

            let tombstone = reclaim_claim_tombstone_path(&path, &confirmed, proposed);
            match fs::rename(&path, &tombstone) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(lock_conflict(
                        result_path,
                        Some(&confirmed),
                        attempt,
                        format!(
                            "failed to atomically tombstone stale reclaim claim {}: {error}; claim_retry={claim_attempt}/{MAX_RECLAIM_CLAIM_ATTEMPTS}",
                            path.display()
                        ),
                    ));
                }
            }

            match create_lock_file(&path, proposed, result_path) {
                Ok(()) => {
                    let claim = Self {
                        path: path.clone(),
                        result_path: result_path.to_path_buf(),
                        owner_token: proposed.owner_token.clone(),
                    };
                    if let Err(cleanup_error) = remove_tombstone(&tombstone, result_path) {
                        return match claim.release() {
                            Ok(()) => Err(cleanup_error),
                            Err(release_error) => Err(combine_lock_errors(
                                result_path,
                                &cleanup_error,
                                "new reclaim claim release also failed",
                                &release_error,
                            )),
                        };
                    }
                    environment.after_reclaim_claim_persisted();
                    return Ok(claim);
                }
                Err(CreateLockError::AlreadyExists) => {
                    remove_tombstone(&tombstone, result_path)?;
                }
                Err(CreateLockError::Fatal(error)) => {
                    return match remove_tombstone(&tombstone, result_path) {
                        Ok(()) => Err(error),
                        Err(cleanup_error) => Err(combine_lock_errors(
                            result_path,
                            &error,
                            "stale reclaim claim tombstone cleanup also failed",
                            &cleanup_error,
                        )),
                    };
                }
            }
        }

        Err(lock_conflict(
            result_path,
            Some(observed),
            attempt,
            format!(
                "reclaim claim recovery exhausted; claim_retries={MAX_RECLAIM_CLAIM_ATTEMPTS}/{MAX_RECLAIM_CLAIM_ATTEMPTS}; escalation=fail_loud"
            ),
        ))
    }

    fn release(self) -> LockResult<()> {
        let record = read_lock_record(&self.path, &self.result_path)?;
        if record.owner_token != self.owner_token {
            return Err(lock_error(
                &self.result_path,
                format!(
                    "reclaim claim release refused because owner token changed: expected {} actual {}",
                    self.owner_token, record.owner_token
                ),
            ));
        }
        fs::remove_file(&self.path).map_err(|error| {
            lock_error(
                &self.result_path,
                format!(
                    "failed to release reclaim claim {}: {error}",
                    self.path.display()
                ),
            )
        })
    }
}

fn confirm_reclaim_claim_owner_is_stale(
    result_path: &Path,
    claim: &EnvResultLockRecord,
    proposed: &EnvResultLockRecord,
    attempt: usize,
    claim_attempt: usize,
    environment: &impl LockEnvironment,
) -> LockResult<()> {
    ensure_matching_host(result_path, claim, proposed, attempt, "reclaim claim owner")?;
    match environment.inspect_process(claim.pid) {
        Ok(ProcessStatus::Missing) => Ok(()),
        Ok(ProcessStatus::Alive { start_token }) if start_token != claim.process_start_token => {
            Ok(())
        }
        Ok(ProcessStatus::Alive { .. }) => Err(lock_conflict(
            result_path,
            Some(claim),
            attempt,
            format!(
                "reclaim claim owner process is still active; claim_retry={claim_attempt}/{MAX_RECLAIM_CLAIM_ATTEMPTS}"
            ),
        )),
        Err(error) => Err(lock_conflict(
            result_path,
            Some(claim),
            attempt,
            format!(
                "reclaim claim owner liveness could not be proven: {error}; claim_retry={claim_attempt}/{MAX_RECLAIM_CLAIM_ATTEMPTS}"
            ),
        )),
    }
}

fn reclaim_under_claim(
    lock_path: &Path,
    result_path: &Path,
    observed: &EnvResultLockRecord,
    proposed: &EnvResultLockRecord,
    attempt: usize,
    stale_reason: &str,
    environment: &impl LockEnvironment,
) -> LockResult<RecoveryOutcome> {
    let confirmed = read_lock_record(lock_path, result_path).map_err(|error| {
        lock_conflict(
            result_path,
            Some(observed),
            attempt,
            format!("stale owner re-read failed: {}", error.message),
        )
    })?;
    ensure_matching_host(
        result_path,
        &confirmed,
        proposed,
        attempt,
        "lock owner recheck",
    )?;
    if confirmed.owner_token != observed.owner_token {
        return Ok(RecoveryOutcome::Retry(format!(
            "owner token changed during stale confirmation: {} -> {}; retry={attempt}/{MAX_LOCK_ACQUIRE_ATTEMPTS}",
            observed.owner_token, confirmed.owner_token
        )));
    }
    match environment.inspect_process(confirmed.pid) {
        Ok(ProcessStatus::Missing) => {}
        Ok(ProcessStatus::Alive { start_token })
            if start_token != confirmed.process_start_token => {}
        Ok(ProcessStatus::Alive { .. }) => {
            return Err(lock_conflict(
                result_path,
                Some(&confirmed),
                attempt,
                "owner process became active during stale confirmation",
            ));
        }
        Err(error) => {
            return Err(lock_conflict(
                result_path,
                Some(&confirmed),
                attempt,
                format!("owner liveness recheck could not be proven: {error}"),
            ));
        }
    }

    let tombstone = tombstone_path(lock_path, observed, proposed);
    match fs::rename(lock_path, &tombstone) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RecoveryOutcome::Retry(format!(
                "stale rename race after {stale_reason}; original_error=already_exists; retry={attempt}/{MAX_LOCK_ACQUIRE_ATTEMPTS}"
            )));
        }
        Err(error) => {
            return Err(lock_conflict(
                result_path,
                Some(observed),
                attempt,
                format!("failed to atomically tombstone stale lock after {stale_reason}: {error}"),
            ));
        }
    }
    match create_lock_file(lock_path, proposed, result_path) {
        Ok(()) => {
            let acquired = EnvResultLock {
                path: lock_path.to_path_buf(),
                result_path: result_path.to_path_buf(),
                owner_token: proposed.owner_token.clone(),
            };
            if let Err(cleanup_error) = remove_tombstone(&tombstone, result_path) {
                return match acquired.release() {
                    Ok(()) => Err(cleanup_error),
                    Err(release_error) => Err(combine_lock_errors(
                        result_path,
                        &cleanup_error,
                        "newly acquired lock release also failed",
                        &release_error,
                    )),
                };
            }
            Ok(RecoveryOutcome::Acquired(acquired))
        }
        Err(CreateLockError::AlreadyExists) => {
            remove_tombstone(&tombstone, result_path)?;
            Ok(RecoveryOutcome::Retry(format!(
                "another writer won after {stale_reason}; original_error=already_exists; retry={attempt}/{MAX_LOCK_ACQUIRE_ATTEMPTS}"
            )))
        }
        Err(CreateLockError::Fatal(error)) => match remove_tombstone(&tombstone, result_path) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(combine_lock_errors(
                result_path,
                &error,
                "stale tombstone cleanup also failed",
                &cleanup_error,
            )),
        },
    }
}

fn combine_lock_errors(
    result_path: &Path,
    primary: &LabError,
    cleanup_context: &str,
    cleanup: &LabError,
) -> LabError {
    lock_error(
        result_path,
        format!(
            "{}; {cleanup_context}: {}",
            primary.message, cleanup.message
        ),
    )
}

fn ensure_matching_host(
    result_path: &Path,
    observed: &EnvResultLockRecord,
    proposed: &EnvResultLockRecord,
    attempt: usize,
    context: &str,
) -> LockResult<()> {
    if observed.host_identity == proposed.host_identity {
        return Ok(());
    }
    Err(lock_conflict(
        result_path,
        Some(observed),
        attempt,
        format!(
            "{context} host identity mismatch: owner_host={} current_host={}; cross-host reclaim is forbidden",
            observed.host_identity, proposed.host_identity
        ),
    ))
}

fn normalize_result_path(result_path: &Path) -> LockResult<PathBuf> {
    let file_name = result_path.file_name().ok_or_else(|| {
        lock_error(
            result_path,
            "result path has no file name for lock ownership",
        )
    })?;
    let parent = result_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        lock_error(
            result_path,
            format!("failed to create lock parent {}: {error}", parent.display()),
        )
    })?;
    let normalized_parent = fs::canonicalize(parent).map_err(|error| {
        lock_error(
            result_path,
            format!(
                "failed to normalize lock parent {}: {error}",
                parent.display()
            ),
        )
    })?;
    Ok(normalized_parent.join(file_name))
}

fn create_lock_file(
    lock_path: &Path,
    record: &EnvResultLockRecord,
    result_path: &Path,
) -> Result<(), CreateLockError> {
    match fs::symlink_metadata(lock_path) {
        Ok(_) => return Err(CreateLockError::AlreadyExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CreateLockError::Fatal(lock_error(
                result_path,
                format!(
                    "failed to inspect lock target {}: {error}",
                    lock_path.display()
                ),
            )));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(record).map_err(|error| {
        CreateLockError::Fatal(lock_error(
            result_path,
            format!("failed to serialize lock record: {error}"),
        ))
    })?;
    bytes.push(b'\n');
    let pending_path = atomic_record_pending_path(lock_path, &record.owner_token, result_path)
        .map_err(CreateLockError::Fatal)?;
    match publish_new_synced_record(lock_path, &pending_path, &bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(CreateLockError::AlreadyExists)
        }
        Err(error) => Err(CreateLockError::Fatal(lock_error(
            result_path,
            format!("failed to publish lock {}: {error}", lock_path.display()),
        ))),
    }
}

fn atomic_record_pending_path(
    target_path: &Path,
    owner_token: &str,
    result_path: &Path,
) -> LockResult<PathBuf> {
    let file_name = target_path.file_name().ok_or_else(|| {
        lock_error(
            result_path,
            format!(
                "atomic lock target has no file name: {}",
                target_path.display()
            ),
        )
    })?;
    let mut pending_name = file_name.to_os_string();
    pending_name.push(format!(".pending.{owner_token}"));
    Ok(target_path.with_file_name(pending_name))
}

fn publish_new_synced_record(
    target_path: &Path,
    pending_path: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    write_synced_pending_record(pending_path, bytes)?;
    publish_synced_pending_record(target_path, pending_path)
}

fn publish_synced_pending_record(target_path: &Path, pending_path: &Path) -> io::Result<()> {
    match fs::hard_link(pending_path, target_path) {
        Ok(()) => {}
        Err(error) => return Err(cleanup_pending_after_error(pending_path, error)),
    }
    if let Err(error) = fs::remove_file(pending_path) {
        let cleanup_error = io::Error::other(format!(
            "failed to remove atomic lock alias {} after publishing {}: {error}",
            pending_path.display(),
            target_path.display()
        ));
        return Err(rollback_published_record(target_path, cleanup_error));
    }
    Ok(())
}

fn write_synced_pending_record(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let write = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    match write {
        Ok(()) => Ok(()),
        Err(error) => Err(cleanup_pending_after_error(path, error)),
    }
}

fn cleanup_pending_after_error(path: &Path, error: io::Error) -> io::Error {
    match fs::remove_file(path) {
        Ok(()) => error,
        Err(cleanup_error) if cleanup_error.kind() == io::ErrorKind::NotFound => error,
        Err(cleanup_error) => io::Error::other(format!(
            "{error}; atomic lock cleanup failed for {}: {cleanup_error}",
            path.display()
        )),
    }
}

fn rollback_published_record(path: &Path, error: io::Error) -> io::Error {
    match fs::remove_file(path) {
        Ok(()) => error,
        Err(rollback_error) if rollback_error.kind() == io::ErrorKind::NotFound => error,
        Err(rollback_error) => io::Error::other(format!(
            "{error}; atomic lock rollback failed for {}: {rollback_error}",
            path.display()
        )),
    }
}

fn read_lock_record(lock_path: &Path, result_path: &Path) -> LockResult<EnvResultLockRecord> {
    read_lock_record_if_present(lock_path, result_path)?.ok_or_else(|| {
        lock_error(
            result_path,
            format!("lock does not exist: {}", lock_path.display()),
        )
    })
}

fn read_lock_record_if_present(
    lock_path: &Path,
    result_path: &Path,
) -> LockResult<Option<EnvResultLockRecord>> {
    let started = Instant::now();
    loop {
        let bytes = match fs::read(lock_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if transient_lock_io(&error) => {
                retry_incomplete_lock_read(started, lock_path, result_path, error.to_string())?;
                continue;
            }
            Err(error) => {
                return Err(lock_error(
                    result_path,
                    format!("failed to read lock {}: {error}", lock_path.display()),
                ));
            }
        };
        let record: EnvResultLockRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) if bytes.is_empty() || error.is_eof() => {
                retry_incomplete_lock_read(started, lock_path, result_path, error.to_string())?;
                continue;
            }
            Err(error) => {
                return Err(lock_error(
                    result_path,
                    format!("failed to parse lock {}: {error}", lock_path.display()),
                ));
            }
        };
        record.validate(result_path)?;
        return Ok(Some(record));
    }
}

fn transient_lock_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock
    )
}

fn retry_incomplete_lock_read(
    started: Instant,
    lock_path: &Path,
    result_path: &Path,
    reason: String,
) -> LockResult<()> {
    let remaining = RECORD_READ_TIMEOUT.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(lock_error(
            result_path,
            format!(
                "lock did not stabilize within {}ms; lock={}; escalation=fail_loud; last_error={reason}",
                RECORD_READ_TIMEOUT.as_millis(),
                lock_path.display()
            ),
        ));
    }
    std::thread::sleep(RECORD_READ_DELAY.min(remaining));
    Ok(())
}

fn tombstone_path(
    lock_path: &Path,
    observed: &EnvResultLockRecord,
    proposed: &EnvResultLockRecord,
) -> PathBuf {
    let file_name = lock_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("result.json.lock");
    lock_path.with_file_name(format!(
        "{file_name}.stale.{}.{}",
        observed.owner_token, proposed.owner_token
    ))
}

fn reclaim_claim_path(lock_path: &Path, observed: &EnvResultLockRecord) -> PathBuf {
    let file_name = lock_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("result.json.lock");
    lock_path.with_file_name(format!("{file_name}.reclaim.{}", observed.owner_token))
}

fn reclaim_claim_tombstone_path(
    claim_path: &Path,
    observed: &EnvResultLockRecord,
    proposed: &EnvResultLockRecord,
) -> PathBuf {
    let file_name = claim_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("result.json.lock.reclaim");
    claim_path.with_file_name(format!(
        "{file_name}.stale.{}.{}",
        observed.owner_token, proposed.owner_token
    ))
}

fn remove_tombstone(tombstone: &Path, result_path: &Path) -> LockResult<()> {
    fs::remove_file(tombstone).map_err(|error| {
        lock_error(
            result_path,
            format!(
                "failed to clean stale lock tombstone {}: {error}",
                tombstone.display()
            ),
        )
    })
}

fn lock_error(result_path: &Path, reason: impl AsRef<str>) -> LabError {
    LabError::usage(format!(
        "env detection result lock failure; target={}; reason={}",
        result_path.display(),
        reason.as_ref()
    ))
}

fn lock_conflict(
    result_path: &Path,
    owner: Option<&EnvResultLockRecord>,
    attempt: usize,
    reason: impl AsRef<str>,
) -> LabError {
    let owner_context = owner.map_or_else(
        || "owner=unavailable".to_string(),
        |record| {
            format!(
                "owner_host={} owner_pid={} owner_start={} owner_token={}",
                record.host_identity, record.pid, record.process_start_token, record.owner_token
            )
        },
    );
    LabError::usage(format!(
        "env detection result lock conflict; target={}; {owner_context}; original_error=already_exists; retry={attempt}/{MAX_LOCK_ACQUIRE_ATTEMPTS}; escalation=fail_loud; reason={}",
        result_path.display(),
        reason.as_ref()
    ))
}

fn reclaim_exhausted(result_path: &Path, attempts: usize, reason: &str) -> LabError {
    LabError::usage(format!(
        "env detection result lock recovery exhausted; target={}; original_error=already_exists; retries={attempts}/{MAX_LOCK_ACQUIRE_ATTEMPTS}; reclaim_claim_retries={MAX_RECLAIM_CLAIM_ATTEMPTS}; escalation=fail_loud; last_reason={reason}",
        result_path.display()
    ))
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct WindowsProbeOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(target_os = "windows")]
fn run_windows_probe(
    program: &str,
    args: &[&str],
    timeout: Duration,
    purpose: &str,
) -> Result<WindowsProbeOutput, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start {purpose}: {error}"))?;
    let Some(stdout) = child.stdout.take() else {
        let cleanup = stop_windows_probe(&mut child, WINDOWS_PROBE_SHUTDOWN_TIMEOUT);
        return Err(format!(
            "failed to open {purpose} stdout; cleanup={cleanup:?}"
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        let cleanup = stop_windows_probe(&mut child, WINDOWS_PROBE_SHUTDOWN_TIMEOUT);
        return Err(format!(
            "failed to open {purpose} stderr; cleanup={cleanup:?}"
        ));
    };
    let stdout_reader = spawn_windows_probe_reader(stdout);
    let stderr_reader = spawn_windows_probe_reader(stderr);
    let started = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return collect_windows_probe_output(status, stdout_reader, stderr_reader, purpose);
            }
            Ok(None) if started.elapsed() < timeout => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(WINDOWS_PROBE_POLL_DELAY.min(remaining));
            }
            Ok(None) => {
                let status = stop_windows_probe(&mut child, WINDOWS_PROBE_SHUTDOWN_TIMEOUT)
                    .map_err(|cleanup_error| {
                        format!(
                            "{purpose} timed out after {}ms; probe cleanup failed: {cleanup_error}",
                            timeout.as_millis()
                        )
                    })?;
                let output = collect_windows_probe_output(
                    status,
                    stdout_reader,
                    stderr_reader,
                    purpose,
                )
                .map_err(|output_error| {
                    format!(
                        "{purpose} timed out after {}ms and was terminated; output collection failed: {output_error}",
                        timeout.as_millis()
                    )
                })?;
                return Err(format!(
                    "{purpose} timed out after {}ms and was terminated; status={}; stdout_bytes={}; stderr={}",
                    timeout.as_millis(),
                    output.status,
                    output.stdout.len(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Err(error) => {
                let cleanup = stop_windows_probe(&mut child, WINDOWS_PROBE_SHUTDOWN_TIMEOUT);
                return Err(format!(
                    "failed to poll {purpose}: {error}; cleanup={cleanup:?}"
                ));
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn spawn_windows_probe_reader(
    mut reader: impl Read + Send + 'static,
) -> JoinHandle<io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

#[cfg(target_os = "windows")]
fn collect_windows_probe_output(
    status: ExitStatus,
    stdout_reader: JoinHandle<io::Result<Vec<u8>>>,
    stderr_reader: JoinHandle<io::Result<Vec<u8>>>,
    purpose: &str,
) -> Result<WindowsProbeOutput, String> {
    Ok(WindowsProbeOutput {
        status,
        stdout: join_windows_probe_reader(stdout_reader, purpose, "stdout")?,
        stderr: join_windows_probe_reader(stderr_reader, purpose, "stderr")?,
    })
}

#[cfg(target_os = "windows")]
fn join_windows_probe_reader(
    reader: JoinHandle<io::Result<Vec<u8>>>,
    purpose: &str,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("{purpose} {stream} reader panicked"))?
        .map_err(|error| format!("failed to read {purpose} {stream}: {error}"))
}

#[cfg(target_os = "windows")]
fn stop_windows_probe(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    if let Some(status) = child
        .try_wait()
        .map_err(|error| format!("failed to poll probe before termination: {error}"))?
    {
        return Ok(status);
    }
    if let Err(error) = child.kill() {
        if let Some(status) = child
            .try_wait()
            .map_err(|poll_error| format!("{error}; follow-up poll failed: {poll_error}"))?
        {
            return Ok(status);
        }
        return Err(format!("failed to terminate probe: {error}"));
    }
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to poll terminated probe: {error}"))?
        {
            return Ok(status);
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(format!(
                "probe did not exit within {}ms after termination",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(WINDOWS_PROBE_POLL_DELAY.min(remaining));
    }
}

#[cfg(target_os = "windows")]
fn inspect_system_process(pid: u32) -> Result<ProcessStatus, String> {
    let script = format!(
        "$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; if ($null -eq $p) {{ exit 3 }}; try {{ [Console]::Out.Write($p.StartTime.ToUniversalTime().Ticks.ToString([Globalization.CultureInfo]::InvariantCulture)); exit 0 }} catch {{ [Console]::Error.Write($_.Exception.Message); exit 4 }}"
    );
    let output = run_windows_probe(
        "powershell.exe",
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ],
        WINDOWS_PROBE_TIMEOUT,
        &format!("PowerShell process inspector for PID {pid}"),
    )?;
    if output.status.success() {
        let start_token = String::from_utf8(output.stdout)
            .map_err(|error| format!("process inspector returned non-UTF-8 output: {error}"))?;
        let start_token = start_token.trim().to_string();
        if start_token.is_empty() {
            return Err("process inspector returned an empty start time".to_string());
        }
        return Ok(ProcessStatus::Alive { start_token });
    }
    if output.status.code() == Some(3) {
        return Ok(ProcessStatus::Missing);
    }
    Err(format!(
        "process inspector failed for PID {pid}: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

#[cfg(target_os = "linux")]
fn inspect_system_process(pid: u32) -> Result<ProcessStatus, String> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ProcessStatus::Missing),
        Err(error) => {
            return Err(format!(
                "failed to inspect process stat {}: {error}",
                path.display()
            ));
        }
    };
    let command_end = stat
        .rfind(')')
        .ok_or_else(|| format!("process stat {} has no command terminator", path.display()))?;
    let fields = stat[command_end + 1..]
        .split_whitespace()
        .collect::<Vec<_>>();
    let start_token = fields
        .get(19)
        .ok_or_else(|| format!("process stat {} has no start time", path.display()))?;
    Ok(ProcessStatus::Alive {
        start_token: (*start_token).to_string(),
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn inspect_system_process(pid: u32) -> Result<ProcessStatus, String> {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| format!("failed to start process inspector: {error}"))?;
    if output.status.success() {
        let start_token = String::from_utf8(output.stdout)
            .map_err(|error| format!("process inspector returned non-UTF-8 output: {error}"))?;
        let start_token = start_token.trim().to_string();
        if start_token.is_empty() {
            return Ok(ProcessStatus::Missing);
        }
        return Ok(ProcessStatus::Alive { start_token });
    }
    if output.stderr.is_empty() {
        return Ok(ProcessStatus::Missing);
    }
    Err(format!(
        "process inspector failed for PID {pid}: status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

#[cfg(not(any(windows, unix)))]
fn inspect_system_process(_pid: u32) -> Result<ProcessStatus, String> {
    Err("process identity inspection is unsupported on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn system_host_identity() -> Result<String, String> {
    let output = run_windows_probe("hostname.exe", &[], WINDOWS_PROBE_TIMEOUT, "host inspector")?;
    if !output.status.success() {
        return Err(format!(
            "host inspector failed: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    normalize_host_identity(
        String::from_utf8(output.stdout)
            .map_err(|error| format!("host inspector returned non-UTF-8 output: {error}"))?
            .as_str(),
    )
}

#[cfg(target_os = "linux")]
fn system_host_identity() -> Result<String, String> {
    let hostname = fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|error| format!("failed to read kernel hostname: {error}"))?;
    normalize_host_identity(&hostname)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn system_host_identity() -> Result<String, String> {
    let output = Command::new("hostname")
        .output()
        .map_err(|error| format!("failed to start host inspector: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "host inspector failed: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    normalize_host_identity(
        String::from_utf8(output.stdout)
            .map_err(|error| format!("host inspector returned non-UTF-8 output: {error}"))?
            .as_str(),
    )
}

#[cfg(not(any(windows, unix)))]
fn system_host_identity() -> Result<String, String> {
    Err("host identity inspection is unsupported on this platform".to_string())
}

fn normalize_host_identity(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("host inspector returned an empty hostname".to_string());
    }
    Ok(normalized)
}

#[cfg(target_os = "windows")]
fn system_random_seed() -> Result<[u8; 32], String> {
    let script = "$bytes=New-Object byte[] 32; $rng=[Security.Cryptography.RandomNumberGenerator]::Create(); try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }; [Console]::Out.Write((($bytes | ForEach-Object { $_.ToString('x2') }) -join ''))";
    let output = run_windows_probe(
        "powershell.exe",
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ],
        WINDOWS_PROBE_TIMEOUT,
        "PowerShell random source",
    )?;
    if !output.status.success() {
        return Err(format!(
            "PowerShell random source failed: status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_seed_hex(
        String::from_utf8(output.stdout)
            .map_err(|error| format!("random source returned non-UTF-8 output: {error}"))?
            .trim(),
    )
}

#[cfg(unix)]
fn system_random_seed() -> Result<[u8; 32], String> {
    let mut seed = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut seed))
        .map_err(|error| format!("failed to read /dev/urandom: {error}"))?;
    Ok(seed)
}

#[cfg(not(any(windows, unix)))]
fn system_random_seed() -> Result<[u8; 32], String> {
    Err("secure random source is unsupported on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn parse_seed_hex(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("random source did not return 32 hexadecimal bytes".to_string());
    }
    let mut seed = [0u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|error| format!("failed to parse random byte: {error}"))?;
    }
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::process::{Child, Stdio};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Barrier, Mutex};
    use tempfile::TempDir;

    struct FakeEnvironment {
        host_identity: String,
        current: ProcessIdentity,
        statuses: Mutex<BTreeMap<u32, Result<ProcessStatus, String>>>,
        token_sequence: AtomicU64,
        terminate_after_claim: bool,
    }

    impl FakeEnvironment {
        fn new() -> Self {
            Self {
                host_identity: "test-host".to_string(),
                current: ProcessIdentity {
                    pid: 42,
                    start_token: "current-start".to_string(),
                },
                statuses: Mutex::new(BTreeMap::new()),
                token_sequence: AtomicU64::new(1),
                terminate_after_claim: false,
            }
        }

        fn terminating_reclaimer(pid: u32, start_token: &str) -> Self {
            Self {
                host_identity: "test-host".to_string(),
                current: ProcessIdentity {
                    pid,
                    start_token: start_token.to_string(),
                },
                statuses: Mutex::new(BTreeMap::new()),
                token_sequence: AtomicU64::new(1),
                terminate_after_claim: true,
            }
        }

        fn set_status(&self, pid: u32, status: Result<ProcessStatus, String>) {
            self.statuses.lock().unwrap().insert(pid, status);
        }

        fn with_host_identity(mut self, host_identity: &str) -> Self {
            self.host_identity = host_identity.to_string();
            self
        }
    }

    impl LockEnvironment for FakeEnvironment {
        fn host_identity(&self) -> Result<String, String> {
            Ok(self.host_identity.clone())
        }

        fn current_process(&self) -> Result<ProcessIdentity, String> {
            Ok(self.current.clone())
        }

        fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String> {
            if let Some(status) = self.statuses.lock().unwrap().get(&pid) {
                return status.clone();
            }
            if pid == self.current.pid {
                return Ok(ProcessStatus::Alive {
                    start_token: self.current.start_token.clone(),
                });
            }
            Ok(ProcessStatus::Missing)
        }

        fn next_owner_token(&self) -> Result<String, String> {
            Ok(format!(
                "{:064x}",
                self.token_sequence.fetch_add(1, Ordering::Relaxed)
            ))
        }

        fn now_unix_ms(&self) -> Result<u64, String> {
            Ok(1_750_000_000_000)
        }

        fn after_reclaim_claim_persisted(&self) {
            if self.terminate_after_claim {
                panic!("fault injection: reclaimer terminated after claim persistence");
            }
        }
    }

    struct DelayedOwnerProofEnvironment {
        inner: FakeEnvironment,
        delayed_pid: u32,
        delay: Duration,
        delayed: AtomicBool,
    }

    impl DelayedOwnerProofEnvironment {
        fn new(delayed_pid: u32, delay: Duration) -> Self {
            Self {
                inner: FakeEnvironment::new(),
                delayed_pid,
                delay,
                delayed: AtomicBool::new(false),
            }
        }
    }

    impl LockEnvironment for DelayedOwnerProofEnvironment {
        fn host_identity(&self) -> Result<String, String> {
            self.inner.host_identity()
        }

        fn current_process(&self) -> Result<ProcessIdentity, String> {
            self.inner.current_process()
        }

        fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String> {
            if pid == self.delayed_pid && !self.delayed.swap(true, Ordering::AcqRel) {
                std::thread::sleep(self.delay);
            }
            self.inner.inspect_process(pid)
        }

        fn next_owner_token(&self) -> Result<String, String> {
            self.inner.next_owner_token()
        }

        fn now_unix_ms(&self) -> Result<u64, String> {
            self.inner.now_unix_ms()
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_probe_timeout_terminates_helper() {
        let started = Instant::now();
        let error = run_windows_probe(
            "powershell.exe",
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 60",
            ],
            Duration::from_millis(100),
            "timeout test probe",
        )
        .expect_err("probe must be terminated at its deadline");
        assert!(error.contains("timed out after 100ms"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "bounded probe cleanup exceeded three seconds"
        );
    }

    #[test]
    fn atomic_lock_publication_hides_partial_bytes_and_preserves_collision() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let normalized = normalize_result_path(&result).unwrap();
        let target = normalized.with_extension("json.lock");
        let environment = FakeEnvironment::new();
        let record = EnvResultLockRecord::new(&environment, &normalized).unwrap();
        let mut complete = serde_json::to_vec(&record).unwrap();
        complete.push(b'\n');
        let pending =
            atomic_record_pending_path(&target, &record.owner_token, &normalized).unwrap();

        write_synced_pending_record(&pending, &complete).unwrap();
        assert!(!target.exists());
        assert_eq!(fs::read(&pending).unwrap(), complete);
        publish_synced_pending_record(&target, &pending).unwrap();
        assert_eq!(fs::read(&target).unwrap(), complete);
        assert!(!pending.exists());

        let contender_record = EnvResultLockRecord::new(&environment, &normalized).unwrap();
        let contender =
            atomic_record_pending_path(&target, &contender_record.owner_token, &normalized)
                .unwrap();
        let mut replacement = serde_json::to_vec(&contender_record).unwrap();
        replacement.push(b'\n');
        write_synced_pending_record(&contender, &replacement).unwrap();
        let error = publish_synced_pending_record(&target, &contender)
            .expect_err("atomic lock publication must not replace an existing owner");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target).unwrap(), complete);
        assert!(!contender.exists());
    }

    #[test]
    fn normal_release_removes_owned_lock() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();

        let lock = EnvResultLock::acquire_with(&result, &environment).unwrap();
        let lock_path = normalize_result_path(&result)
            .unwrap()
            .with_extension("json.lock");
        assert!(lock_path.exists());
        let record =
            read_lock_record(&lock_path, &normalize_result_path(&result).unwrap()).unwrap();
        assert_eq!(record.schema_version, LOCK_SCHEMA_VERSION);
        assert_eq!(record.host_identity, environment.host_identity);
        let pending = atomic_record_pending_path(&lock_path, &record.owner_token, &result).unwrap();
        assert!(!pending.exists());
        lock.release().unwrap();
        assert!(!lock_path.exists());
    }

    #[test]
    fn old_acquisition_time_does_not_override_a_live_owner() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        write_foreign_record(
            &result,
            environment.current.pid,
            &environment.current.start_token,
            &format!("{:064x}", 899),
        );

        let error = EnvResultLock::acquire_with(&result, &environment)
            .expect_err("age alone must not make a live lock stale");
        assert!(error.message.contains("owner process is still active"));
        assert!(
            normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock")
                .exists()
        );
    }

    #[test]
    fn release_refuses_a_changed_owner_token() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        let lock = EnvResultLock::acquire_with(&result, &environment).unwrap();
        write_foreign_record(
            &result,
            environment.current.pid,
            &environment.current.start_token,
            &format!("{:064x}", 898),
        );

        let error = lock
            .release()
            .expect_err("release must verify its owner token");
        assert!(error.message.contains("owner token changed"));
        assert!(
            normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock")
                .exists()
        );
    }

    #[test]
    fn pid_reuse_is_recovered_only_after_start_time_mismatch() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        write_foreign_record(
            &result,
            environment.current.pid,
            "old-start",
            &format!("{:064x}", 900),
        );

        let lock = EnvResultLock::acquire_with(&result, &environment).unwrap();
        lock.release().unwrap();
    }

    #[test]
    fn corrupt_lock_record_is_not_deleted() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let lock_path = normalize_result_path(&result)
            .unwrap()
            .with_extension("json.lock");
        fs::write(&lock_path, b"pid=42\n").unwrap();

        let error = EnvResultLock::acquire_with(&result, &FakeEnvironment::new())
            .expect_err("corrupt lock must be refused");
        assert!(error.message.contains("unparseable or unverifiable"));
        assert!(!error.message.contains("did not stabilize"));
        assert!(lock_path.exists());
    }

    #[test]
    fn empty_host_identity_is_rejected_before_lock_creation() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new().with_host_identity("  ");

        let error = EnvResultLock::acquire_with(&result, &environment)
            .expect_err("empty host identity must be rejected");
        assert!(error.message.contains("lock host identity is empty"));
        assert!(!result.with_extension("json.lock").exists());
    }

    #[test]
    fn old_lock_schema_is_preserved_and_rejected() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let mut record =
            write_foreign_record(&result, 77, "foreign-start", &format!("{:064x}", 910));
        record.schema_version = "env-result-lock.v1".to_string();
        let lock_path = normalize_result_path(&result)
            .unwrap()
            .with_extension("json.lock");
        fs::write(&lock_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

        let error = EnvResultLock::acquire_with(&result, &FakeEnvironment::new())
            .expect_err("old schema without the current contract must fail closed");
        assert!(error.message.contains("unsupported lock schema"));
        assert!(lock_path.exists());
    }

    #[test]
    fn foreign_host_lock_is_never_reclaimed() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        environment.set_status(77, Err("foreign PID must not be inspected".to_string()));
        write_foreign_record_on_host(
            &result,
            "other-host",
            77,
            "foreign-start",
            &format!("{:064x}", 911),
        );
        let lock_path = normalize_result_path(&result)
            .unwrap()
            .with_extension("json.lock");

        let error = EnvResultLock::acquire_with(&result, &environment)
            .expect_err("foreign host lock must fail before stale recovery");
        assert!(error.message.contains("host identity mismatch"));
        assert!(error.message.contains("cross-host reclaim is forbidden"));
        assert!(lock_path.exists());
        assert_no_recovery_artifacts(&result);
    }

    #[test]
    fn unconfirmed_owner_is_not_deleted() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        environment.set_status(77, Err("access denied".to_string()));
        write_foreign_record(&result, 77, "foreign-start", &format!("{:064x}", 901));

        let error = EnvResultLock::acquire_with(&result, &environment)
            .expect_err("unverifiable owner must be refused");
        assert!(error.message.contains("liveness could not be proven"));
        assert!(
            normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock")
                .exists()
        );
    }

    #[test]
    fn two_reclaimers_have_one_winner_for_fifty_rounds() {
        let temp = TempDir::new().unwrap();
        let environment = Arc::new(FakeEnvironment::new());
        for round in 0..50 {
            let result = temp.path().join(format!("envinst_{round}/result.json"));
            write_foreign_record(
                &result,
                77,
                "foreign-start",
                &format!("{:064x}", 902 + round),
            );
            let barrier = Arc::new(Barrier::new(3));
            let mut handles = Vec::new();
            for _ in 0..2 {
                let result = result.clone();
                let environment = Arc::clone(&environment);
                let barrier = Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    barrier.wait();
                    EnvResultLock::acquire_with(&result, environment.as_ref())
                }));
            }
            barrier.wait();
            let outcomes = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>();
            assert!(
                outcomes.iter().filter(|outcome| outcome.is_ok()).count() == 1,
                "round={round} outcomes={outcomes:?}"
            );
            assert!(
                outcomes.iter().filter(|outcome| outcome.is_err()).count() == 1,
                "round={round} outcomes={outcomes:?}"
            );
            for error in outcomes.iter().filter_map(|outcome| outcome.as_ref().err()) {
                assert!(
                    error.message.contains("lock conflict")
                        || error.message.contains("recovery exhausted"),
                    "round={round} error={}",
                    error.message
                );
            }
            outcomes
                .into_iter()
                .find_map(Result::ok)
                .unwrap()
                .release()
                .unwrap();
            assert_no_recovery_artifacts(&result);
        }
    }

    #[test]
    fn owner_proof_does_not_consume_reclaim_claim_budget() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let stale_pid = 77;
        write_foreign_record(
            &result,
            stale_pid,
            "former-owner-start",
            &format!("{:064x}", 914),
        );
        let proof_delay = Duration::from_millis(2_100);
        let environment = DelayedOwnerProofEnvironment::new(stale_pid, proof_delay);
        let started = Instant::now();

        let lock = EnvResultLock::acquire_with(&result, &environment)
            .expect("claim phase must receive a fresh budget after stale owner proof");
        assert!(started.elapsed() >= proof_delay);
        lock.release().unwrap();
        assert_no_recovery_artifacts(&result);
    }

    #[test]
    fn killed_process_lock_is_recovered() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = SystemLockEnvironment;
        let mut child = sleeping_child();
        let status = wait_for_child_identity(&environment, child.id());
        let ProcessStatus::Alive { start_token } = status else {
            panic!("sleeping child disappeared before lock fixture setup");
        };
        write_foreign_record_on_host(
            &result,
            &environment.host_identity().unwrap(),
            child.id(),
            &start_token,
            &format!("{:064x}", 903),
        );
        child.kill().unwrap();
        child.wait().unwrap();

        let lock = EnvResultLock::acquire_with(&result, &environment).unwrap();
        lock.release().unwrap();
        assert_no_recovery_artifacts(&result);
    }

    #[test]
    fn unconfirmed_reclaim_claim_owner_is_preserved() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        environment.set_status(88, Err("access denied".to_string()));
        let observed = write_foreign_record(&result, 77, "foreign-start", &format!("{:064x}", 904));
        let claim_path = reclaim_claim_path(
            &normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock"),
            &observed,
        );
        write_record_at(
            &claim_path,
            &result,
            88,
            "claim-start",
            &format!("{:064x}", 905),
        );

        let error = EnvResultLock::acquire_with(&result, &environment)
            .expect_err("unverifiable reclaim claim owner must be preserved");
        assert!(
            error
                .message
                .contains("reclaim claim owner liveness could not be proven")
        );
        assert!(claim_path.exists());
        assert!(
            normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock")
                .exists()
        );
    }

    #[test]
    fn foreign_host_reclaim_claim_is_never_reclaimed() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let environment = FakeEnvironment::new();
        environment.set_status(88, Err("foreign PID must not be inspected".to_string()));
        let observed = write_foreign_record(&result, 77, "foreign-start", &format!("{:064x}", 912));
        let claim_path = reclaim_claim_path(
            &normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock"),
            &observed,
        );
        write_record_at_on_host(
            &claim_path,
            &result,
            "other-host",
            88,
            "claim-start",
            &format!("{:064x}", 913),
        );

        let error = EnvResultLock::acquire_with(&result, &environment)
            .expect_err("foreign host reclaim claim must fail before stale recovery");
        assert!(error.message.contains("host identity mismatch"));
        assert!(claim_path.exists());
        assert!(
            normalize_result_path(&result)
                .unwrap()
                .with_extension("json.lock")
                .exists()
        );
    }

    #[test]
    fn terminated_reclaimer_claim_is_recovered_without_manual_cleanup() {
        let temp = TempDir::new().unwrap();
        let result = temp.path().join("envinst_a/result.json");
        let observed =
            write_foreign_record(&result, 77, "former-owner-start", &format!("{:064x}", 906));
        let crashing_result = result.clone();
        let crashed = std::thread::spawn(move || {
            let environment = FakeEnvironment::terminating_reclaimer(88, "reclaimer-start");
            EnvResultLock::acquire_with(&crashing_result, &environment)
        })
        .join();
        assert!(crashed.is_err(), "fault injection must terminate reclaimer");

        let lock_path = normalize_result_path(&result)
            .unwrap()
            .with_extension("json.lock");
        let claim_path = reclaim_claim_path(&lock_path, &observed);
        assert!(lock_path.exists(), "fault point must precede stale rename");
        assert!(
            claim_path.exists(),
            "terminated reclaimer must leave its claim"
        );

        let recovery_environment = FakeEnvironment::new();
        let lock = EnvResultLock::acquire_with(&result, &recovery_environment).unwrap();
        lock.release().unwrap();
        assert_no_recovery_artifacts(&result);
    }

    fn write_foreign_record(
        result_path: &Path,
        pid: u32,
        process_start_token: &str,
        owner_token: &str,
    ) -> EnvResultLockRecord {
        write_foreign_record_on_host(
            result_path,
            "test-host",
            pid,
            process_start_token,
            owner_token,
        )
    }

    fn write_foreign_record_on_host(
        result_path: &Path,
        host_identity: &str,
        pid: u32,
        process_start_token: &str,
        owner_token: &str,
    ) -> EnvResultLockRecord {
        let normalized = normalize_result_path(result_path).unwrap();
        let record = EnvResultLockRecord {
            schema_version: LOCK_SCHEMA_VERSION.to_string(),
            host_identity: host_identity.to_string(),
            owner_token: owner_token.to_string(),
            pid,
            process_start_token: process_start_token.to_string(),
            acquired_at_unix_ms: 1_750_000_000_000,
            normalized_result_path: normalized.display().to_string(),
        };
        fs::write(
            normalized.with_extension("json.lock"),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        record
    }

    fn write_record_at(
        path: &Path,
        result_path: &Path,
        pid: u32,
        process_start_token: &str,
        owner_token: &str,
    ) {
        write_record_at_on_host(
            path,
            result_path,
            "test-host",
            pid,
            process_start_token,
            owner_token,
        );
    }

    fn write_record_at_on_host(
        path: &Path,
        result_path: &Path,
        host_identity: &str,
        pid: u32,
        process_start_token: &str,
        owner_token: &str,
    ) {
        let normalized = normalize_result_path(result_path).unwrap();
        let record = EnvResultLockRecord {
            schema_version: LOCK_SCHEMA_VERSION.to_string(),
            host_identity: host_identity.to_string(),
            owner_token: owner_token.to_string(),
            pid,
            process_start_token: process_start_token.to_string(),
            acquired_at_unix_ms: 1_750_000_000_000,
            normalized_result_path: normalized.display().to_string(),
        };
        fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    }

    fn assert_no_recovery_artifacts(result: &Path) {
        let lock_dir = result.parent().unwrap();
        assert!(
            fs::read_dir(lock_dir).unwrap().all(|entry| {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                !name.contains(".reclaim.")
                    && !name.contains(".stale.")
                    && !name.contains(".pending.")
            }),
            "successful recovery must not leave claim or tombstone artifacts"
        );
    }

    fn wait_for_child_identity(environment: &SystemLockEnvironment, pid: u32) -> ProcessStatus {
        for _ in 0..20 {
            match environment.inspect_process(pid).unwrap() {
                ProcessStatus::Missing => std::thread::sleep(Duration::from_millis(25)),
                status => return status,
            }
        }
        panic!("child process did not become observable");
    }

    #[cfg(target_os = "windows")]
    fn sleeping_child() -> Child {
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 60",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    fn sleeping_child() -> Child {
        Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }
}
