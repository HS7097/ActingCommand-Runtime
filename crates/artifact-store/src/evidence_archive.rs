// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ArtifactStoreError, ArtifactStoreResult, canonical_sha256};
use actingcommand_contract::{
    ArtifactKind, CapturePayload, CapturePersistedEvidence, CapturePinnedEvidence,
    CaptureSummaryRecord, CorrelationId, EventActor, EventPayload, EventSource, EventType,
    EvidenceCompleteness, OriginModule, PinnedFrameReason, ProjectedArtifactReference,
    ProjectedEvent, ProjectionPayload, ProjectionProfile, RetentionClass, RunId, TaskOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use zip::ZipArchive;

pub(crate) const EVIDENCE_MANIFEST_PATH: &str = "evidence/manifest.json";
pub(crate) const EVIDENCE_SCHEMA_VERSION: &str = "actingcommand.evidence.v2";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceScreenshotCounts {
    pub captured: u64,
    pub deduplicated: u64,
    pub dropped: u64,
    pub persisted: u64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_index: Option<usize>,
    pub reason: PinnedFrameReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePinnedFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_index: Option<usize>,
    pub reason: PinnedFrameReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ProjectedArtifactReference>,
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
    pub source_capture_summary_sequence: u64,
    pub task_outcome: TaskOutcome,
    pub evidence_completeness: EvidenceCompleteness,
    pub terminal_receipt: ProjectedEvent,
    pub artifact_count: u64,
    pub screenshots: Vec<EvidenceScreenshot>,
    pub screenshot_counts: EvidenceScreenshotCounts,
    pub pinned_count: u64,
    pub pinned_reason_counts: BTreeMap<PinnedFrameReason, u64>,
    pub pinned: Vec<EvidencePinnedFrame>,
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
pub struct EvidenceArchiveVerification {
    pub manifest: EvidenceManifest,
    pub zip_byte_count: u64,
    pub zip_sha256: String,
    pub manifest_sha256: String,
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

pub(crate) fn inspect_evidence_archive(
    path: &Path,
) -> ArtifactStoreResult<EvidenceArchiveVerification> {
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

pub(crate) fn validate_projected_events(
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

pub(crate) fn terminal_event_type(outcome: TaskOutcome) -> EventType {
    match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    }
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

fn validate_manifest(
    manifest: &EvidenceManifest,
    archived: &BTreeMap<String, Vec<u8>>,
) -> ArtifactStoreResult<()> {
    if manifest.schema_version != EVIDENCE_SCHEMA_VERSION
        || manifest.ledger_sequence_start == 0
        || manifest.ledger_sequence_end < manifest.ledger_sequence_start
        || manifest.source_capture_summary_sequence < manifest.ledger_sequence_start
        || manifest.source_capture_summary_sequence >= manifest.terminal_receipt.sequence
        || manifest.source_capture_summary_sequence > manifest.ledger_sequence_end
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
    let summary = manifest_capture_summary(manifest)?;
    let summary_events = events
        .iter()
        .filter(|event| event.event_type == EventType::CaptureSummaryCommitted)
        .collect::<Vec<_>>();
    let [summary_event] = summary_events.as_slice() else {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_invalid",
            "verify_evidence_manifest",
            "archive must contain exactly one authoritative capture summary",
        ));
    };
    let ProjectionPayload::Full(payload) = &summary_event.payload else {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_invalid",
            "verify_evidence_manifest",
            "archived capture summary is not a forensic full projection",
        ));
    };
    let EventPayload::Capture(CapturePayload::SummaryCommitted(payload)) = payload.as_ref() else {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_invalid",
            "verify_evidence_manifest",
            "archived capture summary payload is incompatible",
        ));
    };
    if summary_event.sequence != manifest.source_capture_summary_sequence
        || summary_event.links.run_id() != Some(&manifest.run_id)
        || summary_event.links.correlation_id() != Some(&manifest.correlation_id)
        || summary_event.origin.source() != EventSource::Runtime
        || summary_event.origin.module() != OriginModule::CapturePipeline
        || summary_event.origin.actor() != EventActor::Runtime
        || payload.summary() != &summary
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_capture_summary_conflict",
            "verify_evidence_manifest",
            "manifest capture summary does not match the archived ledger fact",
        ));
    }
    let artifact_count = u64::try_from(manifest.screenshots.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "verify_evidence_manifest",
            "evidence screenshot count exceeds u64",
        )
    })?;
    let pinned_count = u64::try_from(manifest.pinned.len()).map_err(|_| {
        ArtifactStoreError::fatal(
            "evidence_count_overflow",
            "verify_evidence_manifest",
            "evidence pinned count exceeds u64",
        )
    })?;
    if manifest.artifact_count != artifact_count
        || manifest.screenshot_counts.persisted != artifact_count
        || manifest.pinned_count != pinned_count
        || manifest.pinned_count != manifest.pinned_reason_counts.values().copied().sum::<u64>()
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_count_mismatch",
            "verify_evidence_manifest",
            "manifest artifact, screenshot, or pinned counts are inconsistent",
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
        let expected_legacy_reason = manifest
            .pinned
            .iter()
            .find(|pin| pin.frame_index == Some(screenshot.frame_index) && pin.artifact.is_some())
            .map(|pin| pin.reason);
        if screenshot.pinned_reason != expected_legacy_reason {
            return Err(ArtifactStoreError::fatal(
                "evidence_pinned_mismatch",
                "verify_evidence_manifest",
                "legacy screenshot pin projection differs from the authoritative pin mapping",
            ));
        }
    }
    let mut pin_pairs = BTreeSet::new();
    let mut actual_pinned_reasons = BTreeMap::new();
    let mut expected_missing = Vec::new();
    for pin in &manifest.pinned {
        if !pin_pairs.insert((pin.frame_index, pin.reason)) {
            return Err(ArtifactStoreError::fatal(
                "evidence_pinned_mismatch",
                "verify_evidence_manifest",
                "authoritative pinned frame/reason pairs must be unique",
            ));
        }
        *actual_pinned_reasons.entry(pin.reason).or_insert(0) += 1;
        match (&pin.frame_index, &pin.artifact) {
            (Some(frame_index), Some(artifact)) => {
                let screenshot = manifest
                    .screenshots
                    .iter()
                    .find(|screenshot| screenshot.frame_index == *frame_index)
                    .ok_or_else(|| {
                        ArtifactStoreError::fatal(
                            "evidence_pinned_mismatch",
                            "verify_evidence_manifest",
                            "pinned artifact is absent from screenshots",
                        )
                    })?;
                if &screenshot.artifact != artifact {
                    return Err(ArtifactStoreError::fatal(
                        "evidence_pinned_mismatch",
                        "verify_evidence_manifest",
                        "pinned artifact does not match the persisted screenshot",
                    ));
                }
            }
            (_, None) => expected_missing.push(MissingPinnedFrame {
                frame_index: pin.frame_index,
                reason: pin.reason,
            }),
            (None, Some(_)) => {
                return Err(ArtifactStoreError::fatal(
                    "evidence_pinned_mismatch",
                    "verify_evidence_manifest",
                    "pinned artifact is missing its frame index",
                ));
            }
        }
    }
    expected_missing.sort_by_key(|missing| (missing.frame_index, missing.reason));
    if actual_pinned_reasons != manifest.pinned_reason_counts
        || expected_missing != manifest.missing_pinned
    {
        return Err(ArtifactStoreError::fatal(
            "evidence_pinned_mismatch",
            "verify_evidence_manifest",
            "pinned reason or missing-frame accounting is inconsistent",
        ));
    }
    Ok(())
}

