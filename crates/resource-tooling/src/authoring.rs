// SPDX-License-Identifier: AGPL-3.0-only

//! Typed resource-authoring plans and transactional publication.
//!
//! This module owns filesystem publication but has no Runtime, scheduler, device, or ledger
//! storage authority. Callers supply a validator and an event sink so critical intent can become
//! durable before the canonical resource tree changes.

use actingcommand_contract::{LabError, LabResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Provenance retained by an authoring receipt without embedding local artifact paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringProvenance {
    pub record_id: String,
    pub source_artifact_ids: Vec<String>,
}

/// Source bytes for one planned authoring file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthoringFileSource {
    Bytes(Vec<u8>),
    Copy {
        source: PathBuf,
        artifact_id: String,
    },
}

/// How a planned file interacts with a pre-existing candidate tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoringWriteMode {
    Replace,
    CreateIfMissing,
}

/// One validated relative file operation in an authoring draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoringFile {
    relative_path: PathBuf,
    source: AuthoringFileSource,
    mode: AuthoringWriteMode,
}

impl AuthoringFile {
    pub fn bytes(
        relative_path: impl Into<PathBuf>,
        bytes: Vec<u8>,
        mode: AuthoringWriteMode,
    ) -> LabResult<Self> {
        Self::new(
            relative_path.into(),
            AuthoringFileSource::Bytes(bytes),
            mode,
        )
    }

    pub fn copy(
        relative_path: impl Into<PathBuf>,
        source: impl Into<PathBuf>,
        artifact_id: impl Into<String>,
        mode: AuthoringWriteMode,
    ) -> LabResult<Self> {
        let artifact_id = non_empty("artifact_id", artifact_id.into())?;
        Self::new(
            relative_path.into(),
            AuthoringFileSource::Copy {
                source: source.into(),
                artifact_id,
            },
            mode,
        )
    }

    fn new(
        relative_path: PathBuf,
        source: AuthoringFileSource,
        mode: AuthoringWriteMode,
    ) -> LabResult<Self> {
        validate_relative_path(&relative_path, "authoring file")?;
        Ok(Self {
            relative_path,
            source,
            mode,
        })
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub const fn mode(&self) -> AuthoringWriteMode {
        self.mode
    }
}

/// Immutable file plan produced by the Lab authoring workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoringDraft {
    correlation_id: String,
    draft_id: String,
    replace_scope: PathBuf,
    files: Vec<AuthoringFile>,
    provenance: AuthoringProvenance,
}

impl AuthoringDraft {
    pub fn new(
        correlation_id: impl Into<String>,
        draft_id: impl Into<String>,
        replace_scope: impl Into<PathBuf>,
        files: Vec<AuthoringFile>,
        provenance: AuthoringProvenance,
    ) -> LabResult<Self> {
        let correlation_id = non_empty("correlation_id", correlation_id.into())?;
        let draft_id = non_empty("draft_id", draft_id.into())?;
        let replace_scope = replace_scope.into();
        validate_relative_path(&replace_scope, "replace_scope")?;
        non_empty("record_id", provenance.record_id.clone())?;
        if files.is_empty() {
            return Err(authoring_error(
                "authoring_draft_empty",
                "authoring draft must contain at least one file",
            ));
        }

        let mut paths = BTreeSet::new();
        for file in &files {
            if !paths.insert(file.relative_path.clone()) {
                return Err(authoring_error(
                    "authoring_path_conflict",
                    format!(
                        "authoring draft contains duplicate path {}",
                        file.relative_path.display()
                    ),
                ));
            }
            if file.mode == AuthoringWriteMode::Replace
                && !file.relative_path.starts_with(&replace_scope)
            {
                return Err(authoring_error(
                    "authoring_scope_escape",
                    format!(
                        "replace file {} is outside replace scope {}",
                        file.relative_path.display(),
                        replace_scope.display()
                    ),
                ));
            }
        }

        Ok(Self {
            correlation_id,
            draft_id,
            replace_scope,
            files,
            provenance,
        })
    }

    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    pub fn draft_id(&self) -> &str {
        &self.draft_id
    }

    pub fn replace_scope(&self) -> &Path {
        &self.replace_scope
    }

    pub fn files(&self) -> &[AuthoringFile] {
        &self.files
    }

    pub fn provenance(&self) -> &AuthoringProvenance {
        &self.provenance
    }
}

/// Filesystem target and conflict policy for an explicit publish request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoringPublishRequest {
    pub target_root: PathBuf,
    pub target_label: String,
    pub force: bool,
}

/// Validator output retained in the terminal publish receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringValidationReport {
    pub checks: Vec<String>,
}

impl AuthoringValidationReport {
    pub fn new(checks: Vec<String>) -> LabResult<Self> {
        if checks.is_empty() || checks.iter().any(|check| check.trim().is_empty()) {
            return Err(authoring_error(
                "authoring_validation_empty",
                "authoring validation must report at least one named check",
            ));
        }
        Ok(Self { checks })
    }
}

/// Hash record for one file in the fully validated candidate tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringFileHash {
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// Terminal receipt returned only after publication and outcome durability both succeed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringReceipt {
    pub correlation_id: String,
    pub draft_id: String,
    pub target_label: String,
    pub target_fingerprint: String,
    pub changed_paths: Vec<String>,
    pub file_hashes: Vec<AuthoringFileHash>,
    pub validation: AuthoringValidationReport,
    pub provenance: AuthoringProvenance,
}

/// Typed lifecycle events emitted by the publisher through caller-owned durable ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoringEventKind {
    AuthoringStarted,
    DraftBuilt,
    ValidationCompleted,
    PromoteIntent,
    Promoted,
    PromoteFailed,
}

/// Redacted authoring event payload. Raw local paths never enter this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringEvent {
    pub kind: AuthoringEventKind,
    pub correlation_id: String,
    pub draft_id: String,
    pub target_label: String,
    pub target_fingerprint: String,
    pub changed_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

/// Durable authoring-event ingress supplied by the Lab/Runtime composition boundary.
pub trait AuthoringEventSink {
    fn append(&mut self, event: &AuthoringEvent) -> LabResult<()>;
}

impl<F> AuthoringEventSink for F
where
    F: FnMut(&AuthoringEvent) -> LabResult<()>,
{
    fn append(&mut self, event: &AuthoringEvent) -> LabResult<()> {
        self(event)
    }
}

