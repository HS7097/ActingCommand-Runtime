// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, CapturePipelineCounts, CapturePipelineSummary,
    ScreenshotNameAllocator, StoredArtifact, canonical_sha256,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactPayloadDraft, ArtifactProducer,
    ArtifactRedactionState, ArtifactReference, AuditInput, CorrelationId, DiagnosticCode,
    EventActor, EventDraft, EventOrigin, EventSeverity, EventSource, EventType,
    EvidenceCompleteness, IdentifierIssuer, OriginModule, PinnedFrameReason,
    ProjectedArtifactReference, ProjectedEvent, ProjectionProfile, RetentionClass, RunId,
    TaskOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const EVIDENCE_MANIFEST_PATH: &str = "evidence/manifest.json";
const EVIDENCE_SCHEMA_VERSION: &str = "actingcommand.evidence.v1";
const TEMP_PATH_ATTEMPTS: u64 = 1_024;

static NEXT_EXPORT_TEMP: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageVerification {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePackage {
    file_name: String,
    sha256: String,
    verification: PackageVerification,
}

impl EvidencePackage {
    pub fn new(
        file_name: impl Into<String>,
        sha256: impl AsRef<str>,
        verification: PackageVerification,
    ) -> ArtifactStoreResult<Self> {
        let file_name = file_name.into();
        if file_name.is_empty()
            || file_name.contains(['/', '\\'])
            || Path::new(&file_name)
                .file_name()
                .and_then(|value| value.to_str())
                != Some(file_name.as_str())
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_package_invalid",
                "create_evidence_package",
                "package file name must be one safe file-name component",
            ));
        }
        Ok(Self {
            file_name,
            sha256: normalize_sha256(sha256.as_ref())?,
            verification,
        })
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn verification(&self) -> PackageVerification {
        self.verification
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceJsonDocument(Vec<u8>);

impl EvidenceJsonDocument {
    pub fn from_serializable<T: Serialize>(value: &T) -> ArtifactStoreResult<Self> {
        let value = serde_json::to_value(value).map_err(|error| {
            ArtifactStoreError::fatal(
                "evidence_json_invalid",
                "serialize_evidence_json",
                error.to_string(),
            )
        })?;
        if !value.is_object() {
            return Err(ArtifactStoreError::fatal(
                "evidence_json_invalid",
                "serialize_evidence_json",
                "evidence JSON document must be an object",
            ));
        }
        serde_json::to_vec_pretty(&value)
            .map(Self)
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "evidence_json_invalid",
                    "serialize_evidence_json",
                    error.to_string(),
                )
            })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceExportDocuments {
    pub result: EvidenceJsonDocument,
    pub diagnostics: EvidenceJsonDocument,
    pub summary: String,
}

