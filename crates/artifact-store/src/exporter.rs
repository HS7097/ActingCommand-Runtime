// SPDX-License-Identifier: AGPL-3.0-only

use crate::evidence_archive::{
    EVIDENCE_MANIFEST_PATH, EVIDENCE_SCHEMA_VERSION, inspect_evidence_archive,
    validate_archive_path, validate_projected_events, verify_evidence_archive,
};
use crate::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, CapturePipelineCounts, CapturePipelineSummary,
    EvidenceArchiveEntry, EvidenceManifest, EvidencePackage, EvidencePinnedFrame,
    EvidenceScreenshot, EvidenceScreenshotCounts, MissingPinnedFrame, ScreenshotNameAllocator,
    StoredArtifact, canonical_sha256, capture_summary_record, validate_capture_pipeline_summary,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactPayloadDraft, ArtifactProducer,
    ArtifactRedactionState, ArtifactReference, AuditInput, CapturePayload, CorrelationId,
    DiagnosticCode, EventActor, EventDraft, EventOrigin, EventPayload, EventSeverity, EventSource,
    EventType, IdentifierIssuer, OriginModule, PinnedFrameReason, ProjectedEvent,
    ProjectionPayload, ProjectionProfile, RetentionClass, RunId, TaskOutcome,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};
const TEMP_PATH_ATTEMPTS: u64 = 1_024;

static NEXT_EXPORT_TEMP: AtomicU64 = AtomicU64::new(1);

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
    pub source_capture_summary_sequence: u64,
    pub pipeline: CapturePipelineSummary,
    pub documents: EvidenceExportDocuments,
    pub archive_context: ArtifactWriteContext,
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
        let pinned = request
            .pipeline
            .pinned
            .iter()
            .map(|pin| EvidencePinnedFrame {
                frame_index: pin.frame_index,
                reason: pin.reason,
                artifact: pin.artifact.as_ref().map(|artifact| artifact.project(true)),
            })
            .collect();
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
                source_capture_summary_sequence: request.source_capture_summary_sequence,
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
                pinned,
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

fn validate_request(request: &EvidenceExportRequest) -> ArtifactStoreResult<()> {
    validate_projected_events(
        &request.events,
        request.identity.run_id,
        request.identity.correlation_id,
        &request.identity.terminal_receipt,
        request.identity.task_outcome,
    )?;
    validate_pipeline_summary(&request.pipeline)?;
    validate_authoritative_capture_summary(request)
}

fn validate_authoritative_capture_summary(
    request: &EvidenceExportRequest,
) -> ArtifactStoreResult<()> {
    let summaries = request
        .events
        .iter()
        .filter(|event| event.event_type == EventType::CaptureSummaryCommitted)
        .collect::<Vec<_>>();
    let [event] = summaries.as_slice() else {
        return Err(ArtifactStoreError::fatal(
            if summaries.is_empty() {
                "evidence_capture_summary_missing"
            } else {
                "evidence_capture_summary_duplicate"
            },
            "validate_evidence_capture_summary",
            "evidence export requires exactly one authoritative capture summary",
        ));
    };
    if event.sequence != request.source_capture_summary_sequence
        || event.sequence >= request.identity.terminal_receipt.sequence
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_not_ready",
            "validate_evidence_capture_summary",
            "capture summary sequence is not covered before the terminal receipt",
        ));
    }
    if event.links.run_id() != Some(&request.identity.run_id)
        || event.links.correlation_id() != Some(&request.identity.correlation_id)
        || event.origin.source() != EventSource::Runtime
        || event.origin.module() != OriginModule::CapturePipeline
        || event.origin.actor() != EventActor::Runtime
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_conflict",
            "validate_evidence_capture_summary",
            "capture summary links do not match the export identity",
        ));
    }
    let ProjectionPayload::Full(payload) = &event.payload else {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_invalid",
            "validate_evidence_capture_summary",
            "capture summary requires a forensic full projection",
        ));
    };
    let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) = payload.as_ref() else {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_invalid",
            "validate_evidence_capture_summary",
            "capture summary event has an incompatible payload",
        ));
    };
    let expected = capture_summary_record(&request.pipeline)?;
    if payload.summary() != &expected {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_conflict",
            "validate_evidence_capture_summary",
            "capture summary payload does not match the supplied pipeline summary",
        ));
    }
    Ok(())
}