/// Candidate-tree validation supplied by the deterministic authoring composition.
pub trait AuthoringValidator {
    fn validate(
        &mut self,
        candidate_root: &Path,
        draft: &AuthoringDraft,
    ) -> LabResult<AuthoringValidationReport>;
}

/// Resolution supplied by the durable authoring fact source for an interrupted candidate swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoringRecoveryDecision {
    CommitCandidate,
    RollbackCandidate,
}

/// Stable identity and candidate digest used to reconcile a filesystem journal with durable facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoringRecoveryContext {
    correlation_id: String,
    draft_id: String,
    target_label: String,
    target_fingerprint: String,
    candidate_tree_sha256: String,
}

impl AuthoringRecoveryContext {
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    pub fn draft_id(&self) -> &str {
        &self.draft_id
    }

    pub fn target_label(&self) -> &str {
        &self.target_label
    }

    pub fn target_fingerprint(&self) -> &str {
        &self.target_fingerprint
    }

    pub fn candidate_tree_sha256(&self) -> &str {
        &self.candidate_tree_sha256
    }
}

/// Durable recovery authority supplied by the Lab/Runtime composition boundary.
pub trait AuthoringRecoveryOracle {
    fn decide(
        &mut self,
        context: &AuthoringRecoveryContext,
    ) -> LabResult<AuthoringRecoveryDecision>;
}

impl<F> AuthoringRecoveryOracle for F
where
    F: FnMut(&AuthoringRecoveryContext) -> LabResult<AuthoringRecoveryDecision>,
{
    fn decide(
        &mut self,
        context: &AuthoringRecoveryContext,
    ) -> LabResult<AuthoringRecoveryDecision> {
        self(context)
    }
}

impl<F> AuthoringValidator for F
where
    F: FnMut(&Path, &AuthoringDraft) -> LabResult<AuthoringValidationReport>,
{
    fn validate(
        &mut self,
        candidate_root: &Path,
        draft: &AuthoringDraft,
    ) -> LabResult<AuthoringValidationReport> {
        self(candidate_root, draft)
    }
}

/// Publish a complete candidate tree using same-volume staging and rollback.
pub fn publish_authoring_draft(
    request: &AuthoringPublishRequest,
    draft: &AuthoringDraft,
    validator: &mut dyn AuthoringValidator,
    events: &mut dyn AuthoringEventSink,
) -> LabResult<AuthoringReceipt> {
    let mut recovery = |_context: &AuthoringRecoveryContext| {
        Err(authoring_error(
            "authoring_recovery_oracle_required",
            "an interrupted candidate swap requires a durable recovery oracle",
        ))
    };
    publish_authoring_draft_inner(request, draft, &mut recovery, validator, events, None)
}

/// Publish with a caller-owned durable oracle for reconciling interrupted candidate swaps.
pub fn publish_authoring_draft_with_recovery(
    request: &AuthoringPublishRequest,
    draft: &AuthoringDraft,
    recovery: &mut dyn AuthoringRecoveryOracle,
    validator: &mut dyn AuthoringValidator,
    events: &mut dyn AuthoringEventSink,
) -> LabResult<AuthoringReceipt> {
    publish_authoring_draft_inner(request, draft, recovery, validator, events, None)
}