impl EvidenceExportDocuments {
    pub fn new(
        result: EvidenceJsonDocument,
        diagnostics: EvidenceJsonDocument,
        summary: impl Into<String>,
    ) -> ArtifactStoreResult<Self> {
        let summary = summary.into();
        if summary.trim().is_empty() || summary.contains('\0') {
            return Err(ArtifactStoreError::fatal(
                "evidence_summary_invalid",
                "create_evidence_documents",
                "evidence summary must be non-empty text without NUL bytes",
            ));
        }
        Ok(Self {
            result,
            diagnostics,
            summary,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceExportIdentity {
    pub run_id: RunId,
    pub correlation_id: CorrelationId,
    pub package: EvidencePackage,
    pub task_outcome: TaskOutcome,
    pub terminal_receipt: ProjectedEvent,
    pub projection_profile: ProjectionProfile,
    pub retention_class: RetentionClass,
    pub archive_redaction_state: ArtifactRedactionState,
}

#[derive(Debug, Clone)]
pub struct EvidenceExportRequest {
    pub output_path: PathBuf,
    pub identity: EvidenceExportIdentity,
    pub events: Vec<ProjectedEvent>,
    pub pipeline: CapturePipelineSummary,
    pub documents: EvidenceExportDocuments,
    pub archive_context: ArtifactWriteContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceScreenshotCounts {
    pub captured: u64,
    pub deduplicated: u64,
    pub dropped: u64,
    pub persisted: u64,
}

impl From<CapturePipelineCounts> for EvidenceScreenshotCounts {
    fn from(value: CapturePipelineCounts) -> Self {
        Self {
            captured: value.captured,
            deduplicated: value.deduplicated,
            dropped: value.dropped,
            persisted: value.persisted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArchiveEntry {
    pub path: String,
    pub byte_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceScreenshot {
    pub frame_index: usize,
    pub archive_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_reason: Option<PinnedFrameReason>,
    pub artifact: ProjectedArtifactReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingPinnedFrame {
    pub frame_index: usize,
    pub reason: PinnedFrameReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceManifest {
    pub schema_version: String,
    pub run_id: RunId,
    pub correlation_id: CorrelationId,
    pub package: EvidencePackage,
    pub ledger_sequence_start: u64,
    pub ledger_sequence_end: u64,
    pub task_outcome: TaskOutcome,
    pub evidence_completeness: EvidenceCompleteness,
    pub terminal_receipt: ProjectedEvent,
    pub artifact_count: u64,
    pub screenshots: Vec<EvidenceScreenshot>,
    pub screenshot_counts: EvidenceScreenshotCounts,
    pub pinned_count: u64,
    pub pinned_reason_counts: BTreeMap<PinnedFrameReason, u64>,
    pub missing_pinned: Vec<MissingPinnedFrame>,
    pub projection_profile: ProjectionProfile,
    pub retention_class: RetentionClass,
    pub normalized_output_path: String,
    pub entries: Vec<EvidenceArchiveEntry>,
    /// Hash of the canonical entry-digest list. The final ZIP hash lives in the external receipt
    /// because embedding a file's own final hash inside that file is self-referential.
    pub archive_content_sha256: String,
}

#[derive(Debug, Clone)]
pub struct EvidenceExportReceipt {
    output_path: PathBuf,
    zip_byte_count: u64,
    zip_sha256: String,
    manifest_sha256: String,
    archive: ArtifactReference,
    manifest: EvidenceManifest,
}

impl EvidenceExportReceipt {
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    pub const fn zip_byte_count(&self) -> u64 {
        self.zip_byte_count
    }

    pub fn zip_sha256(&self) -> &str {
        &self.zip_sha256
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub const fn archive(&self) -> &ArtifactReference {
        &self.archive
    }

    pub const fn manifest(&self) -> &EvidenceManifest {
        &self.manifest
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceArchiveVerification {
    pub manifest: EvidenceManifest,
    pub zip_byte_count: u64,
    pub zip_sha256: String,
    pub manifest_sha256: String,
}

pub struct EvidenceExporter {
    artifact_store: ArtifactStore,
    event_ids: IdentifierIssuer,
}

impl EvidenceExporter {
    pub fn open(artifact_root: impl AsRef<Path>) -> ArtifactStoreResult<Self> {
        Ok(Self {
            artifact_store: ArtifactStore::open(artifact_root)?,
            event_ids: IdentifierIssuer::new().map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_issuer_failed",
                    "open_evidence_exporter",
                    error.to_string(),
                )
            })?,
        })
    }

    pub fn export(
        &mut self,
        request: EvidenceExportRequest,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<EvidenceExportReceipt> {
        match self.export_inner(&request, sink) {
            Ok(receipt) => Ok(receipt),
            Err(mut error) => {
                match artifact_count(&request.pipeline) {
                    Ok(count) => {
                        if let Err(event_error) = self.append_export_event(
                            sink,
                            &request,
                            ArtifactPayloadDraft::export_failed(
                                DiagnosticCode::ArtifactExportFailed,
                                request.identity.task_outcome,
                                request.pipeline.evidence_completeness,
                                count,
                                AuditInput::new(),
                            ),
                            EventSeverity::Error,
                            None,
                        ) {
                            error = error.with_secondary(&event_error);
                        }
                    }
                    Err(count_error) => error = error.with_secondary(&count_error),
                }
                Err(error)
            }
        }
    }

    fn export_inner(
        &mut self,
        request: &EvidenceExportRequest,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<EvidenceExportReceipt> {
        validate_request(request)?;
        let output_path = normalize_output_path(&request.output_path)?;
        if output_path.exists() {
            return Err(ArtifactStoreError::fatal(
                "evidence_output_collision",
                "export_evidence",
                "evidence output path already exists",
            ));
        }

        let (entries, manifest) = self.build_entries(request, &output_path)?;
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            ArtifactStoreError::fatal(
                "evidence_manifest_invalid",
                "serialize_evidence_manifest",
                error.to_string(),
            )
        })?;
        let manifest_sha256 = canonical_sha256(&manifest_bytes);
        let (temp_path, temp_file) = create_export_temp(&output_path)?;
        if let Err(error) = write_archive(temp_file, &entries, &manifest_bytes) {
            return Err(cleanup_file(&temp_path, "cleanup_evidence_temp", error));
        }
        let temp_verification = match inspect_evidence_archive(&temp_path) {
            Ok(verification) => verification,
            Err(error) => {
                return Err(cleanup_file(&temp_path, "cleanup_evidence_temp", error));
            }
        };
        if temp_verification.manifest != manifest
            || temp_verification.manifest_sha256 != manifest_sha256
        {
            return Err(cleanup_file(
                &temp_path,
                "cleanup_evidence_temp",
                ArtifactStoreError::fatal(
                    "evidence_archive_mismatch",
                    "verify_evidence_temp",
                    "archive manifest changed during ZIP generation",
                ),
            ));
        }

        publish_archive(&temp_path, &output_path)?;
        let published = match verify_evidence_archive(&output_path, &temp_verification.zip_sha256) {
            Ok(verification) => verification,
            Err(error) => {
                return Err(cleanup_file(
                    &output_path,
                    "rollback_evidence_output",
                    error,
                ));
            }
        };
        let zip_bytes = match fs::read(&output_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Err(cleanup_file(
                    &output_path,
                    "rollback_evidence_output",
                    ArtifactStoreError::fatal(
                        "evidence_archive_read_failed",
                        "read_published_evidence",
                        error.to_string(),
                    ),
                ));
            }
        };

        let stored = match self.artifact_store.put(
            ArtifactWriteRequest::new(
                ArtifactKind::EvidenceArchive,
                &zip_bytes,
                request.archive_context.clone(),
                ArtifactIssuePolicy::new(
                    ArtifactProducer::EvidenceExporter,
                    request.identity.retention_class,
                    request.identity.archive_redaction_state,
                ),
            ),
            sink,
        ) {
            Ok(stored) => stored,
            Err(error) => {
                return Err(cleanup_file(
                    &output_path,
                    "rollback_evidence_output",
                    error,
                ));
            }
        };
        if stored.reference().sha256() != published.zip_sha256
            || stored.reference().byte_count() != published.zip_byte_count
        {
            let error = cleanup_file(
                &output_path,
                "rollback_evidence_output",
                ArtifactStoreError::fatal(
                    "evidence_archive_mismatch",
                    "store_evidence_archive",
                    "stored archive metadata does not match the published ZIP",
                ),
            );
            return Err(self.artifact_store.rollback_stored(&stored, error));
        }

        let completed_artifact_count = artifact_count(&request.pipeline)?
            .checked_add(1)
            .ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "evidence_count_overflow",
                    "complete_evidence_export",
                    "archive artifact count exceeds u64",
                )
            })?;
        if let Err(error) = self.append_export_event(
            sink,
            request,
            ArtifactPayloadDraft::export_completed(
                request.identity.task_outcome,
                request.pipeline.evidence_completeness,
                completed_artifact_count,
                AuditInput::new(),
            ),
            EventSeverity::Info,
            Some(&stored),
        ) {
            let error = cleanup_file(&output_path, "rollback_evidence_output", error);
            return Err(self.artifact_store.rollback_stored(&stored, error));
        }

        Ok(EvidenceExportReceipt {
            output_path,
            zip_byte_count: published.zip_byte_count,
            zip_sha256: published.zip_sha256,
            manifest_sha256: published.manifest_sha256,
            archive: stored.reference().clone(),
            manifest,
        })
    }

    fn build_entries(
        &self,
        request: &EvidenceExportRequest,
        output_path: &Path,
    ) -> ArtifactStoreResult<(BTreeMap<String, Vec<u8>>, EvidenceManifest)> {
        let mut entries = BTreeMap::new();
        entries.insert(
            "evidence/result.json".to_string(),
            request.documents.result.as_bytes().to_vec(),
        );
        entries.insert(
            "evidence/events.jsonl".to_string(),
            events_jsonl(&request.events)?,
        );
        entries.insert(
            "evidence/diagnostics.json".to_string(),
            request.documents.diagnostics.as_bytes().to_vec(),
        );
        entries.insert(
            "evidence/summary.txt".to_string(),
            request.documents.summary.as_bytes().to_vec(),
        );

        let mut names = ScreenshotNameAllocator::in_memory();
        let mut screenshots = Vec::new();
        let mut frames = request.pipeline.frames.clone();
        frames.sort_by_key(|frame| frame.frame_index);
        for frame in frames {
            if frame.artifact.kind() != ArtifactKind::CaptureFrame {
                return Err(ArtifactStoreError::fatal(
                    "evidence_artifact_invalid",
                    "build_evidence_entries",
                    "capture pipeline contains a non-frame artifact",
                ));
            }
            let bytes = self.artifact_store.read_verified(&frame.artifact)?;
            let file_name = names.allocate(frame.artifact.created_at_unix_ms())?;
            let archive_path = format!("screenshots/{file_name}");
            if entries.insert(archive_path.clone(), bytes).is_some() {
                return Err(ArtifactStoreError::fatal(
                    "evidence_entry_collision",
                    "build_evidence_entries",
                    "duplicate screenshot archive path",
                ));
            }
            screenshots.push(EvidenceScreenshot {
                frame_index: frame.frame_index,
                archive_path,
                pinned_reason: frame.pinned_reason,
                artifact: frame.artifact.project(true),
            });
        }

        let entry_digests = entries
            .iter()
            .map(|(path, bytes)| entry_digest(path, bytes))
            .collect::<ArtifactStoreResult<Vec<_>>>()?;
        let archive_content_sha256 =
            canonical_sha256(&serde_json::to_vec(&entry_digests).map_err(|error| {
                ArtifactStoreError::fatal(
                    "evidence_manifest_invalid",
                    "hash_evidence_entry_manifest",
                    error.to_string(),
                )
            })?);
        let (pinned_reason_counts, missing_pinned) = pinned_accounting(&request.pipeline);
        let normalized_output_path = output_path.to_str().ok_or_else(|| {
            ArtifactStoreError::fatal(
                "evidence_output_invalid",
                "normalize_evidence_output",
                "evidence output path is not valid UTF-8",
            )
        })?;
        let ledger_sequence_start = request
            .events
            .first()
            .map(|event| event.sequence)
            .ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "evidence_ledger_missing",
                    "build_evidence_manifest",
                    "evidence event range is empty",
                )
            })?;
        let ledger_sequence_end = request
            .events
            .last()
            .map(|event| event.sequence)
            .ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "evidence_ledger_missing",
                    "build_evidence_manifest",
                    "evidence event range is empty",
                )
            })?;

        Ok((
            entries,
            EvidenceManifest {
                schema_version: EVIDENCE_SCHEMA_VERSION.to_string(),
                run_id: request.identity.run_id,
                correlation_id: request.identity.correlation_id,
                package: request.identity.package.clone(),
                ledger_sequence_start,
                ledger_sequence_end,
                task_outcome: request.identity.task_outcome,
                evidence_completeness: request.pipeline.evidence_completeness,
                terminal_receipt: request.identity.terminal_receipt.clone(),
                artifact_count: artifact_count(&request.pipeline)?,
                screenshots,
                screenshot_counts: request.pipeline.counts.into(),
                pinned_count: u64::try_from(request.pipeline.pinned.len()).map_err(|_| {
                    ArtifactStoreError::fatal(
                        "evidence_count_overflow",
                        "build_evidence_manifest",
                        "pinned frame count exceeds u64",
                    )
                })?,
                pinned_reason_counts,
                missing_pinned,
                projection_profile: request.identity.projection_profile,
                retention_class: request.identity.retention_class,
                normalized_output_path: normalized_output_path.to_string(),
                entries: entry_digests,
                archive_content_sha256,
            },
        ))
    }

    fn append_export_event(
        &mut self,
        sink: &mut dyn ArtifactEventSink,
        request: &EvidenceExportRequest,
        payload: ArtifactPayloadDraft,
        severity: EventSeverity,
        archive: Option<&StoredArtifact>,
    ) -> ArtifactStoreResult<()> {
        let mut draft = EventDraft::new(
            self.event_ids.mint_event_id().map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_issuer_failed",
                    "append_evidence_export_event",
                    error.to_string(),
                )
            })?,
            request.archive_context.created_at_unix_ms(),
            severity,
            EventOrigin::new(
                EventSource::System,
                OriginModule::EvidenceExporter,
                EventActor::System,
            ),
            request.archive_context.event_links().clone(),
            payload.into(),
        );
        if let Some(archive) = archive {
            draft = draft.with_artifacts(vec![archive.issued.clone()]);
        }
        sink.append(draft)
    }
}