fn validate_pipeline_summary(summary: &CapturePipelineSummary) -> ArtifactStoreResult<()> {
    validate_capture_pipeline_summary(summary)
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
    missing.sort_by_key(|frame| (frame.frame_index, frame.reason));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackageVerification, PersistedFrameEvidence, PinnedFrameEvidence};
    use actingcommand_contract::{
        ArtifactLinksDraft, CapturePayloadDraft, EffectDisposition, EventAction, EventLinksDraft,
        EventPayloadDraft, EvidenceCompleteness, IssuedCorrelationId, IssuedFrameId, IssuedRunId,
        ProjectionPayload, SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
        TaskPayloadDraft,
    };
    use serde::Serialize;
    use std::collections::BTreeSet;
    use std::io::Read;
    use zip::ZipArchive;

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
        partial_summary.counts.captured = 4;
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
                frame_index: Some(7),
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
    fn authoritative_capture_summary_failures_publish_no_archive_or_completion() {
        for (case, expected_code) in [
            ("missing", "evidence_capture_summary_missing"),
            ("duplicate", "evidence_capture_summary_duplicate"),
            ("not-ready", "evidence_capture_summary_not_ready"),
            ("conflict", "evidence_capture_summary_conflict"),
            ("invalid", "evidence_capture_summary_invalid"),
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let mut sink = RecordingSink::default();
            let identity = test_identity();
            let artifact_root = temp.path().join("artifacts");
            let frame = store_frame(
                &artifact_root,
                identity,
                1,
                1_752_147_200_123,
                case.as_bytes(),
                &mut sink,
            );
            let output = temp.path().join(format!("{case}.zip"));
            let mut request = export_request(
                output.clone(),
                identity,
                TaskOutcome::Success,
                complete_summary(vec![(1, frame)], None),
            );
            match case {
                "missing" => {
                    request
                        .events
                        .retain(|event| event.event_type != EventType::CaptureSummaryCommitted);
                }
                "duplicate" => {
                    let mut duplicate = request.events[0].clone();
                    duplicate.sequence = 2;
                    request.events[1].sequence = 3;
                    request.identity.terminal_receipt = request.events[1].clone();
                    request.events.insert(1, duplicate);
                }
                "not-ready" => request.source_capture_summary_sequence = 2,
                "conflict" => {
                    let artifact = request.pipeline.frames[0].artifact.clone();
                    request.pipeline = complete_summary(
                        vec![(1, artifact)],
                        Some((1, PinnedFrameReason::Terminal)),
                    );
                }
                "invalid" => request.events[0].payload = ProjectionPayload::Omitted,
                _ => unreachable!(),
            }
            let files_before = all_files(&artifact_root)
                .into_iter()
                .collect::<BTreeSet<_>>();
            let mut exporter = EvidenceExporter::open(&artifact_root).expect("exporter");

            let error = exporter
                .export(request, &mut sink)
                .expect_err(&format!("{case}: invalid summary cannot export"));

            assert_eq!(error.code(), expected_code, "{case}");
            assert!(!output.exists(), "{case}: no evidence ZIP");
            assert_eq!(
                all_files(&artifact_root)
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                files_before,
                "{case}: no archive object"
            );
            assert!(
                !sink
                    .event_types
                    .contains(&EventType::ArtifactExportCompleted),
                "{case}: no completed export event"
            );
            assert!(
                sink.event_types.contains(&EventType::ArtifactExportFailed),
                "{case}: typed export failure"
            );
        }
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
                frame_index: Some(frame_index),
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

        let summary = projected_capture_summary(identity, &pipeline, 1);
        let terminal = projected_terminal(identity, outcome, 2);
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
            events: vec![summary, terminal],
            source_capture_summary_sequence: 1,
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

    fn projected_capture_summary(
        identity: TestIdentity,
        pipeline: &CapturePipelineSummary,
        sequence: u64,
    ) -> ProjectedEvent {
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let record = capture_summary_record(pipeline).expect("capture summary record");
        let sanitized = EventDraft::new(
            identifiers.mint_event_id().expect("event"),
            1_752_147_200_800,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::CapturePipeline,
                EventActor::Runtime,
            ),
            EventLinksDraft::default()
                .with_run_id(identity.run)
                .with_correlation_id(identity.correlation),
            CapturePayloadDraft::summary_committed(record, AuditInput::new()).into(),
        )
        .sanitize(&TestFingerprinter)
        .expect("sanitize capture summary");
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
            artifacts: Vec::new(),
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
