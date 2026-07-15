// SPDX-License-Identifier: AGPL-3.0-only

//! Transactional publication for generated package archives.
//!
//! Package bytes live in immutable generations. Consumers switch generations by
//! reading an append-only, checksummed pointer journal. This avoids relying on
//! replacement `rename` semantics, which differ between Unix and Windows: Unix
//! commonly replaces an existing destination, while Windows commonly rejects it.
//! A reader ignores only an incomplete trailing journal record; any complete but
//! invalid record is a fatal publication error.

use actingcommand_contract::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STATE_DIRECTORY: &str = ".actingcommand-publish";
const AUTHORITIES_DIRECTORY: &str = "authorities";
const LOCKS_DIRECTORY: &str = "locks";
const GENERATIONS_DIRECTORY: &str = "generations";
const POINTER_FILE: &str = "current.pointer.jsonl";
const GENERATION_MANIFEST_FILE: &str = "generation.json";
const POINTER_SCHEMA: &str = "actingcommand.package-pointer.v1";
const GENERATION_SCHEMA: &str = "actingcommand.package-generation.v1";
const LOCK_SCHEMA: &str = "actingcommand.package-publish-lock.v1";
const MAX_LOCK_ATTEMPTS: usize = 8;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// A committed immutable generation and the physical files that now back its logical outputs.
#[derive(Debug, Clone)]
pub struct PackagePublicationCommit {
    /// Stable identifier recorded by the publication pointer.
    pub generation_id: String,
    /// Normalized logical output paths mapped to immutable generation files.
    pub resolved_outputs: BTreeMap<String, PathBuf>,
}

/// Owns all locks and staging state for one package publication.
#[must_use = "publication transactions must be committed or aborted explicitly"]
#[derive(Debug)]
pub struct PackagePublicationTransaction {
    state_parent: PathBuf,
    authority_root: PathBuf,
    pointer_path: PathBuf,
    generation_id: String,
    generation_dir: PathBuf,
    requested_outputs: BTreeMap<String, OutputPlan>,
    locked_output_keys: BTreeSet<String>,
    lock_set_digest: String,
    locks: Vec<PublicationLock>,
    #[cfg(test)]
    fault: Option<PublicationFaultPoint>,
}

#[derive(Debug, Clone)]
struct OutputPlan {
    relative_path: String,
}

#[derive(Debug)]
struct PublicationLock {
    path: PathBuf,
    owner_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    start_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcessStatus {
    Alive { start_token: String },
    Dead,
}

trait PublicationEnvironment {
    fn current_process(&self) -> Result<ProcessIdentity, String>;
    fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String>;
    fn random_seed(&self) -> Result<[u8; 32], String>;
    fn now_unix_ms(&self) -> Result<u128, String>;
}

struct SystemPublicationEnvironment;

impl PublicationEnvironment for SystemPublicationEnvironment {
    fn current_process(&self) -> Result<ProcessIdentity, String> {
        let pid = std::process::id();
        match inspect_system_process(pid)? {
            ProcessStatus::Alive { start_token } => Ok(ProcessIdentity { pid, start_token }),
            ProcessStatus::Dead => Err(format!("current process {pid} was not found")),
        }
    }

    fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String> {
        inspect_system_process(pid)
    }

    fn random_seed(&self) -> Result<[u8; 32], String> {
        system_random_seed()
    }