pub fn verify_evidence_archive(
    path: impl AsRef<Path>,
    expected_zip_sha256: &str,
) -> ArtifactStoreResult<EvidenceArchiveVerification> {
    let expected = normalize_sha256(expected_zip_sha256)?;
    let verification = inspect_evidence_archive(path.as_ref())?;
    if verification.zip_sha256 != expected {
        return Err(ArtifactStoreError::fatal(
            "evidence_archive_hash_mismatch",
            "verify_evidence_archive",
            "evidence ZIP SHA-256 does not match the expected receipt",
        ));
    }
    Ok(verification)
}

fn validate_request(request: &EvidenceExportRequest) -> ArtifactStoreResult<()> {
    validate_projected_events(
        &request.events,
        request.identity.run_id,
        request.identity.correlation_id,
        &request.identity.terminal_receipt,
        request.identity.task_outcome,
    )?;
    validate_pipeline_summary(&request.pipeline)
}

fn validate_projected_events(
    events: &[ProjectedEvent],
    run_id: RunId,
    correlation_id: CorrelationId,
    terminal: &ProjectedEvent,
    task_outcome: TaskOutcome,
) -> ArtifactStoreResult<()> {
    if events.is_empty() {
        return Err(ArtifactStoreError::fatal(
            "evidence_ledger_missing",
            "validate_evidence_events",
            "evidence export requires at least one projected event",
        ));
    }
    let mut previous = 0;
    for event in events {
        if event.sequence == 0 || event.sequence <= previous {
            return Err(ArtifactStoreError::fatal(
                "evidence_ledger_invalid",
                "validate_evidence_events",
                "projected events must have strictly increasing non-zero sequences",
            ));
        }
        if event.links.correlation_id() != Some(&correlation_id)
            || event
                .links
                .run_id()
                .is_some_and(|event_run_id| event_run_id != &run_id)
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_ledger_invalid",
                "validate_evidence_events",
                "projected event links do not match the export identity",
            ));
        }
        previous = event.sequence;
    }
    if terminal.links.run_id() != Some(&run_id)
        || terminal.links.correlation_id() != Some(&correlation_id)
        || terminal.event_type != terminal_event_type(task_outcome)
        || !events.iter().any(|event| event == terminal)
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_terminal_invalid",
            "validate_evidence_events",
            "terminal receipt is missing, mismatched, or inconsistent with task outcome",
        ));
    }
    Ok(())
}

fn validate_pipeline_summary(summary: &CapturePipelineSummary) -> ArtifactStoreResult<()> {
    let persisted = u64::try_from(summary.frames.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "validate_capture_summary",
            "persisted frame count exceeds u64",
        )
    })?;
    if persisted != summary.counts.persisted {
        return Err(ArtifactStoreError::fatal(
            "evidence_count_mismatch",
            "validate_capture_summary",
            "persisted screenshot count does not match frame evidence",
        ));
    }
    let mut frame_indexes = BTreeSet::new();
    let mut artifact_ids = BTreeSet::new();
    let mut object_keys = BTreeSet::new();
    for frame in &summary.frames {
        if !frame_indexes.insert(frame.frame_index)
            || !artifact_ids.insert(*frame.artifact.artifact_id())
            || !object_keys.insert(frame.artifact.object_key())
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_frame_duplicate",
                "validate_capture_summary",
                "persisted frame indexes and artifact identities must be unique",
            ));
        }
    }
    let (reason_counts, missing) = pinned_accounting(summary);
    let missing_count = u64::try_from(missing.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "validate_capture_summary",
            "missing pinned frame count exceeds u64",
        )
    })?;
    let accounted = summary
        .counts
        .persisted
        .checked_add(summary.counts.deduplicated)
        .and_then(|count| count.checked_add(missing_count))
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "evidence_count_overflow",
                "validate_capture_summary",
                "capture accounting exceeds u64",
            )
        })?;
    if summary.counts.captured != accounted {
        return Err(ArtifactStoreError::fatal(
            "evidence_count_mismatch",
            "validate_capture_summary",
            "captured count must equal persisted, deduplicated, and missing pinned frames",
        ));
    }
    let expected = if !missing.is_empty() {
        EvidenceCompleteness::Failed
    } else if summary.counts.dropped > 0 {
        EvidenceCompleteness::Partial
    } else {
        EvidenceCompleteness::Complete
    };
    if summary.evidence_completeness != expected {
        return Err(ArtifactStoreError::fatal(
            "evidence_completeness_mismatch",
            "validate_capture_summary",
            "evidence completeness does not match pinned and pressure-loss facts",
        ));
    }
    let declared_pinned = u64::try_from(summary.pinned.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "validate_capture_summary",
            "pinned frame count exceeds u64",
        )
    })?;
    if reason_counts.values().copied().sum::<u64>() != declared_pinned {
        return Err(ArtifactStoreError::fatal(
            "evidence_pinned_mismatch",
            "validate_capture_summary",
            "pinned reason distribution is inconsistent",
        ));
    }
    let mut pinned_indexes = BTreeSet::new();
    for pinned in &summary.pinned {
        if !pinned_indexes.insert(pinned.frame_index) {
            return Err(ArtifactStoreError::fatal(
                "evidence_pinned_mismatch",
                "validate_capture_summary",
                "pinned frame indexes must be unique",
            ));
        }
        match &pinned.artifact {
            Some(artifact) => {
                let Some(frame) = summary
                    .frames
                    .iter()
                    .find(|frame| frame.frame_index == pinned.frame_index)
                else {
                    return Err(ArtifactStoreError::fatal(
                        "evidence_pinned_mismatch",
                        "validate_capture_summary",
                        "pinned artifact is absent from persisted frame evidence",
                    ));
                };
                if frame.pinned_reason != Some(pinned.reason)
                    || frame.artifact.artifact_id() != artifact.artifact_id()
                {
                    return Err(ArtifactStoreError::fatal(
                        "evidence_pinned_mismatch",
                        "validate_capture_summary",
                        "pinned artifact metadata is inconsistent",
                    ));
                }
            }
            None if !missing.iter().any(|missing| {
                missing.frame_index == pinned.frame_index && missing.reason == pinned.reason
            }) =>
            {
                return Err(ArtifactStoreError::fatal(
                    "evidence_pinned_mismatch",
                    "validate_capture_summary",
                    "missing pinned frame was not accounted",
                ));
            }
            None => {}
        }
    }
    for frame in &summary.frames {
        if let Some(reason) = frame.pinned_reason
            && !summary
                .pinned
                .iter()
                .any(|pinned| pinned.frame_index == frame.frame_index && pinned.reason == reason)
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_pinned_mismatch",
                "validate_capture_summary",
                "persisted pinned frame is absent from pinned accounting",
            ));
        }
    }
    Ok(())
}