/// Materialize a developer draft without publishing it or emitting Runtime events.
pub fn materialize_authoring_draft(target_root: &Path, draft: &AuthoringDraft) -> LabResult<()> {
    let parent = target_root.parent().ok_or_else(|| {
        authoring_error(
            "authoring_target_has_no_parent",
            "materialization target must have a parent directory",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        io_error(
            "authoring_target_parent_create_failed",
            "create materialization target parent",
            parent,
            error,
        )
    })?;
    let target_root = resolve_publish_target(target_root)?;
    if !target_root.exists() {
        fs::create_dir(&target_root).map_err(|error| {
            io_error(
                "authoring_target_create_failed",
                "create materialization target",
                &target_root,
                error,
            )
        })?;
    }
    reject_materialization_symlinks(&target_root, draft)?;
    apply_draft(&target_root, draft)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    AfterOldTreeMoved,
}

fn publish_authoring_draft_inner(
    request: &AuthoringPublishRequest,
    draft: &AuthoringDraft,
    recovery: &mut dyn AuthoringRecoveryOracle,
    validator: &mut dyn AuthoringValidator,
    events: &mut dyn AuthoringEventSink,
    fault: Option<FaultPoint>,
) -> LabResult<AuthoringReceipt> {
    let target_label = non_empty("target_label", request.target_label.clone())?;
    let target_root = resolve_publish_target(&request.target_root)?;
    let parent = target_root.parent().ok_or_else(|| {
        authoring_error(
            "authoring_target_has_no_parent",
            "publish target must have a parent directory",
        )
    })?;
    let target_name = target_root.file_name().ok_or_else(|| {
        authoring_error(
            "authoring_target_has_no_name",
            "publish target must have a final path component",
        )
    })?;
    let target_fingerprint = path_fingerprint(&target_root);
    let changed_paths = draft
        .files
        .iter()
        .map(|file| slash_path(&file.relative_path))
        .collect::<Vec<_>>();
    let event = |kind, failure_code| AuthoringEvent {
        kind,
        correlation_id: draft.correlation_id.clone(),
        draft_id: draft.draft_id.clone(),
        target_label: target_label.clone(),
        target_fingerprint: target_fingerprint.clone(),
        changed_paths: changed_paths.clone(),
        failure_code,
    };

    events.append(&event(AuthoringEventKind::AuthoringStarted, None))?;

    let result = (|| {
        let journal_path = parent.join(transaction_journal_name(&target_root));
        recover_transaction(&target_root, &journal_path, recovery)?;
        if !target_root.is_dir() {
            return Err(authoring_error(
                "authoring_target_not_directory",
                "publish target is not a directory after transaction recovery",
            ));
        }

        let suffix = unique_suffix();
        let stage_name = format!(
            ".{}.authoring-stage-{suffix}",
            target_name.to_string_lossy()
        );
        let backup_name = format!(
            ".{}.authoring-backup-{suffix}",
            target_name.to_string_lossy()
        );
        let stage_root = parent.join(&stage_name);
        let backup_root = parent.join(&backup_name);
        let mut journal = TransactionJournal {
            schema_version: 2,
            phase: TransactionPhase::Prepared,
            stage_name,
            backup_name,
            correlation_id: Some(draft.correlation_id.clone()),
            draft_id: Some(draft.draft_id.clone()),
            target_label: Some(target_label.clone()),
            target_fingerprint: Some(target_fingerprint.clone()),
            candidate_tree_sha256: None,
        };
        create_journal(&journal_path, &journal)?;

        let prepared = (|| {
            if target_root.join(&draft.replace_scope).exists() && !request.force {
                return Err(authoring_error(
                    "record_promote_target_exists",
                    format!(
                        "publish target scope already exists: {}",
                        draft.replace_scope.display()
                    ),
                ));
            }
            copy_tree(&target_root, &stage_root)?;
            apply_draft(&stage_root, draft)?;
            events.append(&event(AuthoringEventKind::DraftBuilt, None))?;
            let validation = validator.validate(&stage_root, draft)?;
            let file_hashes = hash_tree(&stage_root)?;
            journal.phase = TransactionPhase::Validated;
            journal.candidate_tree_sha256 = Some(hash_file_manifest(&file_hashes));
            write_journal(&journal_path, &journal)?;
            events.append(&event(AuthoringEventKind::ValidationCompleted, None))?;
            events.append(&event(AuthoringEventKind::PromoteIntent, None))?;
            Ok((validation, file_hashes))
        })();

        let (validation, file_hashes) = match prepared {
            Ok(value) => value,
            Err(error) => {
                remove_tree_if_exists(&stage_root)?;
                remove_file_if_exists(&journal_path)?;
                return Err(error);
            }
        };

        if let Err(error) = fs::rename(&target_root, &backup_root) {
            let original = io_error(
                "authoring_backup_rename_failed",
                "move current resource tree to backup",
                &target_root,
                error,
            );
            return Err(cleanup_pre_commit_failure(
                original,
                &stage_root,
                &journal_path,
            ));
        }
        journal.phase = TransactionPhase::OldMoved;
        if let Err(error) = write_journal(&journal_path, &journal) {
            return Err(rollback_failure(
                error,
                &target_root,
                &stage_root,
                &backup_root,
                &journal_path,
            ));
        }

        if fault == Some(FaultPoint::AfterOldTreeMoved) {
            restore_old_tree(&target_root, &stage_root, &backup_root, &journal_path)?;
            return Err(authoring_error(
                "authoring_injected_rename_failure",
                "injected failure after moving the old resource tree",
            ));
        }

        if let Err(error) = fs::rename(&stage_root, &target_root) {
            restore_old_tree(&target_root, &stage_root, &backup_root, &journal_path)?;
            return Err(io_error(
                "authoring_candidate_rename_failed",
                "move validated candidate into place",
                &target_root,
                error,
            ));
        }
        journal.phase = TransactionPhase::CandidateMoved;
        if let Err(error) = write_journal(&journal_path, &journal) {
            return Err(rollback_failure(
                error,
                &target_root,
                &stage_root,
                &backup_root,
                &journal_path,
            ));
        }

        if let Err(error) = events.append(&event(AuthoringEventKind::Promoted, None)) {
            restore_old_tree(&target_root, &stage_root, &backup_root, &journal_path)?;
            return Err(authoring_error(
                "authoring_outcome_not_durable",
                format!("promoted outcome could not be made durable: {error}"),
            ));
        }

        journal.phase = TransactionPhase::Committed;
        if let Err(error) = write_journal(&journal_path, &journal) {
            let cleanup = remove_file_if_exists(&journal_path)
                .and_then(|()| remove_tree_if_exists(&backup_root));
            return Err(match cleanup {
                Ok(()) => authoring_error(
                    "authoring_commit_cleanup_failed",
                    format!(
                        "resource tree was promoted and recorded, but commit journal update failed: {error}"
                    ),
                ),
                Err(cleanup_error) => authoring_error(
                    "authoring_commit_cleanup_failed",
                    format!(
                        "resource tree was promoted and recorded, journal update failed ({error}), and cleanup failed ({cleanup_error})"
                    ),
                ),
            });
        }
        remove_tree_if_exists(&backup_root)?;
        remove_file_if_exists(&journal_path)?;

        Ok(AuthoringReceipt {
            correlation_id: draft.correlation_id.clone(),
            draft_id: draft.draft_id.clone(),
            target_label: target_label.clone(),
            target_fingerprint: target_fingerprint.clone(),
            changed_paths: changed_paths.clone(),
            file_hashes,
            validation,
            provenance: draft.provenance.clone(),
        })
    })();

    match result {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            let failed = event(AuthoringEventKind::PromoteFailed, Some(error.code.clone()));
            if let Err(event_error) = events.append(&failed) {
                return Err(authoring_error(
                    "authoring_failure_not_durable",
                    format!(
                        "resource publish failed ({error}); failure event also failed ({event_error})"
                    ),
                ));
            }
            Err(error)
        }
    }
}