    fn now_unix_ms(&self) -> Result<u128, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .map_err(|error| format!("system clock precedes Unix epoch: {error}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PublicationLockRecord {
    schema_version: String,
    owner_token: String,
    pid: u32,
    process_start_token: String,
    acquired_unix_ms: u128,
    output_set_digest: String,
    normalized_outputs: Vec<String>,
    lock_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PublishedOutput {
    relative_path: String,
    byte_count: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct GenerationManifest {
    schema_version: String,
    generation_id: String,
    output_set_digest: String,
    outputs: BTreeMap<String, PublishedOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CleanupTarget {
    LegacyOutput { normalized_path: String },
    Generation { generation_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PointerRecord {
    schema_version: String,
    sequence: u64,
    committed_unix_ms: u128,
    generation_id: String,
    generation_manifest_sha256: String,
    output_set_digest: String,
    transaction_output_set_digest: String,
    locked_outputs: Vec<String>,
    outputs: BTreeMap<String, PublishedOutput>,
    previous_generation_id: Option<String>,
    pending_cleanup: Vec<CleanupTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PointerEnvelope {
    record: PointerRecord,
    checksum_sha256: String,
}

struct PointerLog {
    last: Option<PointerRecord>,
    complete_len: u64,
    total_len: u64,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationFaultPoint {
    BeforeFirstFile,
    MidFiles,
    BeforeCommit,
    AfterCommit,
    Cleanup,
    PointerPartialWrite,
    PointerWriteReportedFailure,
    ProcessExitAfterCommit,
}

impl PackagePublicationTransaction {
    /// Starts an atomic publication for one logical package path.
    pub fn begin_single(output: &Path) -> LabResult<Self> {
        Self::begin_single_with(output, &SystemPublicationEnvironment)
    }

    /// Starts one atomic publication for a complete set of package outputs.
    pub fn begin_group(output_directory: &Path, outputs: &[PathBuf]) -> LabResult<Self> {
        Self::begin_group_with(output_directory, outputs, &SystemPublicationEnvironment)
    }

    fn begin_single_with(
        output: &Path,
        environment: &impl PublicationEnvironment,
    ) -> LabResult<Self> {
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        Self::begin_with(parent, output, &[output.to_path_buf()], true, environment)
    }

    fn begin_group_with(
        output_directory: &Path,
        outputs: &[PathBuf],
        environment: &impl PublicationEnvironment,
    ) -> LabResult<Self> {
        Self::begin_with(
            output_directory,
            output_directory,
            outputs,
            false,
            environment,
        )
    }

    fn begin_with(
        state_parent: &Path,
        authority_scope: &Path,
        outputs: &[PathBuf],
        single_authority: bool,
        environment: &impl PublicationEnvironment,
    ) -> LabResult<Self> {
        if outputs.is_empty() {
            return Err(publication_error(
                "publication output set must not be empty",
            ));
        }
        fs::create_dir_all(state_parent).map_err(|error| {
            publication_error(format!(
                "failed to create publication output directory {}: {error}",
                state_parent.display()
            ))
        })?;
        let state_parent = fs::canonicalize(state_parent).map_err(|error| {
            publication_error(format!(
                "failed to normalize publication output directory {}: {error}",
                state_parent.display()
            ))
        })?;
        let authority_key = normalize_scope_key(authority_scope, &state_parent, single_authority)?;
        let authority_digest = digest_text(&authority_key);
        let state_root = state_parent.join(STATE_DIRECTORY);
        let authority_root = state_root
            .join(AUTHORITIES_DIRECTORY)
            .join(&authority_digest);
        let pointer_path = authority_root.join(POINTER_FILE);

        let requested = normalize_requested_outputs(&state_parent, outputs)?;
        let mut previous_outputs = BTreeSet::new();
        if let Some(previous) = read_pointer_log(&pointer_path)?.last {
            validate_pointer_record(&previous)?;
            previous_outputs.extend(previous.locked_outputs.iter().cloned());
        }
        let requested_keys = requested.keys().cloned().collect::<BTreeSet<_>>();
        let mut locked_output_keys = requested_keys.clone();
        locked_output_keys.extend(previous_outputs);
        let lock_set_digest = digest_output_set(&locked_output_keys);

        let identity = environment.current_process().map_err(|error| {
            publication_error(format!("failed to identify publication process: {error}"))
        })?;
        let acquired_unix_ms = environment.now_unix_ms().map_err(|error| {
            publication_error(format!("failed to timestamp publication lock: {error}"))
        })?;
        let owner_token = random_identifier(environment, "owner", &identity, acquired_unix_ms)?;
        let normalized_outputs = locked_output_keys.iter().cloned().collect::<Vec<_>>();

        let locks_dir = state_root.join(LOCKS_DIRECTORY);
        fs::create_dir_all(&locks_dir).map_err(|error| {
            publication_error(format!(
                "failed to create publication lock directory {}: {error}",
                locks_dir.display()
            ))
        })?;
        require_regular_directory(&state_root, "publication state directory")?;
        require_regular_directory(&locks_dir, "publication lock directory")?;
        let mut lock_keys = vec![authority_lock_key(&authority_key)];
        lock_keys.extend(locked_output_keys.iter().map(|key| output_lock_key(key)));
        lock_keys.sort();
        lock_keys.dedup();
        let mut locks = Vec::new();
        for lock_key in lock_keys {
            let record = PublicationLockRecord {
                schema_version: LOCK_SCHEMA.to_string(),
                owner_token: owner_token.clone(),
                pid: identity.pid,
                process_start_token: identity.start_token.clone(),
                acquired_unix_ms,
                output_set_digest: lock_set_digest.clone(),
                normalized_outputs: normalized_outputs.clone(),
                lock_key: lock_key.clone(),
            };
            let path = locks_dir.join(format!("{}.lock", digest_text(&lock_key)));
            match PublicationLock::acquire(path, record, environment) {
                Ok(lock) => locks.push(lock),
                Err(error) => {
                    return Err(combine_with_lock_release(error, &mut locks));
                }
            }
        }

        if let Err(error) = validate_no_cross_authority_claims(
            &state_parent,
            &authority_root,
            requested_keys.iter(),
        ) {
            return Err(combine_with_lock_release(error, &mut locks));
        }

        if let Err(error) = recover_authority(
            &state_parent,
            &authority_root,
            &pointer_path,
            &locked_output_keys,
        ) {
            return Err(combine_with_lock_release(error, &mut locks));
        }
        let generation_id = match random_identifier(
            environment,
            "generation",
            &identity,
            environment.now_unix_ms().map_err(|error| {
                publication_error(format!(
                    "failed to timestamp publication generation: {error}"
                ))
            })?,
        ) {
            Ok(identifier) => identifier,
            Err(error) => return Err(combine_with_lock_release(error, &mut locks)),
        };
        let generation_dir = authority_root
            .join(GENERATIONS_DIRECTORY)
            .join(&generation_id);
        if let Err(error) = fs::create_dir_all(generation_dir.join("files")) {
            return Err(combine_with_lock_release(
                publication_error(format!(
                    "failed to create publication generation {}: {error}",
                    generation_dir.display()
                )),
                &mut locks,
            ));
        }
        if let Err(error) = require_regular_directory(&generation_dir, "publication generation")
            .and_then(|()| {
                require_regular_directory(
                    &generation_dir.join("files"),
                    "publication generation files directory",
                )
            })
        {
            return Err(combine_with_lock_release(error, &mut locks));
        }
        let requested_outputs = assign_generation_paths(requested);
        Ok(Self {
            state_parent,
            authority_root,
            pointer_path,
            generation_id,
            generation_dir,
            requested_outputs,
            locked_output_keys,
            lock_set_digest,
            locks,
            #[cfg(test)]
            fault: None,
        })
    }

    /// Returns the immutable-generation path where the caller must write one requested output.
    pub fn staging_path(&self, logical_output: &Path) -> LabResult<PathBuf> {
        let key = normalize_output_for_existing_parent(logical_output, &self.state_parent)?;
        let plan = self.requested_outputs.get(&key).ok_or_else(|| {
            publication_error(format!(
                "output {} is not part of this publication transaction",
                logical_output.display()
            ))
        })?;
        Ok(self.generation_dir.join(&plan.relative_path))
    }

    /// Validates every staged output, switches the pointer once, and performs recorded cleanup.
    pub fn commit(mut self) -> LabResult<PackagePublicationCommit> {
        let outputs = match self.verify_staged_outputs() {
            Ok(outputs) => outputs,
            Err(error) => return Err(self.abort_with_primary(error)),
        };
        let generation_output_set = outputs.keys().cloned().collect::<BTreeSet<_>>();
        let generation_output_set_digest = digest_output_set(&generation_output_set);
        let manifest = GenerationManifest {
            schema_version: GENERATION_SCHEMA.to_string(),
            generation_id: self.generation_id.clone(),
            output_set_digest: generation_output_set_digest.clone(),
            outputs: outputs.clone(),
        };
        let manifest_hash = match write_generation_manifest(&self.generation_dir, &manifest) {
            Ok(hash) => hash,
            Err(error) => return Err(self.abort_with_primary(error)),
        };

        #[cfg(test)]
        if self.fault == Some(PublicationFaultPoint::BeforeCommit) {
            return Err(self.abort_with_primary(publication_error(
                "injected publication failure before pointer commit",
            )));
        }

        let previous = match read_pointer_log(&self.pointer_path) {
            Ok(log) if log.complete_len == log.total_len => log.last,
            Ok(_) => {
                let error = publication_error(format!(
                    "publication pointer {} gained an incomplete record while locked",
                    self.pointer_path.display()
                ));
                return Err(self.abort_with_primary(error));
            }
            Err(error) => return Err(self.abort_with_primary(error)),
        };
        let sequence = match next_sequence(previous.as_ref()) {
            Ok(sequence) => sequence,
            Err(error) => return Err(self.abort_with_primary(error)),
        };
        let pending_cleanup = match self.cleanup_plan(previous.as_ref()) {
            Ok(pending) => pending,
            Err(error) => return Err(self.abort_with_primary(error)),
        };
        let committed_unix_ms = match SystemPublicationEnvironment.now_unix_ms() {
            Ok(value) => value,
            Err(error) => {
                return Err(self.abort_with_primary(publication_error(format!(
                    "failed to timestamp package publication commit: {error}"
                ))));
            }
        };
        let (transaction_output_set_digest, locked_outputs) = if pending_cleanup.is_empty() {
            (
                generation_output_set_digest.clone(),
                outputs.keys().cloned().collect(),
            )
        } else {
            (
                self.lock_set_digest.clone(),
                self.locked_output_keys.iter().cloned().collect(),
            )
        };
        let mut record = PointerRecord {
            schema_version: POINTER_SCHEMA.to_string(),
            sequence,
            committed_unix_ms,
            generation_id: self.generation_id.clone(),
            generation_manifest_sha256: manifest_hash,
            output_set_digest: generation_output_set_digest,
            transaction_output_set_digest,
            locked_outputs,
            outputs,
            previous_generation_id: previous.map(|record| record.generation_id),
            pending_cleanup,
        };
        let append_result = self.append_initial_pointer_for_commit(&record);
        if let Err(error) = append_result {
            return Err(self.resolve_initial_pointer_commit_failure(&record, error));
        }

        #[cfg(test)]
        if self.fault == Some(PublicationFaultPoint::ProcessExitAfterCommit) {
            std::process::exit(74);
        }
        #[cfg(test)]
        if self.fault == Some(PublicationFaultPoint::AfterCommit) {
            let error = publication_error(format!(
                "injected publication failure after commit; committed_generation={}; pending_cleanup={:?}",
                self.generation_id, record.pending_cleanup
            ));
            return Err(self.finish_committed_error(error));
        }

        #[cfg(test)]
        if self.fault == Some(PublicationFaultPoint::Cleanup) {
            let error = publication_error(format!(
                "injected publication cleanup failure; committed_generation={}; pending_cleanup={:?}",
                self.generation_id, record.pending_cleanup
            ));
            return Err(self.finish_committed_error(error));
        }

        if let Err(error) = cleanup_targets(
            &self.state_parent,
            &self.authority_root,
            &record,
            &record.pending_cleanup,
        ) {
            let error = publication_error(format!(
                "package publication committed but cleanup failed; committed_generation={}; pending_cleanup={:?}; original_error={}",
                self.generation_id, record.pending_cleanup, error.message
            ));
            return Err(self.finish_committed_error(error));
        }
        if !record.pending_cleanup.is_empty() {
            record.sequence = match record.sequence.checked_add(1) {
                Some(sequence) => sequence,
                None => {
                    let error = publication_error(format!(
                        "package publication committed but cleanup checkpoint overflowed; committed_generation={}",
                        self.generation_id
                    ));
                    return Err(self.finish_committed_error(error));
                }
            };
            record.pending_cleanup.clear();
            record.locked_outputs = record.outputs.keys().cloned().collect();
            record.transaction_output_set_digest = digest_output_set(
                &record
                    .locked_outputs
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            );
            if let Err(error) = append_pointer_record(&self.pointer_path, &record) {
                let error = publication_error(format!(
                    "package publication committed and cleanup completed but pointer update failed; committed_generation={}; original_error={}",
                    self.generation_id, error.message
                ));
                return Err(self.finish_committed_error(error));
            }
        }
        let resolved_outputs = match self.resolved_outputs(&record.outputs) {
            Ok(outputs) => outputs,
            Err(error) => return Err(self.finish_committed_error(error)),
        };
        if let Err(error) = release_locks(&mut self.locks) {
            return Err(publication_error(format!(
                "package publication committed but lock release failed; committed_generation={}; original_error={}",
                self.generation_id, error.message
            )));
        }
        Ok(PackagePublicationCommit {
            generation_id: self.generation_id.clone(),
            resolved_outputs,
        })
    }

    /// Removes uncommitted staging and releases every transaction lock.
    pub fn abort(mut self) -> LabResult<()> {
        let mut failure = None;
        if self.generation_dir.exists()
            && let Err(error) = fs::remove_dir_all(&self.generation_dir)
        {
            failure = Some(publication_error(format!(
                "failed to remove aborted generation {}: {error}",
                self.generation_dir.display()
            )));
        }
        if let Err(error) = release_locks(&mut self.locks) {
            failure = Some(match failure {
                Some(primary) => combine_errors(primary, error),
                None => error,
            });
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn abort_with_primary(mut self, primary: LabError) -> LabError {
        let mut error = primary;
        if self.generation_dir.exists()
            && let Err(cleanup) = fs::remove_dir_all(&self.generation_dir)
        {
            error = combine_errors(
                error,
                publication_error(format!(
                    "failed to remove uncommitted generation {}: {cleanup}",
                    self.generation_dir.display()
                )),
            );
        }
        if let Err(release) = release_locks(&mut self.locks) {
            error = combine_errors(error, release);
        }
        error
    }

    fn finish_committed_error(mut self, mut error: LabError) -> LabError {
        if let Err(release) = release_locks(&mut self.locks) {
            error = combine_errors(error, release);
        }
        error
    }

    fn resolve_initial_pointer_commit_failure(
        self,
        record: &PointerRecord,
        primary: LabError,
    ) -> LabError {
        match read_pointer_log(&self.pointer_path) {
            Ok(log) if log.last.as_ref() == Some(record) => {
                self.finish_committed_error(publication_error(format!(
                    "package pointer write reported failure after commit became visible; committed_generation={}; pending_cleanup={:?}; original_error={}",
                    record.generation_id, record.pending_cleanup, primary.message
                )))
            }
            Ok(log) if is_pointer_predecessor(log.last.as_ref(), record.sequence) => {
                self.abort_with_primary(primary)
            }
            Ok(log) => self.finish_committed_error(publication_error(format!(
                "package pointer commit state is unknown; generation_retained={}; observed_sequence={:?}; original_error={}",
                record.generation_id,
                log.last.map(|value| value.sequence),
                primary.message
            ))),
            Err(inspect) => self.finish_committed_error(publication_error(format!(
                "package pointer commit state is unknown; generation_retained={}; original_error={}; inspection_error={}",
                record.generation_id, primary.message, inspect.message
            ))),
        }
    }

    fn append_initial_pointer_for_commit(&self, record: &PointerRecord) -> LabResult<()> {
        #[cfg(test)]
        if self.fault == Some(PublicationFaultPoint::PointerPartialWrite) {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.pointer_path)
                .map_err(|error| {
                    publication_error(format!(
                        "failed to inject partial pointer write {}: {error}",
                        self.pointer_path.display()
                    ))
                })?;
            file.write_all(br#"{"record":{"sequence":"#)
                .and_then(|()| file.flush())
                .map_err(|error| {
                    publication_error(format!(
                        "failed to inject partial pointer write {}: {error}",
                        self.pointer_path.display()
                    ))
                })?;
            return Err(publication_error(
                "injected publication pointer partial-write failure",
            ));
        }

        append_pointer_record(&self.pointer_path, record)?;
        #[cfg(test)]
        if self.fault == Some(PublicationFaultPoint::PointerWriteReportedFailure) {
            return Err(publication_error(
                "injected publication pointer write-reported failure",
            ));
        }
        Ok(())
    }

    fn verify_staged_outputs(&self) -> LabResult<BTreeMap<String, PublishedOutput>> {
        let mut outputs = BTreeMap::new();
        let mut expected_paths = BTreeSet::new();
        for (logical, plan) in &self.requested_outputs {
            let staged = self.generation_dir.join(&plan.relative_path);
            let metadata = fs::symlink_metadata(&staged).map_err(|error| {
                publication_error(format!(
                    "missing staged package output {} for {}: {error}",
                    staged.display(),
                    logical
                ))
            })?;
            if !metadata.file_type().is_file() || metadata_is_link_or_reparse(&metadata) {
                return Err(publication_error(format!(
                    "staged package output is not a regular file: {}",
                    staged.display()
                )));
            }
            let (byte_count, sha256) = hash_file(&staged)?;
            expected_paths.insert(staged);
            outputs.insert(
                logical.clone(),
                PublishedOutput {
                    relative_path: plan.relative_path.clone(),
                    byte_count,
                    sha256,
                },
            );
        }
        let files_dir = self.generation_dir.join("files");
        for entry in fs::read_dir(&files_dir).map_err(|error| {
            publication_error(format!(
                "failed to inspect {}: {error}",
                files_dir.display()
            ))
        })? {
            let path = entry
                .map_err(|error| {
                    publication_error(format!(
                        "failed to inspect generation directory {}: {error}",
                        files_dir.display()
                    ))
                })?
                .path();
            if !expected_paths.contains(&path) {
                return Err(publication_error(format!(
                    "unexpected file in package generation: {}",
                    path.display()
                )));
            }
        }
        Ok(outputs)
    }

    fn cleanup_plan(&self, previous: Option<&PointerRecord>) -> LabResult<Vec<CleanupTarget>> {
        let mut pending = BTreeSet::new();
        for key in &self.locked_output_keys {
            let path = PathBuf::from(key);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                    return Err(publication_error(format!(
                        "legacy package output is a symlink or reparse point: {}",
                        path.display()
                    )));
                }
                Ok(metadata) if metadata.file_type().is_dir() => {
                    return Err(publication_error(format!(
                        "legacy package output is a directory: {}",
                        path.display()
                    )));
                }
                Ok(_) => {
                    pending.insert(cleanup_sort_key(&CleanupTarget::LegacyOutput {
                        normalized_path: key.clone(),
                    }));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(publication_error(format!(
                        "failed to inspect legacy package output {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        let retained = previous
            .map(|record| record.generation_id.as_str())
            .into_iter()
            .collect::<BTreeSet<_>>();
        let generations = self.authority_root.join(GENERATIONS_DIRECTORY);
        if generations.exists() {
            for entry in fs::read_dir(&generations).map_err(|error| {
                publication_error(format!("failed to read {}: {error}", generations.display()))
            })? {
                let entry = entry.map_err(|error| {
                    publication_error(format!("failed to read {}: {error}", generations.display()))
                })?;
                let generation_id = entry.file_name().to_string_lossy().to_string();
                if generation_id != self.generation_id && !retained.contains(generation_id.as_str())
                {
                    pending.insert(cleanup_sort_key(&CleanupTarget::Generation {
                        generation_id,
                    }));
                }
            }
        }
        pending
            .into_iter()
            .map(|value| serde_json::from_str(&value).map_err(json_publication_error))
            .collect()
    }

    fn resolved_outputs(
        &self,
        outputs: &BTreeMap<String, PublishedOutput>,
    ) -> LabResult<BTreeMap<String, PathBuf>> {
        outputs
            .iter()
            .map(|(logical, output)| {
                let path = checked_generation_output_path(&self.generation_dir, output)?;
                Ok((logical.clone(), path))
            })
            .collect()
    }

    #[cfg(test)]
    fn with_fault(mut self, fault: PublicationFaultPoint) -> Self {
        self.fault = Some(fault);
        self
    }

    #[cfg(test)]
    fn checkpoint(&self, point: PublicationFaultPoint) -> LabResult<()> {
        if self.fault == Some(point) {
            return Err(publication_error(format!(
                "injected publication failure at {point:?}"
            )));
        }
        Ok(())
    }
}

impl Drop for PackagePublicationTransaction {
    fn drop(&mut self) {
        // Drop cannot report cleanup failures. Leaving ownership records intact
        // makes an abandoned transaction recoverable after its process dies.
    }
}

impl PublicationLock {
    fn acquire(
        path: PathBuf,
        record: PublicationLockRecord,
        environment: &impl PublicationEnvironment,
    ) -> LabResult<Self> {
        for attempt in 1..=MAX_LOCK_ATTEMPTS {
            match create_lock_file(&path, &record) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        owner_token: record.owner_token,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(publication_error(format!(
                        "failed to create publication lock {}: {error}",
                        path.display()
                    )));
                }
            }
            let Some(observed) = read_lock_record_if_present(&path)? else {
                if attempt < MAX_LOCK_ATTEMPTS {
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                continue;
            };
            validate_lock_record(&observed, &record.lock_key)?;
            let stale = match environment.inspect_process(observed.pid) {
                Ok(ProcessStatus::Dead) => true,
                Ok(ProcessStatus::Alive { start_token }) => {
                    start_token != observed.process_start_token
                }
                Err(error) => {
                    return Err(publication_error(format!(
                        "cannot confirm publication lock owner death; lock={}; pid={}; owner_token={}; original_error={error}",
                        path.display(),
                        observed.pid,
                        observed.owner_token
                    )));
                }
            };
            if !stale {
                return Err(publication_error(format!(
                    "publication output is locked by a live owner; lock={}; pid={}; process_start={}; owner_token={}; output_set_digest={}",
                    path.display(),
                    observed.pid,
                    observed.process_start_token,
                    observed.owner_token,
                    observed.output_set_digest
                )));
            }
            match reclaim_stale_lock(&path, &observed, &record.owner_token) {
                Ok(true) => {}
                Ok(false) => {
                    if attempt == MAX_LOCK_ATTEMPTS {
                        break;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(error) => return Err(error),
            }
        }
        Err(publication_error(format!(
            "publication lock recovery exhausted; lock={}; retries={MAX_LOCK_ATTEMPTS}; timeout_ms={}; escalation=fail_loud",
            path.display(),
            LOCK_RETRY_DELAY.as_millis() * MAX_LOCK_ATTEMPTS as u128
        )))
    }

    fn release(&self) -> LabResult<()> {
        let observed = read_lock_record(&self.path)?;
        if observed.owner_token != self.owner_token {
            return Err(publication_error(format!(
                "refusing to release publication lock owned by another process; lock={}; expected_owner={}; observed_owner={}",
                self.path.display(),
                self.owner_token,
                observed.owner_token
            )));
        }
        fs::remove_file(&self.path).map_err(|error| {
            publication_error(format!(
                "failed to release publication lock {}: {error}",
                self.path.display()
            ))
        })
    }
}

/// Resolves a logical package path to its current committed immutable generation.
pub fn resolve_published_package_path(logical_path: &Path) -> LabResult<PathBuf> {
    let Some(parent) = logical_path.parent().or_else(|| Some(Path::new("."))) else {
        return Ok(logical_path.to_path_buf());
    };
    let state_parent = match fs::canonicalize(parent) {
        Ok(parent) => parent,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(logical_path.to_path_buf());
        }
        Err(error) => {
            return Err(publication_error(format!(
                "failed to normalize package output parent {}: {error}",
                parent.display()
            )));
        }
    };
    let logical_key = normalize_output_for_existing_parent(logical_path, &state_parent)?;
    let authorities = state_parent
        .join(STATE_DIRECTORY)
        .join(AUTHORITIES_DIRECTORY);
    if !authorities.exists() {
        return Ok(logical_path.to_path_buf());
    }
    let authority_digests = candidate_authority_digests(&state_parent, &logical_key)?;
    let mut resolved = None;
    for authority_digest in authority_digests {
        let authority_root = authorities.join(authority_digest);
        let metadata = match fs::symlink_metadata(&authority_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(publication_error(format!(
                    "failed to inspect publication authority {}: {error}",
                    authority_root.display()
                )));
            }
        };
        if !metadata.file_type().is_dir() || metadata_is_link_or_reparse(&metadata) {
            return Err(publication_error(format!(
                "publication authority is not a regular directory: {}",
                authority_root.display()
            )));
        }
        let pointer = read_pointer_log(&authority_root.join(POINTER_FILE))?;
        let Some(record) = pointer.last else {
            continue;
        };
        validate_pointer_record(&record)?;
        let Some(output) = record.outputs.get(&logical_key) else {
            continue;
        };
        let generation_dir = authority_root
            .join(GENERATIONS_DIRECTORY)
            .join(&record.generation_id);
        validate_generation(&authority_root, &record)?;
        let candidate = checked_generation_output_path(&generation_dir, output)?;
        if resolved.replace(candidate).is_some() {
            return Err(publication_error(format!(
                "multiple committed publication authorities claim output {}",
                logical_path.display()
            )));
        }
    }
    Ok(resolved.unwrap_or_else(|| logical_path.to_path_buf()))
}

fn validate_no_cross_authority_claims<'a>(
    state_parent: &Path,
    current_authority_root: &Path,
    logical_outputs: impl Iterator<Item = &'a String>,
) -> LabResult<()> {
    let authorities = state_parent
        .join(STATE_DIRECTORY)
        .join(AUTHORITIES_DIRECTORY);
    for logical_output in logical_outputs {
        for digest in candidate_authority_digests(state_parent, logical_output)? {
            let authority_root = authorities.join(digest);
            if authority_root == current_authority_root {
                continue;
            }
            let pointer = read_pointer_log(&authority_root.join(POINTER_FILE))?;
            let Some(record) = pointer.last else {
                continue;
            };
            validate_pointer_record(&record)?;
            if record.outputs.contains_key(logical_output) {
                validate_generation(&authority_root, &record)?;
                return Err(publication_error(format!(
                    "package output is already owned by another publication authority; output={logical_output}; authority={}",
                    authority_root.display()
                )));
            }
        }
    }
    Ok(())
}

fn candidate_authority_digests(
    state_parent: &Path,
    logical_output: &str,
) -> LabResult<BTreeSet<String>> {
    Ok(BTreeSet::from([
        digest_text(logical_output),
        digest_text(&path_key(state_parent)?),
    ]))
}

fn normalize_requested_outputs(
    state_parent: &Path,
    outputs: &[PathBuf],
) -> LabResult<BTreeMap<String, PathBuf>> {
    let mut normalized = BTreeMap::new();
    for output in outputs {
        let key = normalize_output_for_existing_parent(output, state_parent)?;
        let path = PathBuf::from(&key);
        if normalized.insert(key.clone(), path).is_some() {
            return Err(publication_error(format!(
                "duplicate normalized package output: {key}"
            )));
        }
    }
    Ok(normalized)
}

fn assign_generation_paths(requested: BTreeMap<String, PathBuf>) -> BTreeMap<String, OutputPlan> {
    requested
        .into_iter()
        .enumerate()
        .map(|(index, (key, logical_path))| {
            let file_name = logical_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("package.zip");
            let relative_path =
                format!("files/{index:04}-{}-{file_name}", &digest_text(&key)[..16]);
            (key, OutputPlan { relative_path })
        })
        .collect()
}

fn normalize_scope_key(
    authority_scope: &Path,
    state_parent: &Path,
    single: bool,
) -> LabResult<String> {
    if single {
        return normalize_output_for_existing_parent(authority_scope, state_parent);
    }
    let scope = fs::canonicalize(authority_scope).map_err(|error| {
        publication_error(format!(
            "failed to normalize publication authority {}: {error}",
            authority_scope.display()
        ))
    })?;
    path_key(&scope)
}

fn normalize_output_for_existing_parent(output: &Path, state_parent: &Path) -> LabResult<String> {
    let file_name = output.file_name().ok_or_else(|| {
        publication_error(format!(
            "package output has no file name: {}",
            output.display()
        ))
    })?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|error| {
        publication_error(format!(
            "failed to normalize package output parent {}: {error}",
            parent.display()
        ))
    })?;
    if parent != state_parent {
        return Err(publication_error(format!(
            "package output {} is outside publication directory {}",
            output.display(),
            state_parent.display()
        )));
    }
    path_key(&parent.join(file_name))
}

fn path_key(path: &Path) -> LabResult<String> {
    let mut value = path
        .to_str()
        .ok_or_else(|| publication_error(format!("path is not valid UTF-8: {}", path.display())))?
        .replace('\\', "/");
    #[cfg(windows)]
    {
        value = value.to_lowercase();
    }
    Ok(value)
}

fn authority_lock_key(authority_key: &str) -> String {
    format!("authority\0{authority_key}")
}

fn output_lock_key(output_key: &str) -> String {
    format!("output\0{output_key}")
}

fn digest_output_set(outputs: &BTreeSet<String>) -> String {
    let mut hasher = Sha256::new();
    for output in outputs {
        hasher.update(output.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn digest_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn random_identifier(
    environment: &impl PublicationEnvironment,
    purpose: &str,
    identity: &ProcessIdentity,
    now_unix_ms: u128,
) -> LabResult<String> {
    let seed = environment.random_seed().map_err(|error| {
        publication_error(format!(
            "failed to obtain secure random bytes for {purpose}: {error}"
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(seed);
    hasher.update(purpose.as_bytes());
    hasher.update(identity.pid.to_be_bytes());
    hasher.update(identity.start_token.as_bytes());
    hasher.update(now_unix_ms.to_be_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

fn recover_authority(
    state_parent: &Path,
    authority_root: &Path,
    pointer_path: &Path,
    locked_output_keys: &BTreeSet<String>,
) -> LabResult<Option<PointerRecord>> {
    fs::create_dir_all(authority_root.join(GENERATIONS_DIRECTORY)).map_err(|error| {
        publication_error(format!(
            "failed to create publication authority {}: {error}",
            authority_root.display()
        ))
    })?;
    let authorities_directory = authority_root.parent().ok_or_else(|| {
        publication_error(format!(
            "publication authority has no parent: {}",
            authority_root.display()
        ))
    })?;
    require_regular_directory(authorities_directory, "publication authorities directory")?;
    require_regular_directory(authority_root, "publication authority")?;
    require_regular_directory(
        &authority_root.join(GENERATIONS_DIRECTORY),
        "publication generations directory",
    )?;
    let mut pointer = read_pointer_log(pointer_path)?;
    if pointer.complete_len < pointer.total_len {
        truncate_pointer(pointer_path, pointer.complete_len)?;
        pointer.total_len = pointer.complete_len;
    }
    let mut current = pointer.last;
    if let Some(record) = current.as_mut() {
        validate_pointer_record(record)?;
        validate_locked_pointer_outputs(record, state_parent, locked_output_keys)?;
        validate_generation(authority_root, record)?;
        if !record.pending_cleanup.is_empty() {
            cleanup_targets(
                state_parent,
                authority_root,
                record,
                &record.pending_cleanup,
            )?;
            record.sequence = record.sequence.checked_add(1).ok_or_else(|| {
                publication_error("publication pointer sequence overflow during recovery")
            })?;
            record.pending_cleanup.clear();
            record.locked_outputs = record.outputs.keys().cloned().collect();
            record.transaction_output_set_digest = digest_output_set(
                &record
                    .locked_outputs
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            );
            append_pointer_record(pointer_path, record)?;
        }
    }
    cleanup_uncommitted_generations(authority_root, current.as_ref())?;
    Ok(current)
}

fn validate_locked_pointer_outputs(
    record: &PointerRecord,
    state_parent: &Path,
    locked_output_keys: &BTreeSet<String>,
) -> LabResult<()> {
    for key in &record.locked_outputs {
        let path = PathBuf::from(key);
        let parent = path
            .parent()
            .ok_or_else(|| publication_error(format!("published output has no parent: {key}")))?;
        if path_key(parent)? != path_key(state_parent)? {
            return Err(publication_error(format!(
                "published output escapes its authority directory: {key}"
            )));
        }
        if !locked_output_keys.contains(key) {
            return Err(publication_error(format!(
                "publication recovery did not lock previous output: {key}"
            )));
        }
    }
    Ok(())
}

fn cleanup_uncommitted_generations(
    authority_root: &Path,
    current: Option<&PointerRecord>,
) -> LabResult<()> {
    let generations = authority_root.join(GENERATIONS_DIRECTORY);
    let retained = current
        .into_iter()
        .flat_map(|record| {
            std::iter::once(record.generation_id.as_str())
                .chain(record.previous_generation_id.as_deref())
        })
        .collect::<BTreeSet<_>>();
    for entry in fs::read_dir(&generations).map_err(|error| {
        publication_error(format!("failed to read {}: {error}", generations.display()))
    })? {
        let entry = entry.map_err(|error| {
            publication_error(format!("failed to read {}: {error}", generations.display()))
        })?;
        let generation_id = entry.file_name().to_string_lossy().to_string();
        if retained.contains(generation_id.as_str()) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            publication_error(format!(
                "failed to inspect uncommitted generation {}: {error}",
                entry.path().display()
            ))
        })?;
        if !metadata.file_type().is_dir() || metadata_is_link_or_reparse(&metadata) {
            return Err(publication_error(format!(
                "uncommitted generation is not a regular directory: {}",
                entry.path().display()
            )));
        }
        fs::remove_dir_all(entry.path()).map_err(|error| {
            publication_error(format!(
                "failed to remove uncommitted generation {}: {error}",
                entry.path().display()
            ))
        })?;
    }
    Ok(())
}

fn validate_generation(authority_root: &Path, record: &PointerRecord) -> LabResult<()> {
    let generation_dir = authority_root
        .join(GENERATIONS_DIRECTORY)
        .join(&record.generation_id);
    require_regular_directory(&generation_dir, "committed generation")?;
    require_regular_directory(&generation_dir.join("files"), "generation files directory")?;
    let manifest_path = generation_dir.join(GENERATION_MANIFEST_FILE);
    require_regular_file(&manifest_path, "committed generation manifest")?;
    let bytes = fs::read(&manifest_path).map_err(|error| {
        publication_error(format!(
            "failed to read committed generation manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    if hash != record.generation_manifest_sha256 {
        return Err(publication_error(format!(
            "committed generation manifest hash mismatch: {}",
            manifest_path.display()
        )));
    }
    let manifest: GenerationManifest =
        serde_json::from_slice(&bytes).map_err(json_publication_error)?;
    if manifest.schema_version != GENERATION_SCHEMA
        || manifest.generation_id != record.generation_id
        || manifest.output_set_digest != record.output_set_digest
        || manifest.outputs != record.outputs
    {
        return Err(publication_error(format!(
            "committed generation manifest conflicts with pointer: {}",
            manifest_path.display()
        )));
    }
    for output in manifest.outputs.values() {
        let path = checked_generation_output_path(&generation_dir, output)?;
        let (byte_count, sha256) = hash_file(&path)?;
        if byte_count != output.byte_count || sha256 != output.sha256 {
            return Err(publication_error(format!(
                "committed generation output hash mismatch: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn checked_generation_output_path(
    generation_dir: &Path,
    output: &PublishedOutput,
) -> LabResult<PathBuf> {
    let relative = Path::new(&output.relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(publication_error(format!(
            "generation output path is unsafe: {}",
            output.relative_path
        )));
    }
    let path = generation_dir.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        publication_error(format!(
            "failed to inspect generation output {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() || metadata_is_link_or_reparse(&metadata) {
        return Err(publication_error(format!(
            "generation output is not a regular file: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn write_generation_manifest(
    generation_dir: &Path,
    manifest: &GenerationManifest,
) -> LabResult<String> {
    let mut bytes = serde_json::to_vec(manifest).map_err(json_publication_error)?;
    bytes.push(b'\n');
    let final_path = generation_dir.join(GENERATION_MANIFEST_FILE);
    write_new_synced_file(&final_path, &bytes)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn append_pointer_record(path: &Path, record: &PointerRecord) -> LabResult<()> {
    validate_pointer_record(record)?;
    let payload = serde_json::to_vec(record).map_err(json_publication_error)?;
    let envelope = PointerEnvelope {
        record: record.clone(),
        checksum_sha256: format!("{:x}", Sha256::digest(&payload)),
    };
    let mut line = serde_json::to_vec(&envelope).map_err(json_publication_error)?;
    line.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            publication_error(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let existing = read_pointer_log(path)?;
    if existing.complete_len != existing.total_len {
        return Err(publication_error(format!(
            "refusing to append to incomplete publication pointer {}",
            path.display()
        )));
    }
    let expected_sequence = existing
        .last
        .as_ref()
        .map(|previous| previous.sequence.checked_add(1))
        .unwrap_or(Some(1))
        .ok_or_else(|| publication_error("publication pointer sequence overflow"))?;
    if record.sequence != expected_sequence {
        return Err(publication_error(format!(
            "publication pointer sequence is not contiguous: expected={expected_sequence}, next={}",
            record.sequence
        )));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            publication_error(format!(
                "failed to open publication pointer {}: {error}",
                path.display()
            ))
        })?;
    file.write_all(&line).map_err(|error| {
        publication_error(format!(
            "failed to append publication pointer {}: {error}",
            path.display()
        ))
    })?;
    file.flush().map_err(|error| {
        publication_error(format!(
            "failed to flush publication pointer {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        publication_error(format!(
            "failed to sync publication pointer {}: {error}",
            path.display()
        ))
    })
}

fn read_pointer_log(path: &Path) -> LabResult<PointerLog> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.file_type().is_file() || metadata_is_link_or_reparse(&metadata) =>
        {
            return Err(publication_error(format!(
                "publication pointer is not a regular file: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(PointerLog {
                last: None,
                complete_len: 0,
                total_len: 0,
            });
        }
        Err(error) => {
            return Err(publication_error(format!(
                "failed to inspect publication pointer {}: {error}",
                path.display()
            )));
        }
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(publication_error(format!(
                "publication pointer disappeared during read: {}",
                path.display()
            )));
        }
        Err(error) => {
            return Err(publication_error(format!(
                "failed to read publication pointer {}: {error}",
                path.display()
            )));
        }
    };
    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut last: Option<PointerRecord> = None;
    for line in bytes[..complete_len].split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let envelope: PointerEnvelope =
            serde_json::from_slice(line).map_err(json_publication_error)?;
        let payload = serde_json::to_vec(&envelope.record).map_err(json_publication_error)?;
        let checksum = format!("{:x}", Sha256::digest(&payload));
        if checksum != envelope.checksum_sha256 {
            return Err(publication_error(format!(
                "publication pointer checksum mismatch: {}",
                path.display()
            )));
        }
        validate_pointer_record(&envelope.record)?;
        let expected_sequence = last
            .as_ref()
            .map(|previous| previous.sequence.checked_add(1))
            .unwrap_or(Some(1))
            .ok_or_else(|| publication_error("publication pointer sequence overflow"))?;
        if envelope.record.sequence != expected_sequence {
            return Err(publication_error(format!(
                "publication pointer sequence gap in {}; expected={expected_sequence}; observed={}",
                path.display(),
                envelope.record.sequence
            )));
        }
        last = Some(envelope.record);
    }
    Ok(PointerLog {
        last,
        complete_len: u64::try_from(complete_len)
            .map_err(|_| publication_error("publication pointer length exceeds u64"))?,
        total_len: u64::try_from(bytes.len())
            .map_err(|_| publication_error("publication pointer length exceeds u64"))?,
    })
}

fn validate_pointer_record(record: &PointerRecord) -> LabResult<()> {
    if record.schema_version != POINTER_SCHEMA {
        return Err(publication_error(format!(
            "unsupported publication pointer schema: {}",
            record.schema_version
        )));
    }
    if record.sequence == 0
        || record.committed_unix_ms == 0
        || !is_hash_identifier(&record.generation_id)
        || !is_hash_identifier(&record.generation_manifest_sha256)
        || !is_hash_identifier(&record.output_set_digest)
        || !is_hash_identifier(&record.transaction_output_set_digest)
        || record.locked_outputs.is_empty()
        || record.outputs.is_empty()
    {
        return Err(publication_error(
            "publication pointer contains invalid identity fields",
        ));
    }
    if record
        .previous_generation_id
        .as_deref()
        .is_some_and(|value| !is_hash_identifier(value))
    {
        return Err(publication_error(
            "publication pointer contains invalid previous generation identity",
        ));
    }
    if record.previous_generation_id.as_deref() == Some(record.generation_id.as_str()) {
        return Err(publication_error(
            "publication pointer cannot reference its current generation as previous",
        ));
    }
    let mut relative_paths = BTreeSet::new();
    for (logical, output) in &record.outputs {
        if logical.is_empty()
            || output.byte_count == 0
            || !is_hash_identifier(&output.sha256)
            || output.relative_path.is_empty()
            || !relative_paths.insert(output.relative_path.as_str())
        {
            return Err(publication_error(
                "publication pointer contains an invalid output record",
            ));
        }
    }
    let locked = record
        .locked_outputs
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let visible = record.outputs.keys().cloned().collect::<BTreeSet<_>>();
    if locked.len() != record.locked_outputs.len()
        || locked.iter().any(String::is_empty)
        || digest_output_set(&visible) != record.output_set_digest
        || digest_output_set(&locked) != record.transaction_output_set_digest
        || record.outputs.keys().any(|key| !locked.contains(key))
    {
        return Err(publication_error(
            "publication pointer output-set identity is inconsistent",
        ));
    }
    let mut cleanup = BTreeSet::new();
    for target in &record.pending_cleanup {
        if !cleanup.insert(cleanup_sort_key(target)) {
            return Err(publication_error(
                "publication pointer contains duplicate cleanup targets",
            ));
        }
        match target {
            CleanupTarget::LegacyOutput { normalized_path }
                if normalized_path.is_empty() || !locked.contains(normalized_path) =>
            {
                return Err(publication_error(
                    "publication pointer contains an unowned legacy cleanup target",
                ));
            }
            CleanupTarget::Generation { generation_id }
                if !is_hash_identifier(generation_id)
                    || generation_id == &record.generation_id
                    || record.previous_generation_id.as_ref() == Some(generation_id) =>
            {
                return Err(publication_error(
                    "publication pointer contains an unsafe generation cleanup target",
                ));
            }
            CleanupTarget::LegacyOutput { .. } | CleanupTarget::Generation { .. } => {}
        }
    }
    Ok(())
}

fn truncate_pointer(path: &Path, length: u64) -> LabResult<()> {
    let mut file = OpenOptions::new().write(true).open(path).map_err(|error| {
        publication_error(format!(
            "failed to open incomplete publication pointer {}: {error}",
            path.display()
        ))
    })?;
    file.set_len(length).map_err(|error| {
        publication_error(format!(
            "failed to truncate incomplete publication pointer {}: {error}",
            path.display()
        ))
    })?;
    file.seek(SeekFrom::Start(length)).map_err(|error| {
        publication_error(format!(
            "failed to seek publication pointer {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        publication_error(format!(
            "failed to sync recovered publication pointer {}: {error}",
            path.display()
        ))
    })
}

fn next_sequence(previous: Option<&PointerRecord>) -> LabResult<u64> {
    previous
        .map(|record| record.sequence.checked_add(1))
        .unwrap_or(Some(1))
        .ok_or_else(|| publication_error("publication pointer sequence overflow"))
}

fn is_pointer_predecessor(previous: Option<&PointerRecord>, sequence: u64) -> bool {
    match previous {
        None => sequence == 1,
        Some(previous) => previous.sequence.checked_add(1) == Some(sequence),
    }
}

fn cleanup_targets(
    state_parent: &Path,
    authority_root: &Path,
    record: &PointerRecord,
    targets: &[CleanupTarget],
) -> LabResult<()> {
    for target in targets {
        match target {
            CleanupTarget::LegacyOutput { normalized_path } => {
                if !record.locked_outputs.contains(normalized_path) {
                    return Err(publication_error(format!(
                        "refusing to clean unowned legacy output: {normalized_path}"
                    )));
                }
                let path = PathBuf::from(normalized_path);
                let target_parent = path.parent().map(path_key).transpose()?;
                let state_parent_key = path_key(state_parent)?;
                if target_parent.as_deref() != Some(state_parent_key.as_str()) {
                    return Err(publication_error(format!(
                        "legacy cleanup target escapes publication directory: {normalized_path}"
                    )));
                }
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                        return Err(publication_error(format!(
                            "legacy cleanup target is a symlink or reparse point: {}",
                            path.display()
                        )));
                    }
                    Ok(metadata) if metadata.file_type().is_dir() => {
                        return Err(publication_error(format!(
                            "legacy cleanup target is a directory: {}",
                            path.display()
                        )));
                    }
                    Ok(_) => fs::remove_file(&path).map_err(|error| {
                        publication_error(format!(
                            "failed to remove legacy package output {}: {error}",
                            path.display()
                        ))
                    })?,
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(publication_error(format!(
                            "failed to inspect legacy cleanup target {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
            CleanupTarget::Generation { generation_id } => {
                if !is_hash_identifier(generation_id)
                    || generation_id == &record.generation_id
                    || record.previous_generation_id.as_ref() == Some(generation_id)
                {
                    return Err(publication_error(format!(
                        "refusing unsafe generation cleanup target: {generation_id}"
                    )));
                }
                let path = authority_root
                    .join(GENERATIONS_DIRECTORY)
                    .join(generation_id);
                match fs::symlink_metadata(&path) {
                    Ok(metadata)
                        if metadata.file_type().is_dir()
                            && !metadata_is_link_or_reparse(&metadata) =>
                    {
                        fs::remove_dir_all(&path).map_err(|error| {
                            publication_error(format!(
                                "failed to remove old package generation {}: {error}",
                                path.display()
                            ))
                        })?;
                    }
                    Ok(_) => {
                        return Err(publication_error(format!(
                            "generation cleanup target is not a regular directory: {}",
                            path.display()
                        )));
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(publication_error(format!(
                            "failed to inspect generation cleanup target {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn cleanup_sort_key(target: &CleanupTarget) -> String {
    serde_json::to_string(target).expect("cleanup target serialization is infallible")
}

fn create_lock_file(path: &Path, record: &PublicationLockRecord) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        let cleanup = fs::remove_file(path);
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(std::io::Error::other(format!(
                "{error}; lock cleanup failed: {cleanup_error}"
            ))),
        };
    }
    Ok(())
}

fn read_lock_record(path: &Path) -> LabResult<PublicationLockRecord> {
    read_lock_record_if_present(path)?.ok_or_else(|| {
        publication_error(format!(
            "publication lock does not exist: {}",
            path.display()
        ))
    })
}

fn read_lock_record_if_present(path: &Path) -> LabResult<Option<PublicationLockRecord>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(publication_error(format!(
                "failed to inspect publication lock {}: {error}",
                path.display()
            )));
        }
    };
    if !metadata.file_type().is_file() || metadata_is_link_or_reparse(&metadata) {
        return Err(publication_error(format!(
            "publication lock is not a regular file: {}",
            path.display()
        )));
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(publication_error(format!(
                "failed to read publication lock {}: {error}",
                path.display()
            )));
        }
    };
    let record: PublicationLockRecord = serde_json::from_slice(&bytes).map_err(|error| {
        publication_error(format!(
            "failed to parse publication lock {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(record))
}

fn validate_lock_record(record: &PublicationLockRecord, expected_lock_key: &str) -> LabResult<()> {
    let normalized = record
        .normalized_outputs
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if record.schema_version != LOCK_SCHEMA
        || !is_hash_identifier(&record.owner_token)
        || record.process_start_token.is_empty()
        || record.pid == 0
        || record.acquired_unix_ms == 0
        || !is_hash_identifier(&record.output_set_digest)
        || record.normalized_outputs.is_empty()
        || record.normalized_outputs.iter().any(String::is_empty)
        || normalized.len() != record.normalized_outputs.len()
        || digest_output_set(&normalized) != record.output_set_digest
        || record.lock_key.is_empty()
        || record.lock_key != expected_lock_key
    {
        return Err(publication_error(
            "publication lock record is corrupt or belongs to a colliding authority",
        ));
    }
    Ok(())
}

fn reclaim_stale_lock(
    path: &Path,
    observed: &PublicationLockRecord,
    reclaimer_token: &str,
) -> LabResult<bool> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(publication_error(format!(
                "failed to re-read stale publication lock {}: {error}",
                path.display()
            )));
        }
    };
    let current: PublicationLockRecord =
        serde_json::from_slice(&bytes).map_err(json_publication_error)?;
    if current != *observed {
        return Ok(false);
    }
    let tombstone = path.with_extension(format!("reclaim-{reclaimer_token}"));
    match fs::rename(path, &tombstone) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(publication_error(format!(
                "failed to claim stale publication lock {}: {error}",
                path.display()
            )));
        }
    }
    let moved = read_lock_record(&tombstone)?;
    if moved != *observed {
        return Err(publication_error(format!(
            "publication lock changed during stale recovery: {}",
            tombstone.display()
        )));
    }
    fs::remove_file(&tombstone).map_err(|error| {
        publication_error(format!(
            "failed to remove stale publication lock tombstone {}: {error}",
            tombstone.display()
        ))
    })?;
    Ok(true)
}

fn release_locks(locks: &mut Vec<PublicationLock>) -> LabResult<()> {
    let mut failure = None;
    while let Some(lock) = locks.pop() {
        if let Err(error) = lock.release() {
            failure = Some(match failure {
                Some(primary) => combine_errors(primary, error),
                None => error,
            });
        }
    }
    failure.map_or(Ok(()), Err)
}

fn combine_with_lock_release(primary: LabError, locks: &mut Vec<PublicationLock>) -> LabError {
    match release_locks(locks) {
        Ok(()) => primary,
        Err(release) => combine_errors(primary, release),
    }
}

fn combine_errors(primary: LabError, secondary: LabError) -> LabError {
    publication_error(format!(
        "{}; secondary_failure={}",
        primary.message, secondary.message
    ))
}

fn hash_file(path: &Path) -> LabResult<(u64, String)> {
    let mut file = File::open(path).map_err(|error| {
        publication_error(format!(
            "failed to open {} for hashing: {error}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut byte_count = 0u64;
    let mut buffer = [0u8; HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            publication_error(format!("failed to hash {}: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        byte_count = byte_count
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| publication_error("package output chunk length exceeds u64"))?,
            )
            .ok_or_else(|| publication_error("package output length overflow"))?;
        hasher.update(&buffer[..read]);
    }
    Ok((byte_count, format!("{:x}", hasher.finalize())))
}

fn write_new_synced_file(path: &Path, bytes: &[u8]) -> LabResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            publication_error(format!("failed to create {}: {error}", path.display()))
        })?;
    file.write_all(bytes).map_err(|error| {
        publication_error(format!("failed to write {}: {error}", path.display()))
    })?;
    file.flush().map_err(|error| {
        publication_error(format!("failed to flush {}: {error}", path.display()))
    })?;
    file.sync_all()
        .map_err(|error| publication_error(format!("failed to sync {}: {error}", path.display())))
}

fn is_hash_identifier(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn require_regular_directory(path: &Path, label: &str) -> LabResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        publication_error(format!(
            "failed to inspect {label} {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_dir() || metadata_is_link_or_reparse(&metadata) {
        return Err(publication_error(format!(
            "{label} is not a regular directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_regular_file(path: &Path, label: &str) -> LabResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        publication_error(format!(
            "failed to inspect {label} {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() || metadata_is_link_or_reparse(&metadata) {
        return Err(publication_error(format!(
            "{label} is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || metadata_is_reparse_point(metadata)
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn publication_error(message: impl Into<String>) -> LabError {
    LabError::package_invalid(message)
}

fn json_publication_error(error: serde_json::Error) -> LabError {
    publication_error(format!("invalid publication metadata JSON: {error}"))
}

#[cfg(windows)]
fn inspect_system_process(pid: u32) -> Result<ProcessStatus, String> {
    let script = format!(
        "$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue; if ($null -eq $p) {{ exit 3 }}; try {{ [Console]::Out.Write($p.StartTime.ToUniversalTime().Ticks.ToString([Globalization.CultureInfo]::InvariantCulture)); exit 0 }} catch {{ [Console]::Error.Write($_.Exception.Message); exit 4 }}"
    );
    let output = Command::new(system_powershell_path()?)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|error| format!("failed to inspect process {pid}: {error}"))?;
    match output.status.code() {
        Some(0) => {
            let token = String::from_utf8(output.stdout)
                .map_err(|error| format!("process start token was not UTF-8: {error}"))?;
            let token = token.trim().to_string();
            if token.is_empty() {
                Err(format!("process {pid} returned an empty start token"))
            } else {
                Ok(ProcessStatus::Alive { start_token: token })
            }
        }
        Some(3) => Ok(ProcessStatus::Dead),
        _ => Err(format!(
            "failed to inspect process {pid}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

#[cfg(target_os = "linux")]
fn inspect_system_process(pid: u32) -> Result<ProcessStatus, String> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(ProcessStatus::Dead),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let end = stat
        .rfind(')')
        .ok_or_else(|| format!("invalid process stat for {pid}"))?;
    let fields = stat[end + 1..].split_whitespace().collect::<Vec<_>>();
    let start_token = fields
        .get(19)
        .ok_or_else(|| format!("process stat for {pid} lacks start time"))?
        .to_string();
    Ok(ProcessStatus::Alive { start_token })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn inspect_system_process(pid: u32) -> Result<ProcessStatus, String> {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| format!("failed to inspect process {pid}: {error}"))?;
    if output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty() {
        return Ok(ProcessStatus::Dead);
    }
    if !output.status.success() {
        return Err(format!(
            "failed to inspect process {pid}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let token = String::from_utf8(output.stdout)
        .map_err(|error| format!("process start token was not UTF-8: {error}"))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        Err(format!("process {pid} returned an empty start token"))
    } else {
        Ok(ProcessStatus::Alive { start_token: token })
    }
}

#[cfg(not(any(windows, unix)))]
fn inspect_system_process(_pid: u32) -> Result<ProcessStatus, String> {
    Err("process identity inspection is unsupported on this platform".to_string())
}

#[cfg(windows)]
fn system_random_seed() -> Result<[u8; 32], String> {
    let script = "$bytes=New-Object byte[] 32; $rng=[Security.Cryptography.RandomNumberGenerator]::Create(); try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }; [Console]::Out.Write((($bytes | ForEach-Object { $_.ToString('x2') }) -join ''))";
    let output = Command::new(system_powershell_path()?)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|error| format!("failed to start secure random provider: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "secure random provider failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let hex = String::from_utf8(output.stdout)
        .map_err(|error| format!("secure random output was not UTF-8: {error}"))?;
    parse_seed_hex(hex.trim())
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

#[cfg(windows)]
fn parse_seed_hex(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("random source did not return 32 hexadecimal bytes".to_string());
    }
    let mut seed = [0u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|error| format!("invalid random source output: {error}"))?;
    }
    Ok(seed)
}

#[cfg(windows)]
fn system_powershell_path() -> Result<PathBuf, String> {
    let root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .ok_or_else(|| "SystemRoot and WINDIR are unavailable".to_string())?;
    let path = PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "Windows PowerShell executable was not found at {}",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use tempfile::TempDir;

    #[derive(Clone)]
    struct FakeEnvironment {
        inner: Arc<FakeEnvironmentInner>,
    }

    struct FakeEnvironmentInner {
        current: Mutex<ProcessIdentity>,
        statuses: Mutex<BTreeMap<u32, Result<ProcessStatus, String>>>,
        counter: AtomicU64,
        now: AtomicU64,
    }

    impl FakeEnvironment {
        fn new(pid: u32, start_token: &str) -> Self {
            let current = ProcessIdentity {
                pid,
                start_token: start_token.to_string(),
            };
            let mut statuses = BTreeMap::new();
            statuses.insert(
                pid,
                Ok(ProcessStatus::Alive {
                    start_token: start_token.to_string(),
                }),
            );
            Self {
                inner: Arc::new(FakeEnvironmentInner {
                    current: Mutex::new(current),
                    statuses: Mutex::new(statuses),
                    counter: AtomicU64::new(1),
                    now: AtomicU64::new(1_700_000_000_000),
                }),
            }
        }

        fn set_current(&self, pid: u32, start_token: &str) {
            *self.inner.current.lock().unwrap() = ProcessIdentity {
                pid,
                start_token: start_token.to_string(),
            };
            self.set_status(
                pid,
                Ok(ProcessStatus::Alive {
                    start_token: start_token.to_string(),
                }),
            );
        }

        fn set_status(&self, pid: u32, status: Result<ProcessStatus, String>) {
            self.inner.statuses.lock().unwrap().insert(pid, status);
        }
    }

    impl PublicationEnvironment for FakeEnvironment {
        fn current_process(&self) -> Result<ProcessIdentity, String> {
            Ok(self.inner.current.lock().unwrap().clone())
        }

        fn inspect_process(&self, pid: u32) -> Result<ProcessStatus, String> {
            self.inner
                .statuses
                .lock()
                .unwrap()
                .get(&pid)
                .cloned()
                .unwrap_or(Ok(ProcessStatus::Dead))
        }

        fn random_seed(&self) -> Result<[u8; 32], String> {
            let value = self.inner.counter.fetch_add(1, Ordering::SeqCst);
            let mut seed = [0u8; 32];
            seed[..8].copy_from_slice(&value.to_be_bytes());
            Ok(seed)
        }

        fn now_unix_ms(&self) -> Result<u128, String> {
            Ok(u128::from(self.inner.now.fetch_add(1, Ordering::SeqCst)))
        }
    }

    #[test]
    fn publication_faults_expose_only_complete_old_or_new_generations() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(101, "start-a");
        let single = temp.path().join("single.zip");
        publish_single(&env, &single, b"old");

        let before_first = PackagePublicationTransaction::begin_single_with(&single, &env)
            .unwrap()
            .with_fault(PublicationFaultPoint::BeforeFirstFile);
        assert!(
            before_first
                .checkpoint(PublicationFaultPoint::BeforeFirstFile)
                .is_err()
        );
        before_first.abort().unwrap();
        assert_eq!(read_visible(&single), b"old");

        let before_commit = PackagePublicationTransaction::begin_single_with(&single, &env)
            .unwrap()
            .with_fault(PublicationFaultPoint::BeforeCommit);
        stage(&before_commit, &single, b"not-committed");
        assert!(before_commit.commit().is_err());
        assert_eq!(read_visible(&single), b"old");

        let partial_pointer = PackagePublicationTransaction::begin_single_with(&single, &env)
            .unwrap()
            .with_fault(PublicationFaultPoint::PointerPartialWrite);
        stage(&partial_pointer, &single, b"partial-pointer");
        assert!(partial_pointer.commit().is_err());
        assert_eq!(read_visible(&single), b"old");

        let reported_failure = PackagePublicationTransaction::begin_single_with(&single, &env)
            .unwrap()
            .with_fault(PublicationFaultPoint::PointerWriteReportedFailure);
        stage(&reported_failure, &single, b"visible-despite-error");
        let error = reported_failure
            .commit()
            .expect_err("visible pointer failure must remain committed");
        assert!(error.message.contains("committed_generation="));
        assert_eq!(read_visible(&single), b"visible-despite-error");

        let after_commit = PackagePublicationTransaction::begin_single_with(&single, &env)
            .unwrap()
            .with_fault(PublicationFaultPoint::AfterCommit);
        stage(&after_commit, &single, b"new-after-commit");
        let error = after_commit.commit().expect_err("after-commit fault");
        assert!(error.message.contains("committed_generation="));
        assert_eq!(read_visible(&single), b"new-after-commit");

        let group_dir = temp.path().join("group");
        fs::create_dir_all(&group_dir).unwrap();
        let left = group_dir.join("left.zip");
        let right = group_dir.join("right.zip");
        publish_group(
            &env,
            &group_dir,
            &[(&left, b"old-left"), (&right, b"old-right")],
        );
        let middle = PackagePublicationTransaction::begin_group_with(
            &group_dir,
            &[left.clone(), right.clone()],
            &env,
        )
        .unwrap()
        .with_fault(PublicationFaultPoint::MidFiles);
        stage(&middle, &left, b"partial-left");
        assert!(middle.checkpoint(PublicationFaultPoint::MidFiles).is_err());
        middle.abort().unwrap();
        assert_eq!(read_visible(&left), b"old-left");
        assert_eq!(read_visible(&right), b"old-right");
    }

    #[test]
    fn cleanup_failure_is_nonzero_and_recovery_keeps_committed_generation() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(102, "start-a");
        let output = temp.path().join("legacy.zip");
        fs::write(&output, b"legacy").unwrap();
        let transaction = PackagePublicationTransaction::begin_single_with(&output, &env)
            .unwrap()
            .with_fault(PublicationFaultPoint::Cleanup);
        stage(&transaction, &output, b"committed");
        let error = transaction.commit().expect_err("cleanup fault must fail");
        assert!(error.message.contains("committed_generation="));
        assert!(error.message.contains("pending_cleanup="));
        assert_eq!(read_visible(&output), b"committed");
        assert!(
            output.exists(),
            "injected cleanup leaves legacy output pending"
        );

        let recovery = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        recovery.abort().unwrap();
        assert!(!output.exists());
        assert_eq!(read_visible(&output), b"committed");
    }

    #[test]
    fn pointer_partial_tail_is_ignored_then_truncated_under_lock() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(103, "start-a");
        let output = temp.path().join("package.zip");
        let first = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        let pointer = first.pointer_path.clone();
        stage(&first, &output, b"old");
        first.commit().unwrap();
        OpenOptions::new()
            .append(true)
            .open(&pointer)
            .unwrap()
            .write_all(br#"{"record":{"sequence":2"#)
            .unwrap();
        assert_eq!(read_visible(&output), b"old");

        let second = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        stage(&second, &output, b"new");
        second.commit().unwrap();
        assert_eq!(read_visible(&output), b"new");
        assert!(fs::read(&pointer).unwrap().ends_with(b"\n"));
    }

    #[test]
    fn uncommitted_generation_is_not_visible_to_consumers() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(104, "start-a");
        let output = temp.path().join("package.zip");
        let transaction = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        stage(&transaction, &output, b"uncommitted");
        assert_eq!(resolve_published_package_path(&output).unwrap(), output);
        assert!(!output.exists());
        transaction.abort().unwrap();
    }

    #[test]
    fn same_output_conflicts_while_distinct_outputs_do_not() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(105, "start-a");
        let first_path = temp.path().join("first.zip");
        let second_path = temp.path().join("second.zip");
        let first = PackagePublicationTransaction::begin_single_with(&first_path, &env).unwrap();
        let conflict = PackagePublicationTransaction::begin_single_with(&first_path, &env)
            .expect_err("same output must conflict");
        assert!(conflict.message.contains("locked by a live owner"));
        let second = PackagePublicationTransaction::begin_single_with(&second_path, &env).unwrap();
        first.abort().unwrap();
        second.abort().unwrap();
        assert_ne!(
            digest_text(&output_lock_key("C:/a-b.zip")),
            digest_text(&output_lock_key("C:/a_b.zip"))
        );
    }

    #[test]
    fn one_logical_output_cannot_be_claimed_by_two_authorities() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(117, "start-a");
        let output = temp.path().join("package.zip");
        publish_single(&env, &output, b"single");

        let error =
            PackagePublicationTransaction::begin_group_with(temp.path(), &[output.clone()], &env)
                .expect_err("group authority must not duplicate a single-output claim");
        assert!(
            error
                .message
                .contains("already owned by another publication authority")
        );
        assert_eq!(read_visible(&output), b"single");
    }

    #[test]
    fn lock_record_binds_owner_process_time_and_output_set() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(106, "process-start");
        let output = temp.path().join("package.zip");
        let transaction = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        for lock in &transaction.locks {
            let record = read_lock_record(&lock.path).unwrap();
            assert_eq!(record.pid, 106);
            assert_eq!(record.process_start_token, "process-start");
            assert_eq!(record.owner_token, lock.owner_token);
            assert!(record.acquired_unix_ms > 0);
            assert_eq!(record.output_set_digest, transaction.lock_set_digest);
            assert_eq!(record.normalized_outputs.len(), 1);
        }
        transaction.abort().unwrap();
    }

    #[test]
    fn committed_pointer_separates_visible_outputs_from_temporary_lock_scope() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(114, "start-a");
        let left = temp.path().join("left.zip");
        let right = temp.path().join("right.zip");
        publish_group(
            &env,
            temp.path(),
            &[(&left, b"old-left"), (&right, b"old-right")],
        );

        let transaction =
            PackagePublicationTransaction::begin_group_with(temp.path(), &[left.clone()], &env)
                .unwrap();
        let pointer = transaction.pointer_path.clone();
        stage(&transaction, &left, b"new-left");
        transaction.commit().unwrap();

        let record = read_pointer_log(&pointer).unwrap().last.unwrap();
        let state_parent = fs::canonicalize(temp.path()).unwrap();
        let expected =
            BTreeSet::from([normalize_output_for_existing_parent(&left, &state_parent).unwrap()]);
        assert_eq!(
            record.outputs.keys().cloned().collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(
            record.locked_outputs,
            expected.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(record.output_set_digest, digest_output_set(&expected));
        assert_eq!(
            record.transaction_output_set_digest,
            digest_output_set(&expected)
        );
        assert_eq!(read_visible(&left), b"new-left");
        assert_eq!(resolve_published_package_path(&right).unwrap(), right);
    }

    #[test]
    fn publication_retains_only_current_and_previous_complete_generations() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(119, "start-a");
        let output = temp.path().join("package.zip");
        publish_single(&env, &output, b"first");
        publish_single(&env, &output, b"second");
        let transaction = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        let generations = transaction.authority_root.join(GENERATIONS_DIRECTORY);
        stage(&transaction, &output, b"third");
        transaction.commit().unwrap();

        let retained = fs::read_dir(generations)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 2);
        assert!(retained.iter().all(|path| path.is_dir()));
        assert_eq!(read_visible(&output), b"third");
    }

    #[test]
    fn corrupt_lock_is_not_silently_reclaimed() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(115, "old-start");
        let output = temp.path().join("package.zip");
        let abandoned = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        let lock = abandoned.locks[0].path.clone();
        drop(abandoned);
        fs::write(&lock, b"{}\n").unwrap();
        env.set_status(115, Ok(ProcessStatus::Dead));
        env.set_current(116, "new-start");

        let error = PackagePublicationTransaction::begin_single_with(&output, &env)
            .expect_err("corrupt lock must block recovery");
        assert!(error.message.contains("failed to parse publication lock"));
        assert!(error.message.contains(lock.to_string_lossy().as_ref()));
        assert!(lock.exists());
    }

    #[test]
    fn pid_reuse_is_reclaimed_but_unknown_owner_is_not() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(107, "old-start");
        let reused = temp.path().join("reused.zip");
        let abandoned = PackagePublicationTransaction::begin_single_with(&reused, &env).unwrap();
        drop(abandoned);
        env.set_status(
            107,
            Ok(ProcessStatus::Alive {
                start_token: "new-start".to_string(),
            }),
        );
        env.set_current(108, "current-start");
        let recovered = PackagePublicationTransaction::begin_single_with(&reused, &env).unwrap();
        recovered.abort().unwrap();

        let unknown = temp.path().join("unknown.zip");
        env.set_current(109, "unknown-owner");
        let abandoned = PackagePublicationTransaction::begin_single_with(&unknown, &env).unwrap();
        drop(abandoned);
        env.set_status(109, Err("access denied".to_string()));
        env.set_current(110, "new-owner");
        let error = PackagePublicationTransaction::begin_single_with(&unknown, &env)
            .expect_err("unknown owner state must not be reclaimed");
        assert!(
            error
                .message
                .contains("cannot confirm publication lock owner death")
        );
        env.set_status(109, Ok(ProcessStatus::Dead));
        let cleanup = PackagePublicationTransaction::begin_single_with(&unknown, &env).unwrap();
        cleanup.abort().unwrap();
    }

    #[test]
    fn concurrent_stale_reclaim_has_one_winner() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(111, "old-start");
        let output = temp.path().join("package.zip");
        let abandoned = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        drop(abandoned);
        env.set_status(111, Ok(ProcessStatus::Dead));
        env.set_current(112, "new-start");

        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let env = env.clone();
            let output = output.clone();
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                let result = PackagePublicationTransaction::begin_single_with(&output, &env);
                let won = result.is_ok();
                barrier.wait();
                if let Ok(transaction) = result {
                    transaction.abort().unwrap();
                }
                won
            }));
        }
        barrier.wait();
        let winners = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
    }

    #[test]
    fn corrupt_complete_pointer_and_generation_corruption_fail_loud() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(113, "start-a");
        let output = temp.path().join("package.zip");
        let transaction = PackagePublicationTransaction::begin_single_with(&output, &env).unwrap();
        let pointer = transaction.pointer_path.clone();
        stage(&transaction, &output, b"complete");
        let commit = transaction.commit().unwrap();
        OpenOptions::new()
            .append(true)
            .open(&pointer)
            .unwrap()
            .write_all(b"{}\n")
            .unwrap();
        assert!(resolve_published_package_path(&output).is_err());

        let bytes = fs::read(&pointer).unwrap();
        let valid_len = bytes[..bytes.len() - 3]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .unwrap()
            + 1;
        OpenOptions::new()
            .write(true)
            .open(&pointer)
            .unwrap()
            .set_len(u64::try_from(valid_len).unwrap())
            .unwrap();
        let resolved = commit.resolved_outputs.values().next().unwrap();
        fs::write(resolved, b"tampered").unwrap();
        assert!(resolve_published_package_path(&output).is_err());
    }

    #[test]
    fn unrelated_authority_corruption_does_not_block_a_distinct_output() {
        let temp = TempDir::new().unwrap();
        let env = FakeEnvironment::new(118, "start-a");
        let healthy = temp.path().join("healthy.zip");
        let corrupt = temp.path().join("corrupt.zip");
        publish_single(&env, &healthy, b"healthy");
        let transaction = PackagePublicationTransaction::begin_single_with(&corrupt, &env).unwrap();
        let pointer = transaction.pointer_path.clone();
        stage(&transaction, &corrupt, b"corrupt");
        transaction.commit().unwrap();
        OpenOptions::new()
            .append(true)
            .open(pointer)
            .unwrap()
            .write_all(b"{}\n")
            .unwrap();

        assert_eq!(read_visible(&healthy), b"healthy");
        assert!(resolve_published_package_path(&corrupt).is_err());
    }

    #[test]
    fn process_crash_leaves_old_visibility_and_is_recoverable() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("crash.zip");
        fs::write(&output, b"old-legacy").unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "package_publish::tests::process_crash_child_entry",
                "--nocapture",
            ])
            .env("ACTINGCOMMAND_PUBLICATION_CRASH_CHILD", &output)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        assert_eq!(fs::read(&output).unwrap(), b"old-legacy");

        let recovery = PackagePublicationTransaction::begin_single(&output).unwrap();
        stage(&recovery, &output, b"recovered");
        recovery.commit().unwrap();
        assert_eq!(read_visible(&output), b"recovered");
    }

    #[test]
    fn process_crash_after_pointer_commit_keeps_new_visibility_and_recovers_cleanup() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("crash-after-commit.zip");
        fs::write(&output, b"old-legacy").unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "package_publish::tests::process_crash_child_entry",
                "--nocapture",
            ])
            .env("ACTINGCOMMAND_PUBLICATION_CRASH_CHILD", &output)
            .env("ACTINGCOMMAND_PUBLICATION_CRASH_PHASE", "after_commit")
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(74));
        assert_eq!(read_visible(&output), b"committed-before-crash");
        assert!(output.exists());

        let recovery = PackagePublicationTransaction::begin_single(&output).unwrap();
        recovery.abort().unwrap();
        assert!(!output.exists());
        assert_eq!(read_visible(&output), b"committed-before-crash");
    }

    #[test]
    fn process_crash_child_entry() {
        let Ok(output) = std::env::var("ACTINGCOMMAND_PUBLICATION_CRASH_CHILD") else {
            return;
        };
        let output = PathBuf::from(output);
        if std::env::var("ACTINGCOMMAND_PUBLICATION_CRASH_PHASE")
            .ok()
            .as_deref()
            == Some("after_commit")
        {
            let transaction = PackagePublicationTransaction::begin_single(&output)
                .unwrap()
                .with_fault(PublicationFaultPoint::ProcessExitAfterCommit);
            stage(&transaction, &output, b"committed-before-crash");
            transaction.commit().unwrap();
            unreachable!("after-commit crash fault must terminate the process");
        }
        let transaction = PackagePublicationTransaction::begin_single(&output).unwrap();
        stage(&transaction, &output, b"partial");
        std::process::exit(73);
    }

    fn publish_single(env: &FakeEnvironment, output: &Path, bytes: &[u8]) {
        let transaction = PackagePublicationTransaction::begin_single_with(output, env).unwrap();
        stage(&transaction, output, bytes);
        transaction.commit().unwrap();
    }

    fn publish_group(env: &FakeEnvironment, directory: &Path, files: &[(&Path, &[u8])]) {
        let outputs = files
            .iter()
            .map(|(path, _)| path.to_path_buf())
            .collect::<Vec<_>>();
        let transaction =
            PackagePublicationTransaction::begin_group_with(directory, &outputs, env).unwrap();
        for (path, bytes) in files {
            stage(&transaction, path, bytes);
        }
        transaction.commit().unwrap();
    }

    fn stage(transaction: &PackagePublicationTransaction, output: &Path, bytes: &[u8]) {
        let staged = transaction.staging_path(output).unwrap();
        fs::write(staged, bytes).unwrap();
    }

    fn read_visible(output: &Path) -> Vec<u8> {
        let resolved = resolve_published_package_path(output).unwrap();
        fs::read(resolved).unwrap()
    }
}