fn pinned_accounting(
    summary: &CapturePipelineSummary,
) -> (BTreeMap<PinnedFrameReason, u64>, Vec<MissingPinnedFrame>) {
    let mut counts = BTreeMap::new();
    let mut missing = Vec::new();
    for pinned in &summary.pinned {
        *counts.entry(pinned.reason).or_insert(0) += 1;
        if pinned.artifact.is_none() {
            missing.push(MissingPinnedFrame {
                frame_index: pinned.frame_index,
                reason: pinned.reason,
            });
        }
    }
    missing.sort_by_key(|frame| frame.frame_index);
    (counts, missing)
}

fn artifact_count(summary: &CapturePipelineSummary) -> ArtifactStoreResult<u64> {
    u64::try_from(summary.frames.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "count_evidence_artifacts",
            "artifact count exceeds u64",
        )
    })
}

fn terminal_event_type(outcome: TaskOutcome) -> EventType {
    match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    }
}

fn events_jsonl(events: &[ProjectedEvent]) -> ArtifactStoreResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, event).map_err(|error| {
            ArtifactStoreError::fatal(
                "evidence_event_invalid",
                "serialize_evidence_events",
                error.to_string(),
            )
        })?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn parse_events_jsonl(bytes: &[u8]) -> ArtifactStoreResult<Vec<ProjectedEvent>> {
    if bytes.is_empty() || !bytes.ends_with(b"\n") {
        return Err(ArtifactStoreError::fatal(
            "evidence_ledger_invalid",
            "parse_evidence_events",
            "events.jsonl must be non-empty and newline terminated",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_ledger_invalid",
            "parse_evidence_events",
            error.to_string(),
        )
    })?;
    text.strip_suffix('\n')
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "evidence_ledger_invalid",
                "parse_evidence_events",
                "events.jsonl newline terminator is invalid",
            )
        })?
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                return Err(ArtifactStoreError::fatal(
                    "evidence_ledger_invalid",
                    "parse_evidence_events",
                    "events.jsonl contains an empty record",
                ));
            }
            serde_json::from_str(line).map_err(|error| {
                ArtifactStoreError::fatal(
                    "evidence_ledger_invalid",
                    "parse_evidence_events",
                    error.to_string(),
                )
            })
        })
        .collect()
}

fn entry_digest(path: &str, bytes: &[u8]) -> ArtifactStoreResult<EvidenceArchiveEntry> {
    validate_archive_path(path)?;
    Ok(EvidenceArchiveEntry {
        path: path.to_string(),
        byte_count: u64::try_from(bytes.len()).map_err(|_| {
            ArtifactStoreError::fatal(
                "evidence_count_overflow",
                "hash_evidence_entry",
                "evidence entry byte count exceeds u64",
            )
        })?,
        sha256: canonical_sha256(bytes),
    })
}

fn normalize_output_path(path: &Path) -> ArtifactStoreResult<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        ArtifactStoreError::fatal(
            "evidence_output_invalid",
            "normalize_evidence_output",
            "evidence output path must include a file name",
        )
    })?;
    if file_name.to_str().is_none() {
        return Err(ArtifactStoreError::fatal(
            "evidence_output_invalid",
            "normalize_evidence_output",
            "evidence output file name is not valid UTF-8",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "evidence_output_invalid",
                    "resolve_evidence_output",
                    error.to_string(),
                )
            })?
            .join(path)
    };
    let parent = absolute.parent().ok_or_else(|| {
        ArtifactStoreError::fatal(
            "evidence_output_invalid",
            "normalize_evidence_output",
            "evidence output path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_output_failed",
            "create_evidence_output_directory",
            error.to_string(),
        )
    })?;
    let parent = parent.canonicalize().map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_output_invalid",
            "canonicalize_evidence_output_directory",
            error.to_string(),
        )
    })?;
    Ok(parent.join(file_name))
}

fn create_export_temp(output_path: &Path) -> ArtifactStoreResult<(PathBuf, File)> {
    let parent = output_path.parent().ok_or_else(|| {
        ArtifactStoreError::fatal(
            "evidence_output_invalid",
            "create_evidence_temp",
            "evidence output path has no parent directory",
        )
    })?;
    let file_name = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "evidence_output_invalid",
                "create_evidence_temp",
                "evidence output file name is not valid UTF-8",
            )
        })?;
    for _ in 0..TEMP_PATH_ATTEMPTS {
        let nonce = NEXT_EXPORT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ArtifactStoreError::fatal(
                    "evidence_archive_write_failed",
                    "create_evidence_temp",
                    error.to_string(),
                ));
            }
        }
    }
    Err(ArtifactStoreError::fatal(
        "evidence_temp_exhausted",
        "create_evidence_temp",
        "could not allocate a unique evidence temporary file",
    ))
}

fn write_archive(
    file: File,
    entries: &BTreeMap<String, Vec<u8>>,
    manifest: &[u8],
) -> ArtifactStoreResult<()> {
    let options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let mut zip = ZipWriter::new(file);
    for (path, bytes) in entries {
        validate_archive_path(path)?;
        zip.start_file(path, options).map_err(zip_write_error)?;
        zip.write_all(bytes).map_err(|error| {
            ArtifactStoreError::fatal(
                "evidence_archive_write_failed",
                "write_evidence_entry",
                error.to_string(),
            )
        })?;
    }
    zip.start_file(EVIDENCE_MANIFEST_PATH, options)
        .map_err(zip_write_error)?;
    zip.write_all(manifest).map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_archive_write_failed",
            "write_evidence_manifest",
            error.to_string(),
        )
    })?;
    let file = zip.finish().map_err(zip_write_error)?;
    file.sync_all().map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_archive_sync_failed",
            "sync_evidence_temp",
            error.to_string(),
        )
    })
}

fn publish_archive(temp_path: &Path, output_path: &Path) -> ArtifactStoreResult<()> {
    fs::hard_link(temp_path, output_path).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
            "evidence_output_collision"
        } else {
            "evidence_archive_publish_failed"
        };
        ArtifactStoreError::fatal(code, "publish_evidence_archive", error.to_string())
    })?;
    if let Err(error) = fs::remove_file(temp_path) {
        let error = ArtifactStoreError::fatal(
            "evidence_temp_cleanup_failed",
            "publish_evidence_archive",
            error.to_string(),
        );
        return Err(cleanup_file(output_path, "rollback_evidence_output", error));
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(output_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            cleanup_file(
                output_path,
                "rollback_evidence_output",
                ArtifactStoreError::fatal(
                    "evidence_archive_sync_failed",
                    "sync_published_evidence",
                    error.to_string(),
                ),
            )
        })
}