fn apply_draft(candidate_root: &Path, draft: &AuthoringDraft) -> LabResult<()> {
    remove_tree_if_exists(&candidate_root.join(&draft.replace_scope))?;
    for file in &draft.files {
        let destination = candidate_root.join(&file.relative_path);
        if file.mode == AuthoringWriteMode::CreateIfMissing && destination.exists() {
            continue;
        }
        let parent = destination.parent().ok_or_else(|| {
            authoring_error(
                "authoring_destination_has_no_parent",
                format!(
                    "authoring destination has no parent: {}",
                    destination.display()
                ),
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            io_error(
                "authoring_create_directory_failed",
                "create candidate directory",
                parent,
                error,
            )
        })?;
        let bytes = match &file.source {
            AuthoringFileSource::Bytes(bytes) => bytes.clone(),
            AuthoringFileSource::Copy { source, .. } => read_source_file(source)?,
        };
        write_new_file(&destination, &bytes)?;
    }
    Ok(())
}

fn reject_materialization_symlinks(root: &Path, draft: &AuthoringDraft) -> LabResult<()> {
    let paths = std::iter::once(draft.replace_scope())
        .chain(draft.files().iter().map(AuthoringFile::relative_path));
    for relative in paths {
        let mut current = root.to_path_buf();
        for component in relative.components() {
            current.push(component.as_os_str());
            if !current.exists() {
                break;
            }
            let metadata = fs::symlink_metadata(&current).map_err(|error| {
                io_error(
                    "authoring_metadata_failed",
                    "inspect materialization path",
                    &current,
                    error,
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(authoring_error(
                    "authoring_symlink_rejected",
                    format!("resource authoring rejects symlink {}", current.display()),
                ));
            }
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> LabResult<()> {
    fs::create_dir(destination).map_err(|error| {
        io_error(
            "authoring_stage_create_failed",
            "create staging root",
            destination,
            error,
        )
    })?;
    copy_tree_contents(source, destination)
}

fn copy_tree_contents(source: &Path, destination: &Path) -> LabResult<()> {
    let mut entries = fs::read_dir(source)
        .map_err(|error| {
            io_error(
                "authoring_tree_read_failed",
                "read resource tree",
                source,
                error,
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            io_error(
                "authoring_tree_read_failed",
                "read resource tree entry",
                source,
                error,
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| {
            io_error(
                "authoring_metadata_failed",
                "inspect resource tree entry",
                &source_path,
                error,
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(authoring_error(
                "authoring_symlink_rejected",
                format!(
                    "resource authoring rejects symlink {}",
                    source_path.display()
                ),
            ));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination_path).map_err(|error| {
                io_error(
                    "authoring_stage_create_failed",
                    "create staging directory",
                    &destination_path,
                    error,
                )
            })?;
            copy_tree_contents(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            let bytes = read_source_file(&source_path)?;
            write_new_file(&destination_path, &bytes)?;
        } else {
            return Err(authoring_error(
                "authoring_special_file_rejected",
                format!(
                    "resource authoring rejects special file {}",
                    source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn read_source_file(path: &Path) -> LabResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        io_error(
            "authoring_source_metadata_failed",
            "inspect authoring source",
            path,
            error,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(authoring_error(
            "authoring_source_not_regular_file",
            format!("authoring source is not a regular file: {}", path.display()),
        ));
    }
    fs::read(path).map_err(|error| {
        io_error(
            "authoring_source_read_failed",
            "read authoring source",
            path,
            error,
        )
    })
}

fn write_new_file(path: &Path, bytes: &[u8]) -> LabResult<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            io_error(
                "authoring_file_create_failed",
                "create candidate file",
                path,
                error,
            )
        })?;
    file.write_all(bytes).map_err(|error| {
        io_error(
            "authoring_file_write_failed",
            "write candidate file",
            path,
            error,
        )
    })?;
    file.sync_all().map_err(|error| {
        io_error(
            "authoring_file_sync_failed",
            "sync candidate file",
            path,
            error,
        )
    })
}

fn hash_tree(root: &Path) -> LabResult<Vec<AuthoringFileHash>> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn hash_file_manifest(files: &[AuthoringFileHash]) -> String {
    let mut ordered = files.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut hasher = Sha256::new();
    for file in ordered {
        hasher.update((file.relative_path.len() as u64).to_be_bytes());
        hasher.update(file.relative_path.as_bytes());
        hasher.update(file.bytes.to_be_bytes());
        hasher.update(file.sha256.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn collect_files(root: &Path, current: &Path, files: &mut Vec<AuthoringFileHash>) -> LabResult<()> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| {
            io_error(
                "authoring_hash_read_failed",
                "read candidate tree for hashing",
                current,
                error,
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            io_error(
                "authoring_hash_read_failed",
                "read candidate entry for hashing",
                current,
                error,
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            io_error(
                "authoring_hash_metadata_failed",
                "inspect candidate entry for hashing",
                &path,
                error,
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(authoring_error(
                "authoring_symlink_rejected",
                format!("candidate tree contains symlink {}", path.display()),
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&path).map_err(|error| {
                io_error(
                    "authoring_hash_read_failed",
                    "read candidate file for hashing",
                    &path,
                    error,
                )
            })?;
            let relative = path.strip_prefix(root).map_err(|_| {
                authoring_error(
                    "authoring_hash_scope_error",
                    "candidate file escaped candidate root",
                )
            })?;
            files.push(AuthoringFileHash {
                relative_path: slash_path(relative),
                bytes: bytes.len() as u64,
                sha256: hex_sha256(&bytes),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionPhase {
    Prepared,
    Validated,
    OldMoved,
    CandidateMoved,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransactionJournal {
    schema_version: u32,
    phase: TransactionPhase,
    stage_name: String,
    backup_name: String,
    #[serde(default)]
    correlation_id: Option<String>,
    #[serde(default)]
    draft_id: Option<String>,
    #[serde(default)]
    target_label: Option<String>,
    #[serde(default)]
    target_fingerprint: Option<String>,
    #[serde(default)]
    candidate_tree_sha256: Option<String>,
}

fn recover_transaction(
    target_root: &Path,
    journal_path: &Path,
    recovery: &mut dyn AuthoringRecoveryOracle,
) -> LabResult<()> {
    if !journal_path.exists() {
        return Ok(());
    }
    let bytes = fs::read(journal_path).map_err(|error| {
        io_error(
            "authoring_journal_read_failed",
            "read authoring transaction journal",
            journal_path,
            error,
        )
    })?;
    let journal = parse_journal(&bytes)?;
    if !matches!(journal.schema_version, 1 | 2) {
        return Err(authoring_error(
            "authoring_journal_schema_unsupported",
            format!(
                "unsupported authoring transaction journal schema {}",
                journal.schema_version
            ),
        ));
    }
    validate_single_component(&journal.stage_name, "journal stage_name")?;
    validate_single_component(&journal.backup_name, "journal backup_name")?;
    let parent = target_root.parent().ok_or_else(|| {
        authoring_error(
            "authoring_target_has_no_parent",
            "publish target must have a parent directory",
        )
    })?;
    let stage_root = parent.join(&journal.stage_name);
    let backup_root = parent.join(&journal.backup_name);

    match journal.phase {
        TransactionPhase::Prepared | TransactionPhase::Validated => {
            if !target_root.exists() && backup_root.exists() {
                fs::rename(&backup_root, target_root).map_err(|error| {
                    io_error(
                        "authoring_recovery_restore_failed",
                        "restore prepared transaction backup",
                        target_root,
                        error,
                    )
                })?;
            } else if backup_root.exists() {
                return Err(authoring_error(
                    "authoring_recovery_ambiguous",
                    "prepared transaction has both target and backup trees",
                ));
            }
            remove_tree_if_exists(&stage_root)?;
        }
        TransactionPhase::OldMoved => {
            restore_old_tree(target_root, &stage_root, &backup_root, journal_path)?;
            return Ok(());
        }
        TransactionPhase::CandidateMoved if journal.schema_version == 1 => {
            restore_old_tree(target_root, &stage_root, &backup_root, journal_path)?;
            return Ok(());
        }
        TransactionPhase::CandidateMoved => {
            if !target_root.is_dir() {
                return Err(authoring_error(
                    "authoring_recovery_candidate_missing",
                    "interrupted authoring transaction has no candidate tree",
                ));
            }
            let context = journal.recovery_context()?;
            let canonical_target = fs::canonicalize(target_root).map_err(|error| {
                io_error(
                    "authoring_recovery_target_canonicalize_failed",
                    "canonicalize interrupted authoring target",
                    target_root,
                    error,
                )
            })?;
            if context.target_fingerprint != path_fingerprint(&canonical_target) {
                return Err(authoring_error(
                    "authoring_recovery_target_mismatch",
                    "interrupted authoring journal does not belong to this target tree",
                ));
            }
            let observed_hash = hash_file_manifest(&hash_tree(target_root)?);
            if observed_hash != context.candidate_tree_sha256 {
                return Err(authoring_error(
                    "authoring_recovery_candidate_mismatch",
                    "interrupted authoring candidate does not match its durable journal digest",
                ));
            }
            match recovery.decide(&context)? {
                AuthoringRecoveryDecision::CommitCandidate => {
                    remove_tree_if_exists(&stage_root)?;
                    remove_tree_if_exists(&backup_root)?;
                }
                AuthoringRecoveryDecision::RollbackCandidate => {
                    restore_old_tree(target_root, &stage_root, &backup_root, journal_path)?;
                    return Ok(());
                }
            }
        }
        TransactionPhase::Committed => {
            if !target_root.is_dir() {
                return Err(authoring_error(
                    "authoring_committed_target_missing",
                    "committed authoring transaction has no target tree",
                ));
            }
            remove_tree_if_exists(&stage_root)?;
            remove_tree_if_exists(&backup_root)?;
        }
    }
    remove_file_if_exists(journal_path)
}

impl TransactionJournal {
    fn recovery_context(&self) -> LabResult<AuthoringRecoveryContext> {
        let required = |name: &'static str, value: &Option<String>| {
            value.clone().ok_or_else(|| {
                authoring_error(
                    "authoring_journal_identity_missing",
                    format!("authoring transaction journal is missing {name}"),
                )
            })
        };
        let context = AuthoringRecoveryContext {
            correlation_id: required("correlation_id", &self.correlation_id)?,
            draft_id: required("draft_id", &self.draft_id)?,
            target_label: required("target_label", &self.target_label)?,
            target_fingerprint: required("target_fingerprint", &self.target_fingerprint)?,
            candidate_tree_sha256: required("candidate_tree_sha256", &self.candidate_tree_sha256)?,
        };
        if context.correlation_id.trim().is_empty()
            || context.draft_id.trim().is_empty()
            || context.target_label.trim().is_empty()
            || context.target_fingerprint.len() != 64
            || context.candidate_tree_sha256.len() != 64
            || !context
                .target_fingerprint
                .bytes()
                .chain(context.candidate_tree_sha256.bytes())
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(authoring_error(
                "authoring_journal_identity_invalid",
                "authoring transaction journal contains an invalid digest",
            ));
        }
        Ok(context)
    }
}

fn parse_journal(bytes: &[u8]) -> LabResult<TransactionJournal> {
    if bytes.is_empty() {
        return Err(authoring_error(
            "authoring_journal_corrupt",
            "authoring transaction journal is empty",
        ));
    }
    if !bytes.contains(&b'\n') {
        return serde_json::from_slice(bytes).map_err(|error| {
            authoring_error(
                "authoring_journal_corrupt",
                format!("authoring transaction journal is invalid: {error}"),
            )
        });
    }

    let complete_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let lines = bytes[..complete_len]
        .split(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            if index + 1 == lines.len() {
                continue;
            }
            return Err(authoring_error(
                "authoring_journal_corrupt",
                "authoring transaction journal contains a blank record",
            ));
        }
        let record: TransactionJournal = serde_json::from_slice(line).map_err(|error| {
            authoring_error(
                "authoring_journal_corrupt",
                format!("authoring transaction journal record is invalid: {error}"),
            )
        })?;
        if let Some(previous) = records.last() {
            validate_journal_transition(previous, &record)?;
        }
        records.push(record);
    }
    records.pop().ok_or_else(|| {
        authoring_error(
            "authoring_journal_corrupt",
            "authoring transaction journal has no complete record",
        )
    })
}

fn validate_journal_transition(
    previous: &TransactionJournal,
    next: &TransactionJournal,
) -> LabResult<()> {
    let identity_matches = previous.schema_version == next.schema_version
        && previous.stage_name == next.stage_name
        && previous.backup_name == next.backup_name
        && previous.correlation_id == next.correlation_id
        && previous.draft_id == next.draft_id
        && previous.target_label == next.target_label
        && previous.target_fingerprint == next.target_fingerprint;
    let phase_advances = matches!(
        (previous.phase, next.phase),
        (TransactionPhase::Prepared, TransactionPhase::Validated)
            | (TransactionPhase::Validated, TransactionPhase::OldMoved)
            | (TransactionPhase::OldMoved, TransactionPhase::CandidateMoved)
            | (
                TransactionPhase::CandidateMoved,
                TransactionPhase::Committed
            )
    );
    let hash_is_stable = match previous.phase {
        TransactionPhase::Prepared => {
            previous.candidate_tree_sha256.is_none() && next.candidate_tree_sha256.is_some()
        }
        _ => previous.candidate_tree_sha256 == next.candidate_tree_sha256,
    };
    if identity_matches && phase_advances && hash_is_stable {
        Ok(())
    } else {
        Err(authoring_error(
            "authoring_journal_transition_invalid",
            "authoring transaction journal history is inconsistent",
        ))
    }
}

fn restore_old_tree(
    target_root: &Path,
    stage_root: &Path,
    backup_root: &Path,
    journal_path: &Path,
) -> LabResult<()> {
    if !backup_root.is_dir() {
        return Err(authoring_error(
            "authoring_rollback_backup_missing",
            "cannot roll back authoring transaction because the backup tree is missing",
        ));
    }
    remove_tree_if_exists(target_root)?;
    fs::rename(backup_root, target_root).map_err(|error| {
        io_error(
            "authoring_rollback_restore_failed",
            "restore previous resource tree",
            target_root,
            error,
        )
    })?;
    remove_tree_if_exists(stage_root)?;
    remove_file_if_exists(journal_path)
}

fn cleanup_pre_commit_failure(
    original: LabError,
    stage_root: &Path,
    journal_path: &Path,
) -> LabError {
    match remove_tree_if_exists(stage_root).and_then(|()| remove_file_if_exists(journal_path)) {
        Ok(()) => original,
        Err(cleanup) => authoring_error(
            "authoring_cleanup_failed",
            format!("authoring failed ({original}) and staging cleanup failed ({cleanup})"),
        ),
    }
}

fn rollback_failure(
    original: LabError,
    target_root: &Path,
    stage_root: &Path,
    backup_root: &Path,
    journal_path: &Path,
) -> LabError {
    match restore_old_tree(target_root, stage_root, backup_root, journal_path) {
        Ok(()) => original,
        Err(rollback) => authoring_error(
            "authoring_rollback_failed",
            format!("authoring failed ({original}) and rollback failed ({rollback})"),
        ),
    }
}

fn create_journal(path: &Path, journal: &TransactionJournal) -> LabResult<()> {
    let bytes = journal_record_bytes(journal)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            io_error(
                "authoring_transaction_conflict",
                "create authoring transaction journal",
                path,
                error,
            )
        })?;
    file.write_all(&bytes).map_err(|error| {
        io_error(
            "authoring_journal_write_failed",
            "write authoring transaction journal",
            path,
            error,
        )
    })?;
    file.sync_all().map_err(|error| {
        io_error(
            "authoring_journal_sync_failed",
            "sync authoring transaction journal",
            path,
            error,
        )
    })
}

fn write_journal(path: &Path, journal: &TransactionJournal) -> LabResult<()> {
    let bytes = journal_record_bytes(journal)?;
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| {
            io_error(
                "authoring_journal_open_failed",
                "open authoring transaction journal",
                path,
                error,
            )
        })?;
    file.write_all(&bytes).map_err(|error| {
        io_error(
            "authoring_journal_write_failed",
            "write authoring transaction journal",
            path,
            error,
        )
    })?;
    file.sync_all().map_err(|error| {
        io_error(
            "authoring_journal_sync_failed",
            "sync authoring transaction journal",
            path,
            error,
        )
    })
}

fn journal_record_bytes(journal: &TransactionJournal) -> LabResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(journal).map_err(|error| {
        authoring_error(
            "authoring_journal_serialize_failed",
            format!("failed to serialize authoring transaction journal: {error}"),
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn remove_tree_if_exists(path: &Path) -> LabResult<()> {
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(authoring_error(
            "authoring_tree_type_mismatch",
            format!("expected directory at {}", path.display()),
        ));
    }
    fs::remove_dir_all(path).map_err(|error| {
        io_error(
            "authoring_tree_remove_failed",
            "remove authoring tree",
            path,
            error,
        )
    })
}

fn remove_file_if_exists(path: &Path) -> LabResult<()> {
    if !path.exists() {
        return Ok(());
    }
    if !path.is_file() {
        return Err(authoring_error(
            "authoring_journal_type_mismatch",
            format!("expected regular file at {}", path.display()),
        ));
    }
    fs::remove_file(path).map_err(|error| {
        io_error(
            "authoring_journal_remove_failed",
            "remove authoring transaction journal",
            path,
            error,
        )
    })
}

fn resolve_publish_target(path: &Path) -> LabResult<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        authoring_error(
            "authoring_target_has_no_name",
            "publish target must have a final path component",
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        authoring_error(
            "authoring_target_has_no_parent",
            "publish target must have a parent directory",
        )
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        io_error(
            "authoring_target_unavailable",
            "resolve publish target parent",
            parent,
            error,
        )
    })?;
    if !canonical_parent.is_dir() {
        return Err(authoring_error(
            "authoring_target_not_directory",
            format!(
                "publish target parent is not a directory: {}",
                parent.display()
            ),
        ));
    }
    let target = canonical_parent.join(name);
    if target.exists() {
        let metadata = fs::symlink_metadata(&target).map_err(|error| {
            io_error(
                "authoring_target_unavailable",
                "inspect publish target",
                &target,
                error,
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(authoring_error(
                "authoring_target_symlink_rejected",
                format!("publish target must not be a symlink: {}", path.display()),
            ));
        }
        if !metadata.is_dir() {
            return Err(authoring_error(
                "authoring_target_not_directory",
                format!("publish target is not a directory: {}", path.display()),
            ));
        }
    }
    Ok(target)
}

fn validate_relative_path(path: &Path, label: &str) -> LabResult<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(authoring_error(
            "authoring_path_invalid",
            format!("{label} must be a non-empty normalized relative path"),
        ));
    }
    Ok(())
}

fn validate_single_component(value: &str, label: &str) -> LabResult<()> {
    let path = Path::new(value);
    validate_relative_path(path, label)?;
    if path.components().count() != 1 {
        return Err(authoring_error(
            "authoring_journal_path_invalid",
            format!("{label} must contain exactly one path component"),
        ));
    }
    Ok(())
}

fn non_empty(label: &str, value: String) -> LabResult<String> {
    if value.trim().is_empty() {
        Err(authoring_error(
            "authoring_field_empty",
            format!("{label} must not be empty"),
        ))
    } else {
        Ok(value)
    }
}

fn transaction_journal_name(target_root: &Path) -> String {
    format!(
        ".actingcommand-authoring-{}.json",
        path_fingerprint(target_root)
    )
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

fn path_fingerprint(path: &Path) -> String {
    hex_sha256(path.to_string_lossy().as_bytes())
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn authoring_error(code: &str, message: impl Into<String>) -> LabError {
    LabError::safety_blocked(code, message, &["resource_authoring"])
}

fn io_error(code: &str, action: &str, path: &Path, error: std::io::Error) -> LabError {
    authoring_error(
        code,
        format!("failed to {action} at {}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn draft(source: &Path) -> AuthoringDraft {
        AuthoringDraft::new(
            "corr-1",
            "draft-1",
            "operations/task-a",
            vec![
                AuthoringFile::bytes(
                    "operations/task-a/task.json",
                    br#"{"task_id":"task-a"}"#.to_vec(),
                    AuthoringWriteMode::Replace,
                )
                .expect("task file"),
                AuthoringFile::copy(
                    "operations/task-a/assets/frame.png",
                    source,
                    "artifact-1",
                    AuthoringWriteMode::Replace,
                )
                .expect("asset file"),
                AuthoringFile::bytes(
                    "operations/resources.json",
                    br#"{"schema_version":"1.0","resources":[]}"#.to_vec(),
                    AuthoringWriteMode::CreateIfMissing,
                )
                .expect("resources file"),
            ],
            AuthoringProvenance {
                record_id: "record-1".to_string(),
                source_artifact_ids: vec!["artifact-1".to_string()],
            },
        )
        .expect("draft")
    }

    fn request(root: &Path, force: bool) -> AuthoringPublishRequest {
        AuthoringPublishRequest {
            target_root: root.to_path_buf(),
            target_label: "test-resource-root".to_string(),
            force,
        }
    }

    fn validator() -> impl AuthoringValidator {
        |candidate: &Path, _draft: &AuthoringDraft| {
            let task = fs::read(candidate.join("operations/task-a/task.json"))
                .map_err(|error| LabError::usage(error.to_string()))?;
            serde_json::from_slice::<serde_json::Value>(&task)
                .map_err(|error| LabError::usage(error.to_string()))?;
            AuthoringValidationReport::new(vec!["draft_schema".to_string()])
        }
    }

    #[test]
    fn materialize_replaces_only_the_draft_scope_and_preserves_create_if_missing_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("draft-output");
        let source = temp.path().join("frame.png");
        fs::write(&source, b"png").expect("source");

        materialize_authoring_draft(&root, &draft(&source)).expect("first materialization");
        assert!(root.join("operations/task-a/task.json").is_file());
        assert_eq!(
            fs::read(root.join("operations/task-a/assets/frame.png")).expect("asset"),
            b"png"
        );
        fs::write(root.join("operations/task-a/stale.txt"), b"stale").expect("stale file");
        fs::write(
            root.join("operations/resources.json"),
            br#"{"schema_version":"1.0","resources":[{"id":"keep"}]}"#,
        )
        .expect("resources");

        materialize_authoring_draft(&root, &draft(&source)).expect("second materialization");
        assert!(!root.join("operations/task-a/stale.txt").exists());
        assert!(
            fs::read_to_string(root.join("operations/resources.json"))
                .expect("resources")
                .contains("keep")
        );
    }

    #[test]
    fn publish_replaces_one_scope_and_emits_ordered_events() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old task");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        fs::write(temp.path().join("frame.png"), b"png").expect("source");
        let draft = draft(&temp.path().join("frame.png"));
        let seen = RefCell::new(Vec::new());
        let mut sink = |event: &AuthoringEvent| {
            seen.borrow_mut().push(event.kind);
            Ok(())
        };
        let mut validator = validator();

        let receipt =
            publish_authoring_draft(&request(&root, true), &draft, &mut validator, &mut sink)
                .expect("publish");

        assert!(!root.join("operations/task-a/old.txt").exists());
        assert_eq!(
            fs::read(root.join("operations/task-a/assets/frame.png")).expect("asset"),
            b"png"
        );
        assert!(
            receipt
                .file_hashes
                .iter()
                .any(|file| file.relative_path == "operations/task-a/task.json")
        );
        assert_eq!(
            *seen.borrow(),
            vec![
                AuthoringEventKind::AuthoringStarted,
                AuthoringEventKind::DraftBuilt,
                AuthoringEventKind::ValidationCompleted,
                AuthoringEventKind::PromoteIntent,
                AuthoringEventKind::Promoted,
            ]
        );
    }

    #[test]
    fn conflict_without_force_preserves_old_tree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old task");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        fs::write(temp.path().join("frame.png"), b"png").expect("source");
        let mut validator = validator();
        let mut sink = |_event: &AuthoringEvent| Ok(());

        let error = publish_authoring_draft(
            &request(&root, false),
            &draft(&temp.path().join("frame.png")),
            &mut validator,
            &mut sink,
        )
        .expect_err("conflict");

        assert_eq!(error.code, "record_promote_target_exists");
        assert_eq!(
            fs::read(root.join("operations/task-a/old.txt")).expect("old preserved"),
            b"old"
        );
    }

    #[test]
    fn validation_failure_preserves_old_tree_without_mixed_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old task");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        fs::write(temp.path().join("frame.png"), b"png").expect("source");
        let mut validator = |_candidate: &Path, _draft: &AuthoringDraft| {
            Err(authoring_error(
                "authoring_validation_failed",
                "injected validation failure",
            ))
        };
        let mut sink = |_event: &AuthoringEvent| Ok(());

        let error = publish_authoring_draft(
            &request(&root, true),
            &draft(&temp.path().join("frame.png")),
            &mut validator,
            &mut sink,
        )
        .expect_err("validation failure");

        assert_eq!(error.code, "authoring_validation_failed");
        assert!(root.join("operations/task-a/old.txt").is_file());
        assert!(!root.join("operations/task-a/task.json").exists());
    }

    #[test]
    fn rename_failure_rolls_back_old_tree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old task");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        fs::write(temp.path().join("frame.png"), b"png").expect("source");
        let mut recovery =
            |_context: &AuthoringRecoveryContext| Ok(AuthoringRecoveryDecision::RollbackCandidate);
        let mut validator = validator();
        let mut sink = |_event: &AuthoringEvent| Ok(());

        let error = publish_authoring_draft_inner(
            &request(&root, true),
            &draft(&temp.path().join("frame.png")),
            &mut recovery,
            &mut validator,
            &mut sink,
            Some(FaultPoint::AfterOldTreeMoved),
        )
        .expect_err("rename failure");

        assert_eq!(error.code, "authoring_injected_rename_failure");
        assert!(root.join("operations/task-a/old.txt").is_file());
        assert!(!root.join("operations/task-a/task.json").exists());
    }

    #[test]
    fn terminal_event_failure_rolls_back_candidate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old task");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        fs::write(temp.path().join("frame.png"), b"png").expect("source");
        let mut validator = validator();
        let mut sink = |event: &AuthoringEvent| {
            if event.kind == AuthoringEventKind::Promoted {
                Err(authoring_error(
                    "ledger_unavailable",
                    "injected ledger failure",
                ))
            } else {
                Ok(())
            }
        };

        let error = publish_authoring_draft(
            &request(&root, true),
            &draft(&temp.path().join("frame.png")),
            &mut validator,
            &mut sink,
        )
        .expect_err("terminal event failure");

        assert_eq!(error.code, "authoring_outcome_not_durable");
        assert!(root.join("operations/task-a/old.txt").is_file());
        assert!(!root.join("operations/task-a/task.json").exists());
    }

    #[test]
    fn interrupted_candidate_move_is_rolled_back_before_the_next_publish() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        let stage = temp.path().join(".ours.authoring-stage-crash");
        let backup = temp.path().join(".ours.authoring-backup-crash");
        fs::create_dir_all(root.join("operations/task-a")).expect("old tree");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        fs::create_dir_all(stage.join("operations/task-a")).expect("candidate tree");
        fs::write(stage.join("operations/task-a/task.json"), b"new").expect("candidate file");
        let canonical = fs::canonicalize(&root).expect("canonical target");
        let journal_path = temp.path().join(transaction_journal_name(&canonical));
        fs::rename(&root, &backup).expect("move old tree");
        fs::rename(&stage, &root).expect("move candidate tree");
        create_journal(
            &journal_path,
            &TransactionJournal {
                schema_version: 1,
                phase: TransactionPhase::CandidateMoved,
                stage_name: ".ours.authoring-stage-crash".to_string(),
                backup_name: ".ours.authoring-backup-crash".to_string(),
                correlation_id: None,
                draft_id: None,
                target_label: None,
                target_fingerprint: None,
                candidate_tree_sha256: None,
            },
        )
        .expect("journal");

        let mut recovery =
            |_context: &AuthoringRecoveryContext| Ok(AuthoringRecoveryDecision::RollbackCandidate);
        recover_transaction(&root, &journal_path, &mut recovery).expect("recover");

        assert!(root.join("operations/task-a/old.txt").is_file());
        assert!(!root.join("operations/task-a/task.json").exists());
        assert!(!backup.exists());
        assert!(!journal_path.exists());
    }

    fn install_candidate_moved_v2_transaction(
        temp: &Path,
        root: &Path,
    ) -> (PathBuf, PathBuf, AuthoringRecoveryContext) {
        let stage_name = ".ours.authoring-stage-v2";
        let backup_name = ".ours.authoring-backup-v2";
        let stage = temp.join(stage_name);
        let backup = temp.join(backup_name);
        fs::create_dir_all(stage.join("operations/task-a")).expect("candidate tree");
        fs::write(stage.join("operations/task-a/task.json"), b"new").expect("candidate file");
        let canonical = fs::canonicalize(root).expect("canonical target");
        let journal_path = temp.join(transaction_journal_name(&canonical));
        fs::rename(root, &backup).expect("move old tree");
        fs::rename(&stage, root).expect("move candidate tree");
        let mut journal = TransactionJournal {
            schema_version: 2,
            phase: TransactionPhase::Prepared,
            stage_name: stage_name.to_string(),
            backup_name: backup_name.to_string(),
            correlation_id: Some("correlation_test".to_string()),
            draft_id: Some("draft-test".to_string()),
            target_label: Some("test-resource-root".to_string()),
            target_fingerprint: Some(path_fingerprint(&canonical)),
            candidate_tree_sha256: None,
        };
        create_journal(&journal_path, &journal).expect("prepared journal");
        journal.phase = TransactionPhase::Validated;
        journal.candidate_tree_sha256 = Some(hash_file_manifest(
            &hash_tree(root).expect("candidate hashes"),
        ));
        write_journal(&journal_path, &journal).expect("validated journal");
        journal.phase = TransactionPhase::OldMoved;
        write_journal(&journal_path, &journal).expect("old-moved journal");
        journal.phase = TransactionPhase::CandidateMoved;
        write_journal(&journal_path, &journal).expect("candidate-moved journal");
        let context = journal.recovery_context().expect("recovery context");
        (backup, journal_path, context)
    }

    #[test]
    fn durable_promoted_recovery_commits_candidate_instead_of_rolling_back() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old tree");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        let (backup, journal_path, expected) =
            install_candidate_moved_v2_transaction(temp.path(), &root);
        let mut recovery = |context: &AuthoringRecoveryContext| {
            assert_eq!(context, &expected);
            Ok(AuthoringRecoveryDecision::CommitCandidate)
        };

        recover_transaction(&root, &journal_path, &mut recovery).expect("commit recovery");

        assert_eq!(
            fs::read(root.join("operations/task-a/task.json")).expect("candidate"),
            b"new"
        );
        assert!(!root.join("operations/task-a/old.txt").exists());
        assert!(!backup.exists());
        assert!(!journal_path.exists());
    }

    #[test]
    fn nonterminal_recovery_rolls_back_candidate_to_the_old_tree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old tree");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        let (backup, journal_path, _) = install_candidate_moved_v2_transaction(temp.path(), &root);
        let mut recovery =
            |_context: &AuthoringRecoveryContext| Ok(AuthoringRecoveryDecision::RollbackCandidate);

        recover_transaction(&root, &journal_path, &mut recovery).expect("rollback recovery");

        assert!(root.join("operations/task-a/old.txt").is_file());
        assert!(!root.join("operations/task-a/task.json").exists());
        assert!(!backup.exists());
        assert!(!journal_path.exists());
    }

    #[test]
    fn recovery_rejects_a_candidate_that_changed_after_the_durable_journal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old tree");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        let (backup, journal_path, _) = install_candidate_moved_v2_transaction(temp.path(), &root);
        fs::write(root.join("operations/task-a/task.json"), b"tampered").expect("tamper candidate");
        let mut recovery = |_context: &AuthoringRecoveryContext| -> LabResult<_> {
            panic!("digest mismatch must be rejected before asking the oracle")
        };

        let error = recover_transaction(&root, &journal_path, &mut recovery)
            .expect_err("candidate mismatch");

        assert_eq!(error.code, "authoring_recovery_candidate_mismatch");
        assert_eq!(
            fs::read(root.join("operations/task-a/task.json")).expect("candidate retained"),
            b"tampered"
        );
        assert!(backup.join("operations/task-a/old.txt").is_file());
        assert!(journal_path.is_file());
    }

    #[test]
    fn partial_final_journal_record_recovers_from_the_last_durable_phase() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("ours");
        fs::create_dir_all(root.join("operations/task-a")).expect("old tree");
        fs::write(root.join("operations/task-a/old.txt"), b"old").expect("old file");
        let (backup, journal_path, _) = install_candidate_moved_v2_transaction(temp.path(), &root);
        OpenOptions::new()
            .append(true)
            .open(&journal_path)
            .expect("journal append")
            .write_all(b"{partial")
            .expect("partial tail");
        let mut recovery =
            |_context: &AuthoringRecoveryContext| Ok(AuthoringRecoveryDecision::CommitCandidate);

        recover_transaction(&root, &journal_path, &mut recovery).expect("tail recovery");

        assert!(root.join("operations/task-a/task.json").is_file());
        assert!(!backup.exists());
        assert!(!journal_path.exists());
    }

    #[test]
    fn invalid_relative_path_is_rejected() {
        let error = AuthoringFile::bytes("../escape.json", Vec::new(), AuthoringWriteMode::Replace)
            .expect_err("path escape");

        assert_eq!(error.code, "authoring_path_invalid");
    }
}