fn manifest_capture_summary(
    manifest: &EvidenceManifest,
) -> ArtifactStoreResult<CaptureSummaryRecord> {
    let frames = manifest
        .screenshots
        .iter()
        .map(|screenshot| {
            let frame_index = u64::try_from(screenshot.frame_index).map_err(|_| {
                ArtifactStoreError::fatal(
                    "evidence_count_overflow",
                    "verify_evidence_manifest",
                    "screenshot frame index exceeds u64",
                )
            })?;
            CapturePersistedEvidence::new(frame_index, screenshot.artifact.clone()).map_err(
                |error| {
                    ArtifactStoreError::fatal(
                        error.code(),
                        "verify_evidence_manifest",
                        error.to_string(),
                    )
                },
            )
        })
        .collect::<ArtifactStoreResult<Vec<_>>>()?;
    let pinned = manifest
        .pinned
        .iter()
        .map(|pin| {
            let frame_index = pin
                .frame_index
                .map(u64::try_from)
                .transpose()
                .map_err(|_| {
                    ArtifactStoreError::fatal(
                        "evidence_count_overflow",
                        "verify_evidence_manifest",
                        "pinned frame index exceeds u64",
                    )
                })?;
            CapturePinnedEvidence::new(frame_index, pin.reason, pin.artifact.clone()).map_err(
                |error| {
                    ArtifactStoreError::fatal(
                        error.code(),
                        "verify_evidence_manifest",
                        error.to_string(),
                    )
                },
            )
        })
        .collect::<ArtifactStoreResult<Vec<_>>>()?;
    CaptureSummaryRecord::new(
        manifest.screenshot_counts.captured,
        manifest.screenshot_counts.deduplicated,
        manifest.screenshot_counts.dropped,
        manifest.screenshot_counts.persisted,
        manifest.evidence_completeness,
        frames,
        pinned,
    )
    .map_err(|error| {
        ArtifactStoreError::fatal(error.code(), "verify_evidence_manifest", error.to_string())
    })
}

pub(crate) fn validate_archive_path(path: &str) -> ArtifactStoreResult<()> {
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

fn zip_read_error(error: zip::result::ZipError) -> ArtifactStoreError {
    ArtifactStoreError::fatal(
        "evidence_archive_invalid",
        "read_evidence_archive",
        error.to_string(),
    )
}