fn inspect_evidence_archive(path: &Path) -> ArtifactStoreResult<EvidenceArchiveVerification> {
    let bytes = fs::read(path).map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_archive_read_failed",
            "read_evidence_archive",
            error.to_string(),
        )
    })?;
    let zip_byte_count = u64::try_from(bytes.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "verify_evidence_archive",
            "evidence ZIP byte count exceeds u64",
        )
    })?;
    let zip_sha256 = canonical_sha256(&bytes);
    let mut zip = ZipArchive::new(std::io::Cursor::new(&bytes)).map_err(zip_read_error)?;
    let mut archived = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(zip_read_error)?;
        if entry.is_dir() {
            return Err(ArtifactStoreError::fatal(
                "evidence_archive_invalid",
                "verify_evidence_archive",
                "evidence ZIP must not contain directory entries",
            ));
        }
        let name = entry.name().to_string();
        validate_archive_path(&name)?;
        let mut entry_bytes = Vec::new();
        entry.read_to_end(&mut entry_bytes).map_err(|error| {
            ArtifactStoreError::fatal(
                "evidence_archive_read_failed",
                "read_evidence_entry",
                error.to_string(),
            )
        })?;
        if archived.insert(name, entry_bytes).is_some() {
            return Err(ArtifactStoreError::fatal(
                "evidence_entry_collision",
                "verify_evidence_archive",
                "evidence ZIP contains a duplicate entry name",
            ));
        }
    }
    let manifest_bytes = archived.remove(EVIDENCE_MANIFEST_PATH).ok_or_else(|| {
        ArtifactStoreError::fatal(
            "evidence_manifest_missing",
            "verify_evidence_archive",
            "evidence ZIP is missing evidence/manifest.json",
        )
    })?;
    let manifest_sha256 = canonical_sha256(&manifest_bytes);
    let manifest: EvidenceManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        ArtifactStoreError::fatal(
            "evidence_manifest_invalid",
            "parse_evidence_manifest",
            error.to_string(),
        )
    })?;
    validate_manifest(&manifest, &archived)?;
    Ok(EvidenceArchiveVerification {
        manifest,
        zip_byte_count,
        zip_sha256,
        manifest_sha256,
    })
}

fn validate_manifest(
    manifest: &EvidenceManifest,
    archived: &BTreeMap<String, Vec<u8>>,
) -> ArtifactStoreResult<()> {
    if manifest.schema_version != EVIDENCE_SCHEMA_VERSION
        || manifest.ledger_sequence_start == 0
        || manifest.ledger_sequence_end < manifest.ledger_sequence_start
        || manifest.terminal_receipt.sequence < manifest.ledger_sequence_start
        || manifest.terminal_receipt.sequence > manifest.ledger_sequence_end
        || manifest.terminal_receipt.event_type != terminal_event_type(manifest.task_outcome)
        || manifest.terminal_receipt.links.run_id() != Some(&manifest.run_id)
        || manifest.terminal_receipt.links.correlation_id() != Some(&manifest.correlation_id)
        || !Path::new(&manifest.normalized_output_path).is_absolute()
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_manifest_invalid",
            "verify_evidence_manifest",
            "evidence manifest identity, ledger bounds, terminal receipt, or output path is invalid",
        ));
    }
    let validated_package = EvidencePackage::new(
        manifest.package.file_name(),
        manifest.package.sha256(),
        manifest.package.verification(),
    )?;
    if validated_package != manifest.package {
        return Err(ArtifactStoreError::fatal(
            "evidence_package_invalid",
            "verify_evidence_manifest",
            "manifest package metadata is not canonical",
        ));
    }
    let event_bytes = archived.get("evidence/events.jsonl").ok_or_else(|| {
        ArtifactStoreError::fatal(
            "evidence_ledger_missing",
            "verify_evidence_manifest",
            "evidence/events.jsonl is missing",
        )
    })?;
    let events = parse_events_jsonl(event_bytes)?;
    validate_projected_events(
        &events,
        manifest.run_id,
        manifest.correlation_id,
        &manifest.terminal_receipt,
        manifest.task_outcome,
    )?;
    if events.first().map(|event| event.sequence) != Some(manifest.ledger_sequence_start)
        || events.last().map(|event| event.sequence) != Some(manifest.ledger_sequence_end)
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_ledger_invalid",
            "verify_evidence_manifest",
            "manifest ledger bounds do not match archived projected events",
        ));
    }
    let artifact_count = u64::try_from(manifest.screenshots.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "verify_evidence_manifest",
            "evidence screenshot count exceeds u64",
        )
    })?;
    if manifest.artifact_count != artifact_count
        || manifest.screenshot_counts.persisted != artifact_count
        || manifest.pinned_count != manifest.pinned_reason_counts.values().copied().sum::<u64>()
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_count_mismatch",
            "verify_evidence_manifest",
            "manifest artifact, screenshot, or pinned counts are inconsistent",
        ));
    }
    let missing_count = u64::try_from(manifest.missing_pinned.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "verify_evidence_manifest",
            "missing pinned frame count exceeds u64",
        )
    })?;
    let accounted = manifest
        .screenshot_counts
        .persisted
        .checked_add(manifest.screenshot_counts.deduplicated)
        .and_then(|count| count.checked_add(missing_count))
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "evidence_count_overflow",
                "verify_evidence_manifest",
                "manifest capture accounting exceeds u64",
            )
        })?;
    if manifest.screenshot_counts.captured != accounted {
        return Err(ArtifactStoreError::fatal(
            "evidence_count_mismatch",
            "verify_evidence_manifest",
            "manifest captured count does not match persisted, deduplicated, and missing frames",
        ));
    }
    let expected_completeness = if !manifest.missing_pinned.is_empty() {
        EvidenceCompleteness::Failed
    } else if manifest.screenshot_counts.dropped > 0 {
        EvidenceCompleteness::Partial
    } else {
        EvidenceCompleteness::Complete
    };
    if manifest.evidence_completeness != expected_completeness {
        return Err(ArtifactStoreError::fatal(
            "evidence_completeness_mismatch",
            "verify_evidence_manifest",
            "manifest evidence completeness is inconsistent",
        ));
    }

    let mut expected_paths = BTreeSet::new();
    for entry in &manifest.entries {
        validate_archive_path(&entry.path)?;
        if !expected_paths.insert(entry.path.clone()) || !is_sha256(&entry.sha256) {
            return Err(ArtifactStoreError::fatal(
                "evidence_manifest_invalid",
                "verify_evidence_manifest",
                "manifest entry paths or hashes are invalid",
            ));
        }
        let bytes = archived.get(&entry.path).ok_or_else(|| {
            ArtifactStoreError::fatal(
                "evidence_entry_missing",
                "verify_evidence_manifest",
                "manifest-declared archive entry is missing",
            )
        })?;
        let byte_count = u64::try_from(bytes.len()).map_err(|_| {
            ArtifactStoreError::fatal(
                "evidence_count_overflow",
                "verify_evidence_manifest",
                "archive entry byte count exceeds u64",
            )
        })?;
        if byte_count != entry.byte_count || canonical_sha256(bytes) != entry.sha256 {
            return Err(ArtifactStoreError::fatal(
                "evidence_entry_hash_mismatch",
                "verify_evidence_manifest",
                "archive entry byte count or SHA-256 does not match the manifest",
            ));
        }
    }
    if expected_paths != archived.keys().cloned().collect() {
        return Err(ArtifactStoreError::fatal(
            "evidence_entry_set_mismatch",
            "verify_evidence_manifest",
            "evidence ZIP contains undeclared or missing entries",
        ));
    }
    let content_hash =
        canonical_sha256(&serde_json::to_vec(&manifest.entries).map_err(|error| {
            ArtifactStoreError::fatal(
                "evidence_manifest_invalid",
                "verify_evidence_content_hash",
                error.to_string(),
            )
        })?);
    if content_hash != manifest.archive_content_sha256 {
        return Err(ArtifactStoreError::fatal(
            "evidence_content_hash_mismatch",
            "verify_evidence_manifest",
            "canonical archive content hash does not match the manifest",
        ));
    }
    let mut screenshot_frames = BTreeSet::new();
    let mut screenshot_artifacts = BTreeSet::new();
    let mut screenshot_paths = BTreeSet::new();
    let mut actual_pinned_reasons = BTreeMap::new();
    for screenshot in &manifest.screenshots {
        if !screenshot_frames.insert(screenshot.frame_index)
            || !screenshot_artifacts.insert(screenshot.artifact.artifact_id)
            || !screenshot_paths.insert(screenshot.archive_path.as_str())
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_screenshot_invalid",
                "verify_evidence_manifest",
                "screenshot frame, artifact, and archive-path identities must be unique",
            ));
        }
        let entry = manifest
            .entries
            .iter()
            .find(|entry| entry.path == screenshot.archive_path)
            .ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "evidence_screenshot_missing",
                    "verify_evidence_manifest",
                    "screenshot is absent from the entry digest list",
                )
            })?;
        if screenshot.artifact.kind != ArtifactKind::CaptureFrame
            || screenshot.artifact.object_key.is_none()
            || screenshot.artifact.sha256 != entry.sha256
            || screenshot.artifact.byte_count != entry.byte_count
            || screenshot.artifact.retention_class != manifest.retention_class
            || screenshot.artifact.run_id != Some(manifest.run_id)
            || screenshot.artifact.correlation_id != Some(manifest.correlation_id)
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_screenshot_invalid",
                "verify_evidence_manifest",
                "screenshot artifact metadata does not match archived bytes",
            ));
        }
        if let Some(reason) = screenshot.pinned_reason {
            *actual_pinned_reasons.entry(reason).or_insert(0) += 1;
        }
    }
    let mut missing_frames = BTreeSet::new();
    for missing in &manifest.missing_pinned {
        if !missing_frames.insert(missing.frame_index)
            || screenshot_frames.contains(&missing.frame_index)
        {
            return Err(ArtifactStoreError::fatal(
                "evidence_pinned_mismatch",
                "verify_evidence_manifest",
                "missing pinned frame identities must be unique and absent from screenshots",
            ));
        }
        *actual_pinned_reasons.entry(missing.reason).or_insert(0) += 1;
    }
    if actual_pinned_reasons != manifest.pinned_reason_counts {
        return Err(ArtifactStoreError::fatal(
            "evidence_pinned_mismatch",
            "verify_evidence_manifest",
            "pinned reason distribution does not match screenshot and missing-frame evidence",
        ));
    }
    Ok(())
}

fn validate_archive_path(path: &str) -> ArtifactStoreResult<()> {
    let candidate = Path::new(path);
    let valid_prefix = path.starts_with("evidence/") || path.starts_with("screenshots/");
    if path.is_empty()
        || path.contains('\\')
        || candidate.is_absolute()
        || !valid_prefix
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_entry_path_invalid",
            "validate_evidence_entry_path",
            "evidence archive entry path is unsafe",
        ));
    }
    Ok(())
}

fn normalize_sha256(value: &str) -> ArtifactStoreResult<String> {
    let normalized = if value.starts_with("sha256:") {
        value.to_string()
    } else {
        format!("sha256:{value}")
    };
    if is_sha256(&normalized) {
        Ok(normalized)
    } else {
        Err(ArtifactStoreError::fatal(
            "sha256_invalid",
            "normalize_sha256",
            "SHA-256 must contain exactly 64 lowercase hexadecimal digits",
        ))
    }
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn cleanup_file(
    path: &Path,
    operation: &'static str,
    error: ArtifactStoreError,
) -> ArtifactStoreError {
    match fs::remove_file(path) {
        Ok(()) => error,
        Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => error,
        Err(remove_error) => error.with_secondary(&ArtifactStoreError::fatal(
            "evidence_cleanup_failed",
            operation,
            remove_error.to_string(),
        )),
    }
}

fn zip_write_error(error: zip::result::ZipError) -> ArtifactStoreError {
    ArtifactStoreError::fatal(
        "evidence_archive_write_failed",
        "write_evidence_archive",
        error.to_string(),
    )
}

fn zip_read_error(error: zip::result::ZipError) -> ArtifactStoreError {
    ArtifactStoreError::fatal(
        "evidence_archive_invalid",
        "read_evidence_archive",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PersistedFrameEvidence, PinnedFrameEvidence};
    use actingcommand_contract::{
        ArtifactLinksDraft, EffectDisposition, EventAction, EventLinksDraft, EventPayloadDraft,
        IssuedCorrelationId, IssuedFrameId, IssuedRunId, ProjectionPayload, SanitizationError,
        SecretField, SecretFingerprinter, Sha256Fingerprint, TaskPayloadDraft,
    };
    use serde::Serialize;

    #[derive(Default)]
    struct RecordingSink {
        event_types: Vec<EventType>,
        fail_next: Option<EventType>,
    }

    impl ArtifactEventSink for RecordingSink {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            let sanitized = draft.sanitize(&TestFingerprinter).map_err(|error| {
                ArtifactStoreError::fatal("event_sanitize_failed", "test_sink", error.to_string())
            })?;
            if self.fail_next == Some(sanitized.event_type()) {
                self.fail_next = None;
                return Err(ArtifactStoreError::fatal(
                    "injected_event_failure",
                    "test_sink",
                    "injected evidence event failure",
                ));
            }
            self.event_types.push(sanitized.event_type());
            Ok(())
        }
    }

    struct TestFingerprinter;

    impl SecretFingerprinter for TestFingerprinter {
        fn fingerprint(
            &self,
            _field: SecretField,
            original: &str,
        ) -> Result<Sha256Fingerprint, SanitizationError> {
            Sha256Fingerprint::new(format!("sha256:{}", "a".repeat(64)), original)
        }
    }

    #[derive(Clone, Copy)]
    struct TestIdentity {
        run: IssuedRunId,
        correlation: IssuedCorrelationId,
    }

    #[test]
    fn success_failure_and_cancelled_exports_have_verified_final_receipts() {
        for (index, outcome) in [
            TaskOutcome::Success,
            TaskOutcome::Failure,
            TaskOutcome::Cancelled,
        ]
        .into_iter()
        .enumerate()
        {
            let temp = tempfile::tempdir().expect("tempdir");
            let mut sink = RecordingSink::default();
            let identity = test_identity();
            let frame = store_frame(
                temp.path().join("artifacts"),
                identity,
                1,
                1_752_147_200_123,
                format!("frame-{index}").as_bytes(),
                &mut sink,
            );
            let summary = complete_summary(vec![(1, frame)], None);
            let request = export_request(
                temp.path().join(format!("outcome-{index}.zip")),
                identity,
                outcome,
                summary,
            );
            let mut exporter =
                EvidenceExporter::open(temp.path().join("artifacts")).expect("exporter");

            let receipt = exporter.export(request, &mut sink).expect("export");
            let verified = verify_evidence_archive(receipt.output_path(), receipt.zip_sha256())
                .expect("verify exported archive");

            assert_eq!(verified.manifest.task_outcome, outcome);
            assert_eq!(
                verified.manifest.evidence_completeness,
                EvidenceCompleteness::Complete
            );
            assert_eq!(receipt.archive().kind(), ArtifactKind::EvidenceArchive);
            assert_eq!(receipt.archive().sha256(), receipt.zip_sha256());
            assert_eq!(receipt.zip_byte_count(), verified.zip_byte_count);
            assert!(
                sink.event_types
                    .contains(&EventType::ArtifactExportCompleted)
            );
            assert!(!sink.event_types.contains(&EventType::ArtifactExportFailed));
        }
    }

    #[test]
    fn same_millisecond_screenshots_receive_collision_suffixes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let identity = test_identity();
        let artifact_root = temp.path().join("artifacts");
        let first = store_frame(
            &artifact_root,
            identity,
            1,
            1_752_147_200_123,
            b"first-frame",
            &mut sink,
        );
        let second = store_frame(
            &artifact_root,
            identity,
            2,
            1_752_147_200_123,
            b"second-frame",
            &mut sink,
        );
        let request = export_request(
            temp.path().join("same-millisecond.zip"),
            identity,
            TaskOutcome::Success,
            complete_summary(vec![(1, first), (2, second)], None),
        );
        let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

        let receipt = exporter.export(request, &mut sink).expect("export");
        let names = receipt
            .manifest()
            .screenshots
            .iter()
            .map(|screenshot| screenshot.archive_path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            [
                "screenshots/20250710113320123.png",
                "screenshots/20250710113320123-01.png",
            ]
        );
    }

    #[test]
    fn pressure_loss_is_partial_and_missing_pinned_evidence_is_failed() {
        let partial_temp = tempfile::tempdir().expect("partial tempdir");
        let mut partial_sink = RecordingSink::default();
        let partial_identity = test_identity();
        let partial_root = partial_temp.path().join("artifacts");
        let frame = store_frame(
            &partial_root,
            partial_identity,
            1,
            1_752_147_200_123,
            b"retained-frame",
            &mut partial_sink,
        );
        let mut partial_summary = complete_summary(vec![(1, frame)], None);
        partial_summary.counts.dropped = 3;
        partial_summary.evidence_completeness = EvidenceCompleteness::Partial;
        let partial_request = export_request(
            partial_temp.path().join("partial.zip"),
            partial_identity,
            TaskOutcome::Success,
            partial_summary,
        );
        let mut partial_exporter = EvidenceExporter::open(&partial_root).expect("exporter");
        let partial = partial_exporter
            .export(partial_request, &mut partial_sink)
            .expect("partial export");
        assert_eq!(
            partial.manifest().evidence_completeness,
            EvidenceCompleteness::Partial
        );
        assert_eq!(partial.manifest().screenshot_counts.dropped, 3);
        assert_eq!(partial.manifest().screenshot_counts.deduplicated, 0);

        let failed_temp = tempfile::tempdir().expect("failed tempdir");
        let mut failed_sink = RecordingSink::default();
        let failed_identity = test_identity();
        let failed_summary = CapturePipelineSummary {
            counts: CapturePipelineCounts {
                captured: 1,
                deduplicated: 0,
                dropped: 0,
                persisted: 0,
            },
            evidence_completeness: EvidenceCompleteness::Failed,
            pinned: vec![PinnedFrameEvidence {
                frame_index: 7,
                reason: PinnedFrameReason::Terminal,
                artifact: None,
            }],
            frames: Vec::new(),
        };
        let failed_request = export_request(
            failed_temp.path().join("failed.zip"),
            failed_identity,
            TaskOutcome::Failure,
            failed_summary,
        );
        let mut failed_exporter =
            EvidenceExporter::open(failed_temp.path().join("artifacts")).expect("exporter");
        let failed = failed_exporter
            .export(failed_request, &mut failed_sink)
            .expect("failed-evidence export");
        assert_eq!(
            failed.manifest().evidence_completeness,
            EvidenceCompleteness::Failed
        );
        assert_eq!(failed.manifest().missing_pinned.len(), 1);
        assert_eq!(failed.manifest().artifact_count, 0);
    }

    #[test]
    fn corrupt_source_artifact_fails_without_publishing_output() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let identity = test_identity();
        let artifact_root = temp.path().join("artifacts");
        let frame = store_frame(
            &artifact_root,
            identity,
            1,
            1_752_147_200_123,
            b"original-frame",
            &mut sink,
        );
        fs::write(artifact_root.join(frame.object_key()), b"corrupt-frame").expect("corrupt");
        let output = temp.path().join("corrupt-source.zip");
        let request = export_request(
            output.clone(),
            identity,
            TaskOutcome::Failure,
            complete_summary(vec![(1, frame)], None),
        );
        let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

        let error = exporter
            .export(request, &mut sink)
            .expect_err("corrupt source rejected");

        assert_eq!(error.code(), "artifact_hash_mismatch");
        assert!(!output.exists());
        assert!(sink.event_types.contains(&EventType::ArtifactExportFailed));
        assert!(
            !sink
                .event_types
                .contains(&EventType::ArtifactExportCompleted)
        );
    }

    #[test]
    fn output_collision_preserves_existing_file_and_records_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let identity = test_identity();
        let artifact_root = temp.path().join("artifacts");
        let frame = store_frame(
            &artifact_root,
            identity,
            1,
            1_752_147_200_123,
            b"frame",
            &mut sink,
        );
        let output = temp.path().join("collision.zip");
        fs::write(&output, b"existing").expect("existing output");
        let request = export_request(
            output.clone(),
            identity,
            TaskOutcome::Success,
            complete_summary(vec![(1, frame)], None),
        );
        let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

        let error = exporter
            .export(request, &mut sink)
            .expect_err("collision rejected");

        assert_eq!(error.code(), "evidence_output_collision");
        assert_eq!(fs::read(output).expect("existing bytes"), b"existing");
        assert!(sink.event_types.contains(&EventType::ArtifactExportFailed));
    }

    #[test]
    fn completed_event_failure_rolls_back_output_and_archive_object() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let identity = test_identity();
        let artifact_root = temp.path().join("artifacts");
        let frame = store_frame(
            &artifact_root,
            identity,
            1,
            1_752_147_200_123,
            b"frame",
            &mut sink,
        );
        let output = temp.path().join("event-failure.zip");
        let request = export_request(
            output.clone(),
            identity,
            TaskOutcome::Success,
            complete_summary(vec![(1, frame)], None),
        );
        let source_files = all_files(&artifact_root).len();
        sink.fail_next = Some(EventType::ArtifactExportCompleted);
        let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

        let error = exporter
            .export(request, &mut sink)
            .expect_err("completed event failure");

        assert_eq!(error.code(), "injected_event_failure");
        assert!(!output.exists());
        assert_eq!(all_files(&artifact_root).len(), source_files);
        assert!(sink.event_types.contains(&EventType::ArtifactExportFailed));
        assert!(
            !sink
                .event_types
                .contains(&EventType::ArtifactExportCompleted)
        );
    }

    #[test]
    fn output_directory_failure_is_fatal_and_records_export_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let identity = test_identity();
        let artifact_root = temp.path().join("artifacts");
        let frame = store_frame(
            &artifact_root,
            identity,
            1,
            1_752_147_200_123,
            b"frame",
            &mut sink,
        );
        let blocker = temp.path().join("not-a-directory");
        fs::write(&blocker, b"block").expect("blocker");
        let output = blocker.join("archive.zip");
        let request = export_request(
            output.clone(),
            identity,
            TaskOutcome::Failure,
            complete_summary(vec![(1, frame)], None),
        );
        let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

        let error = exporter
            .export(request, &mut sink)
            .expect_err("output directory failure");

        assert_eq!(error.code(), "evidence_output_failed");
        assert!(!output.exists());
        assert!(sink.event_types.contains(&EventType::ArtifactExportFailed));
        assert!(
            !sink
                .event_types
                .contains(&EventType::ArtifactExportCompleted)
        );
    }

    #[test]
    fn verifier_rehashes_every_declared_entry_not_only_the_outer_zip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let identity = test_identity();
        let artifact_root = temp.path().join("artifacts");
        let frame = store_frame(
            &artifact_root,
            identity,
            1,
            1_752_147_200_123,
            b"frame",
            &mut sink,
        );
        let request = export_request(
            temp.path().join("valid.zip"),
            identity,
            TaskOutcome::Success,
            complete_summary(vec![(1, frame)], None),
        );
        let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");
        let receipt = exporter.export(request, &mut sink).expect("export");
        let (mut entries, manifest) = read_zip_entries(receipt.output_path());
        let screenshot_path = entries
            .keys()
            .find(|path| path.starts_with("screenshots/"))
            .cloned()
            .expect("screenshot");
        entries.insert(screenshot_path, b"tampered-frame".to_vec());
        entries.remove(EVIDENCE_MANIFEST_PATH);
        let corrupt = temp.path().join("corrupt-entry.zip");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&corrupt)
            .expect("corrupt output");
        write_archive(file, &entries, &manifest).expect("rewrite corrupt archive");
        let actual_hash = canonical_sha256(&fs::read(&corrupt).expect("corrupt bytes"));

        let error = verify_evidence_archive(&corrupt, &actual_hash)
            .expect_err("entry hash mismatch rejected");

        assert_eq!(error.code(), "evidence_entry_hash_mismatch");
    }

    fn test_identity() -> TestIdentity {
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        TestIdentity {
            run: identifiers.mint_run_id().expect("run"),
            correlation: identifiers.mint_correlation_id().expect("correlation"),
        }
    }

    fn context(
        identity: TestIdentity,
        frame: Option<IssuedFrameId>,
        timestamp_unix_ms: u64,
    ) -> ArtifactWriteContext {
        let mut artifact_links = ArtifactLinksDraft::default()
            .with_run_id(identity.run)
            .with_correlation_id(identity.correlation);
        let mut event_links = EventLinksDraft::default()
            .with_run_id(identity.run)
            .with_correlation_id(identity.correlation);
        if let Some(frame) = frame {
            artifact_links = artifact_links.with_frame_id(frame);
            event_links = event_links.with_frame_id(frame);
        }
        ArtifactWriteContext::new(artifact_links, event_links, timestamp_unix_ms)
    }

    fn store_frame(
        artifact_root: impl AsRef<Path>,
        identity: TestIdentity,
        frame_index: usize,
        timestamp_unix_ms: u64,
        bytes: &[u8],
        sink: &mut RecordingSink,
    ) -> ArtifactReference {
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let frame = identifiers.mint_frame_id().expect("frame");
        let store = ArtifactStore::open(artifact_root).expect("store");
        store
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    bytes,
                    context(identity, Some(frame), timestamp_unix_ms),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::CapturePipeline,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::NotRequired,
                    ),
                ),
                sink,
            )
            .unwrap_or_else(|error| panic!("store frame {frame_index}: {error}"))
            .reference()
            .clone()
    }

    fn complete_summary(
        frames: Vec<(usize, ArtifactReference)>,
        pinned: Option<(usize, PinnedFrameReason)>,
    ) -> CapturePipelineSummary {
        let persisted = u64::try_from(frames.len()).expect("persisted count");
        let persisted_frames = frames
            .iter()
            .map(|(frame_index, artifact)| PersistedFrameEvidence {
                frame_index: *frame_index,
                pinned_reason: pinned
                    .filter(|(pinned_index, _)| pinned_index == frame_index)
                    .map(|(_, reason)| reason),
                artifact: artifact.clone(),
            })
            .collect::<Vec<_>>();
        let pinned = pinned
            .map(|(frame_index, reason)| PinnedFrameEvidence {
                frame_index,
                reason,
                artifact: frames
                    .iter()
                    .find(|(index, _)| *index == frame_index)
                    .map(|(_, artifact)| artifact.clone()),
            })
            .into_iter()
            .collect();
        CapturePipelineSummary {
            counts: CapturePipelineCounts {
                captured: persisted,
                deduplicated: 0,
                dropped: 0,
                persisted,
            },
            evidence_completeness: EvidenceCompleteness::Complete,
            pinned,
            frames: persisted_frames,
        }
    }

    fn export_request(
        output_path: PathBuf,
        identity: TestIdentity,
        outcome: TaskOutcome,
        pipeline: CapturePipelineSummary,
    ) -> EvidenceExportRequest {
        #[derive(Serialize)]
        struct Document<'a> {
            status: &'a str,
        }

        let terminal = projected_terminal(identity, outcome, 1);
        EvidenceExportRequest {
            output_path,
            identity: EvidenceExportIdentity {
                run_id: *identity.run.transport(),
                correlation_id: *identity.correlation.transport(),
                package: EvidencePackage::new(
                    "sealed-package.zip",
                    "b".repeat(64),
                    PackageVerification::Passed,
                )
                .expect("package"),
                task_outcome: outcome,
                terminal_receipt: terminal.clone(),
                projection_profile: ProjectionProfile::Forensic,
                retention_class: RetentionClass::DebugFull,
                archive_redaction_state: ArtifactRedactionState::NotRequired,
            },
            events: vec![terminal],
            pipeline,
            documents: EvidenceExportDocuments::new(
                EvidenceJsonDocument::from_serializable(&Document { status: "result" })
                    .expect("result"),
                EvidenceJsonDocument::from_serializable(&Document {
                    status: "diagnostics",
                })
                .expect("diagnostics"),
                "sealed evidence summary",
            )
            .expect("documents"),
            archive_context: context(identity, None, 1_752_147_201_000),
        }
    }

    fn projected_terminal(
        identity: TestIdentity,
        outcome: TaskOutcome,
        sequence: u64,
    ) -> ProjectedEvent {
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let payload: EventPayloadDraft = match outcome {
            TaskOutcome::Success => TaskPayloadDraft::completed(
                EventAction::CriticalTest,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            TaskOutcome::Failure => TaskPayloadDraft::failed(
                EventAction::CriticalTest,
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
            TaskOutcome::Cancelled => TaskPayloadDraft::cancelled(
                EventAction::CriticalTest,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        };
        let sanitized = EventDraft::new(
            identifiers.mint_event_id().expect("event"),
            1_752_147_200_900,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::System,
                OriginModule::ProcessTest,
                EventActor::System,
            ),
            EventLinksDraft::default()
                .with_run_id(identity.run)
                .with_correlation_id(identity.correlation),
            payload,
        )
        .sanitize(&TestFingerprinter)
        .expect("sanitize terminal");
        ProjectedEvent {
            schema_version: sanitized.schema_version().to_string(),
            sequence,
            event_id: *sanitized.event_id(),
            timestamp_unix_ms: sanitized.timestamp_unix_ms(),
            event_type: sanitized.event_type(),
            severity: sanitized.severity(),
            sensitivity: sanitized.sensitivity(),
            origin: sanitized.origin().clone(),
            links: sanitized.links().clone(),
            payload_schema: sanitized.payload_schema().to_string(),
            payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
            artifacts: sanitized
                .artifacts()
                .iter()
                .map(|artifact| artifact.project(true))
                .collect(),
        }
    }

    fn read_zip_entries(path: &Path) -> (BTreeMap<String, Vec<u8>>, Vec<u8>) {
        let file = File::open(path).expect("open zip");
        let mut zip = ZipArchive::new(file).expect("zip");
        let mut entries = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).expect("entry");
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("entry bytes");
            entries.insert(entry.name().to_string(), bytes);
        }
        let manifest = entries
            .get(EVIDENCE_MANIFEST_PATH)
            .cloned()
            .expect("manifest");
        (entries, manifest)
    }

    fn all_files(root: &Path) -> Vec<PathBuf> {
        if !root.exists() {
            return Vec::new();
        }
        let mut pending = vec![root.to_path_buf()];
        let mut files = Vec::new();
        while let Some(path) = pending.pop() {
            for entry in fs::read_dir(path).expect("read dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }
}
