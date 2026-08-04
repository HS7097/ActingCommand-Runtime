// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, FramePersistenceCandidate, FrameStore,
    FrameStoreConfig, FrameStoreEvent, FrameStoreFrameInput, FrameStoreOutcome, StoredArtifact,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactPayloadDraft, ArtifactProducer,
    ArtifactRedactionState, ArtifactReference, AuditInput, CapturePayloadDraft,
    CapturePersistedEvidence, CapturePinnedEvidence, CapturePolicyReason, CaptureSummaryRecord,
    DiagnosticCode, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSeverity,
    EventSource, EvidenceCompleteness, IdentifierIssuer, OriginModule, PinnedFrameReason,
    RetentionClass,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const DEFAULT_CAPTURE_CADENCE_MS: u64 = 300;

#[derive(Debug, Clone)]
pub struct CapturePipelineConfig {
    pub frame_store: FrameStoreConfig,
    pub cadence_ms: u64,
    pub retention_class: RetentionClass,
    pub policy_reason: CapturePolicyReason,
    pub redaction_state: ArtifactRedactionState,
}

impl Default for CapturePipelineConfig {
    fn default() -> Self {
        Self {
            frame_store: FrameStoreConfig::default(),
            cadence_ms: DEFAULT_CAPTURE_CADENCE_MS,
            retention_class: RetentionClass::Adaptive,
            policy_reason: CapturePolicyReason::Default,
            redaction_state: ArtifactRedactionState::Pending,
        }
    }
}

impl CapturePipelineConfig {
    fn validate(&self) -> ArtifactStoreResult<()> {
        if self.cadence_ms == 0 {
            return Err(ArtifactStoreError::fatal(
                "invalid_capture_policy",
                "open_capture_pipeline",
                "capture cadence must be positive",
            ));
        }
        self.frame_store.validate().map_err(|error| {
            ArtifactStoreError::fatal("invalid_frame_store_config", "open_capture_pipeline", error)
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CapturePipelineCounts {
    pub captured: u64,
    pub deduplicated: u64,
    pub dropped: u64,
    pub persisted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedFrameEvidence {
    pub frame_index: Option<usize>,
    pub reason: PinnedFrameReason,
    pub artifact: Option<ArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedFrameEvidence {
    pub frame_index: usize,
    pub pinned_reason: Option<PinnedFrameReason>,
    pub artifact: ArtifactReference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePipelineSummary {
    pub counts: CapturePipelineCounts,
    pub evidence_completeness: EvidenceCompleteness,
    pub pinned: Vec<PinnedFrameEvidence>,
    pub frames: Vec<PersistedFrameEvidence>,
}

pub fn build_capture_pipeline_summary(
    counts: CapturePipelineCounts,
    mut pinned: Vec<PinnedFrameEvidence>,
    mut frames: Vec<PersistedFrameEvidence>,
) -> ArtifactStoreResult<CapturePipelineSummary> {
    frames.sort_by_key(|frame| frame.frame_index);
    pinned.sort_by_key(|pin| (pin.frame_index, pin.reason));
    for frame in &mut frames {
        frame.pinned_reason = pinned
            .iter()
            .find(|pin| pin.frame_index == Some(frame.frame_index) && pin.artifact.is_some())
            .map(|pin| pin.reason);
    }
    let accounted = counts
        .deduplicated
        .checked_add(counts.dropped)
        .and_then(|count| count.checked_add(counts.persisted))
        .ok_or_else(|| {
            ArtifactStoreError::fatal(
                "capture_summary_count_overflow",
                "build_capture_summary",
                "capture accounting exceeds u64",
            )
        })?;
    let evidence_completeness =
        if pinned.iter().any(|pin| pin.artifact.is_none()) || accounted != counts.captured {
            EvidenceCompleteness::Failed
        } else if counts.dropped > 0 {
            EvidenceCompleteness::Partial
        } else {
            EvidenceCompleteness::Complete
        };
    let summary = CapturePipelineSummary {
        counts,
        evidence_completeness,
        pinned,
        frames,
    };
    validate_capture_pipeline_summary(&summary)?;
    Ok(summary)
}

pub fn capture_summary_record(
    summary: &CapturePipelineSummary,
) -> ArtifactStoreResult<CaptureSummaryRecord> {
    let frames = summary
        .frames
        .iter()
        .map(|frame| {
            let frame_index = u64::try_from(frame.frame_index).map_err(|_| {
                ArtifactStoreError::fatal(
                    "capture_summary_bound_exceeded",
                    "build_capture_summary_record",
                    "capture frame index exceeds u64",
                )
            })?;
            CapturePersistedEvidence::new(frame_index, frame.artifact.project(true)).map_err(
                |error| {
                    ArtifactStoreError::fatal(
                        error.code(),
                        "build_capture_summary_record",
                        error.to_string(),
                    )
                },
            )
        })
        .collect::<ArtifactStoreResult<Vec<_>>>()?;
    let pinned = summary
        .pinned
        .iter()
        .map(|pin| {
            let frame_index = pin
                .frame_index
                .map(u64::try_from)
                .transpose()
                .map_err(|_| {
                    ArtifactStoreError::fatal(
                        "capture_summary_bound_exceeded",
                        "build_capture_summary_record",
                        "pinned frame index exceeds u64",
                    )
                })?;
            CapturePinnedEvidence::new(
                frame_index,
                pin.reason,
                pin.artifact.as_ref().map(|artifact| artifact.project(true)),
            )
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    error.code(),
                    "build_capture_summary_record",
                    error.to_string(),
                )
            })
        })
        .collect::<ArtifactStoreResult<Vec<_>>>()?;
    CaptureSummaryRecord::new(
        summary.counts.captured,
        summary.counts.deduplicated,
        summary.counts.dropped,
        summary.counts.persisted,
        summary.evidence_completeness,
        frames,
        pinned,
    )
    .map_err(|error| {
        ArtifactStoreError::fatal(
            error.code(),
            "build_capture_summary_record",
            error.to_string(),
        )
    })
}

pub fn validate_capture_pipeline_summary(
    summary: &CapturePipelineSummary,
) -> ArtifactStoreResult<()> {
    capture_summary_record(summary).map(|_| ())
}

#[derive(Debug)]
pub struct CapturePipelineOutcome {
    pub frame: FrameStoreOutcome,
    pub persisted: Vec<ArtifactReference>,
    pub evidence_completeness: EvidenceCompleteness,
}

pub struct CapturePipeline {
    frame_store: FrameStore,
    artifact_store: ArtifactStore,
    event_ids: IdentifierIssuer,
    run_context: ArtifactWriteContext,
    contexts: BTreeMap<usize, ArtifactWriteContext>,
    persisted: BTreeMap<usize, ArtifactReference>,
    pinned: BTreeMap<usize, PinnedFrameReason>,
    missing_pinned: BTreeSet<usize>,
    counts: CapturePipelineCounts,
    retention_class: RetentionClass,
    redaction_state: ArtifactRedactionState,
    paused: bool,
}

impl CapturePipeline {
    pub fn open(
        artifact_root: impl AsRef<Path>,
        frame_temp_root: impl AsRef<Path>,
        config: CapturePipelineConfig,
        run_context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<Self> {
        config.validate()?;
        let mut pipeline = Self {
            frame_store: FrameStore::new(
                frame_temp_root.as_ref().to_path_buf(),
                config.frame_store,
            )?,
            artifact_store: ArtifactStore::open(artifact_root)?,
            event_ids: IdentifierIssuer::new().map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_issuer_failed",
                    "open_capture_pipeline",
                    error.to_string(),
                )
            })?,
            run_context,
            contexts: BTreeMap::new(),
            persisted: BTreeMap::new(),
            pinned: BTreeMap::new(),
            missing_pinned: BTreeSet::new(),
            counts: CapturePipelineCounts::default(),
            retention_class: config.retention_class,
            redaction_state: config.redaction_state,
            paused: false,
        };
        pipeline.append_event(
            sink,
            pipeline.run_context.event_links().clone(),
            pipeline.run_context.created_at_unix_ms(),
            EventSeverity::Info,
            CapturePayloadDraft::policy_changed(
                config.cadence_ms,
                config.retention_class,
                config.policy_reason,
                AuditInput::new(),
            )
            .into(),
        )?;
        Ok(pipeline)
    }

    pub const fn is_paused(&self) -> bool {
        self.paused
    }

    pub const fn counts(&self) -> CapturePipelineCounts {
        self.counts
    }

    pub fn record_frame(
        &mut self,
        input: FrameStoreFrameInput,
        context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<CapturePipelineOutcome> {
        let frame_index = input.frame_index;
        if self.contexts.contains_key(&frame_index) {
            return Err(ArtifactStoreError::fatal(
                "duplicate_frame_index",
                "record_capture_frame",
                format!("frame index {frame_index} already exists"),
            ));
        }
        if self.paused && input.pinned_reason.is_none() {
            return Err(ArtifactStoreError::fatal(
                "capture_pipeline_paused",
                "record_capture_frame",
                "ordinary capture is paused at Tier3; poll pressure before recording another frame",
            ));
        }
        if let Some(reason) = input.pinned_reason {
            self.pinned.insert(frame_index, reason);
        }
        self.contexts.insert(frame_index, context.clone());
        self.counts.captured = self.counts.captured.checked_add(1).ok_or_else(|| {
            ArtifactStoreError::fatal(
                "capture_summary_count_overflow",
                "record_capture_frame",
                "captured frame count exceeds u64",
            )
        })?;

        let frame = match self.frame_store.add_frame(input) {
            Ok(frame) => frame,
            Err(error) => {
                self.contexts.remove(&frame_index);
                self.pinned.remove(&frame_index);
                return Err(error);
            }
        };
        self.paused = frame.pause_required;
        self.emit_frame_store_events(&context, sink)?;
        let persisted = self.persist_candidates(false, sink)?;
        Ok(CapturePipelineOutcome {
            frame,
            persisted,
            evidence_completeness: self.evidence_completeness(),
        })
    }

    pub fn record_pressure_skip(&mut self, skipped_intervals: u64) -> ArtifactStoreResult<()> {
        if !self.paused {
            return Err(ArtifactStoreError::fatal(
                "capture_pipeline_not_paused",
                "record_pressure_skip",
                "pressure skips are valid only while Tier3 is paused",
            ));
        }
        if skipped_intervals == 0 {
            return Err(ArtifactStoreError::fatal(
                "invalid_pressure_skip",
                "record_pressure_skip",
                "skipped interval count must be positive",
            ));
        }
        self.counts.captured = self
            .counts
            .captured
            .checked_add(skipped_intervals)
            .ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "capture_summary_count_overflow",
                    "record_pressure_skip",
                    "captured frame count exceeds u64",
                )
            })?;
        self.counts.dropped = self
            .counts
            .dropped
            .checked_add(skipped_intervals)
            .ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "capture_summary_count_overflow",
                    "record_pressure_skip",
                    "dropped frame count exceeds u64",
                )
            })?;
        Ok(())
    }

    pub fn poll_pressure(
        &mut self,
        context: &ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<bool> {
        let resumed = self.frame_store.refresh_pressure()?;
        if resumed {
            self.paused = false;
        }
        self.emit_frame_store_events(context, sink)?;
        Ok(resumed)
    }

    pub fn finish(
        &mut self,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<CapturePipelineSummary> {
        self.persist_candidates(true, sink)?;
        self.summary()
    }

    pub fn summary(&self) -> ArtifactStoreResult<CapturePipelineSummary> {
        let pinned = self
            .pinned
            .iter()
            .map(|(frame_index, reason)| PinnedFrameEvidence {
                frame_index: Some(*frame_index),
                reason: *reason,
                artifact: self.persisted.get(frame_index).cloned(),
            })
            .collect();
        build_capture_pipeline_summary(
            self.counts,
            pinned,
            self.persisted
                .iter()
                .map(|(frame_index, artifact)| PersistedFrameEvidence {
                    frame_index: *frame_index,
                    pinned_reason: self.pinned.get(frame_index).copied(),
                    artifact: artifact.clone(),
                })
                .collect(),
        )
    }

    pub fn frame_store(&self) -> &FrameStore {
        &self.frame_store
    }

    fn persist_candidates(
        &mut self,
        include_all_retained: bool,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<Vec<ArtifactReference>> {
        let candidates = self
            .frame_store
            .persistence_candidates(include_all_retained)?;
        let mut stored = Vec::new();
        for candidate in candidates {
            match self.persist_candidate(&candidate, sink) {
                Ok(artifact) => {
                    self.frame_store
                        .mark_artifact_persisted(candidate.frame_index)?;
                    self.counts.persisted =
                        self.counts.persisted.checked_add(1).ok_or_else(|| {
                            ArtifactStoreError::fatal(
                                "capture_summary_count_overflow",
                                "persist_capture_frame",
                                "persisted frame count exceeds u64",
                            )
                        })?;
                    self.missing_pinned.remove(&candidate.frame_index);
                    self.persisted
                        .insert(candidate.frame_index, artifact.reference().clone());
                    stored.push(artifact.reference().clone());
                }
                Err(mut error) => {
                    if candidate.pinned_reason.is_some() {
                        self.missing_pinned.insert(candidate.frame_index);
                        if let Err(event_error) =
                            self.record_pinned_failure(candidate.frame_index, sink)
                        {
                            error = error.with_secondary(&event_error);
                        }
                    }
                    return Err(error);
                }
            }
        }
        Ok(stored)
    }

    fn persist_candidate(
        &self,
        candidate: &FramePersistenceCandidate,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<StoredArtifact> {
        let context = self.contexts.get(&candidate.frame_index).ok_or_else(|| {
            ArtifactStoreError::fatal(
                "missing_frame_context",
                "persist_capture_frame",
                format!("frame index {} has no typed context", candidate.frame_index),
            )
        })?;
        self.artifact_store.put(
            ArtifactWriteRequest::new(
                ArtifactKind::CaptureFrame,
                &candidate.png,
                context.clone(),
                ArtifactIssuePolicy::new(
                    ArtifactProducer::CapturePipeline,
                    self.retention_class,
                    self.redaction_state,
                ),
            ),
            sink,
        )
    }

    fn record_pinned_failure(
        &mut self,
        frame_index: usize,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<()> {
        let context = self.contexts.get(&frame_index).cloned().ok_or_else(|| {
            ArtifactStoreError::fatal(
                "missing_frame_context",
                "record_pinned_persistence_failure",
                format!("pinned frame index {frame_index} has no typed context"),
            )
        })?;
        self.append_event(
            sink,
            context.event_links().clone(),
            context.created_at_unix_ms(),
            EventSeverity::Error,
            ArtifactPayloadDraft::store_failed(
                DiagnosticCode::PinnedFrameMissing,
                AuditInput::new(),
            )
            .into(),
        )
    }

    fn emit_frame_store_events(
        &mut self,
        context: &ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<()> {
        for event in self.frame_store.drain_events() {
            let (links, payload) = match event {
                FrameStoreEvent::PressureChanged {
                    state,
                    memory_budget_bytes,
                    resident_bytes,
                } => (
                    context.event_links().clone(),
                    CapturePayloadDraft::pressure_changed(
                        state,
                        memory_budget_bytes,
                        resident_bytes,
                        AuditInput::new(),
                    ),
                ),
                FrameStoreEvent::DedupWindow {
                    representative_frame_index,
                    duplicate_count,
                    duration_ms,
                } => {
                    self.counts.deduplicated = self
                        .counts
                        .deduplicated
                        .checked_add(duplicate_count)
                        .ok_or_else(|| {
                            ArtifactStoreError::fatal(
                                "capture_summary_count_overflow",
                                "emit_capture_dedup_window",
                                "deduplicated frame count exceeds u64",
                            )
                        })?;
                    let representative = self.contexts.get(&representative_frame_index).ok_or_else(
                        || {
                            ArtifactStoreError::fatal(
                                "missing_frame_context",
                                "emit_capture_dedup_window",
                                format!(
                                    "representative frame index {representative_frame_index} has no typed context"
                                ),
                            )
                        },
                    )?;
                    (
                        representative.event_links().clone(),
                        CapturePayloadDraft::dedup_window(
                            duplicate_count,
                            duration_ms,
                            AuditInput::new(),
                        ),
                    )
                }
            };
            self.append_event(
                sink,
                links,
                context.created_at_unix_ms(),
                EventSeverity::Info,
                payload.into(),
            )?;
        }
        Ok(())
    }

    fn append_event(
        &mut self,
        sink: &mut dyn ArtifactEventSink,
        links: EventLinksDraft,
        timestamp_unix_ms: u64,
        severity: EventSeverity,
        payload: actingcommand_contract::EventPayloadDraft,
    ) -> ArtifactStoreResult<()> {
        let draft = EventDraft::new(
            self.event_ids.mint_event_id().map_err(|error| {
                ArtifactStoreError::fatal(
                    "event_issuer_failed",
                    "append_capture_pipeline_event",
                    error.to_string(),
                )
            })?,
            timestamp_unix_ms,
            severity,
            EventOrigin::new(
                EventSource::System,
                OriginModule::CapturePipeline,
                EventActor::System,
            ),
            links,
            payload,
        );
        sink.append(draft)
    }

    fn evidence_completeness(&self) -> EvidenceCompleteness {
        if !self.missing_pinned.is_empty()
            || self
                .pinned
                .keys()
                .any(|frame_index| !self.persisted.contains_key(frame_index))
        {
            EvidenceCompleteness::Failed
        } else if self.counts.dropped > 0 {
            EvidenceCompleteness::Partial
        } else {
            EvidenceCompleteness::Complete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactEventSink, ArtifactStoreError};
    use actingcommand_contract::{
        ArtifactLinksDraft, EventType, IssuedCorrelationId, IssuedFrameId, IssuedRunId,
        SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
    };
    use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};

    #[derive(Default)]
    struct RecordingSink {
        event_types: Vec<EventType>,
        fail_artifact_created: bool,
    }

    impl ArtifactEventSink for RecordingSink {
        fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
            let sanitized = draft.sanitize(&TestFingerprinter).map_err(|error| {
                ArtifactStoreError::fatal("event_sanitize_failed", "test_sink", error.to_string())
            })?;
            if self.fail_artifact_created && sanitized.event_type() == EventType::ArtifactCreated {
                return Err(ArtifactStoreError::fatal(
                    "injected_event_failure",
                    "test_sink",
                    "injected artifact-created failure",
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

    #[test]
    fn default_policy_is_300_ms_and_ledger_visible() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(1_000_000),
            context(1),
            &mut sink,
        )
        .expect("pipeline");

        assert!(!pipeline.is_paused());
        assert_eq!(sink.event_types, [EventType::CapturePolicyChanged]);
    }

    #[test]
    fn explicit_pinned_frame_bypasses_same_page_dedup_and_persists_immediately() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let mut pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(7_000),
            context(1),
            &mut sink,
        )
        .expect("pipeline");

        pipeline
            .record_frame(frame_input(1, None), context(2), &mut sink)
            .expect("ordinary frame");
        let outcome = pipeline
            .record_frame(
                frame_input(2, Some(PinnedFrameReason::RecognitionEvidence)),
                context(3),
                &mut sink,
            )
            .expect("pinned frame");

        assert!(outcome.frame.retained);
        assert_eq!(outcome.persisted.len(), 1);
        assert_eq!(pipeline.counts().deduplicated, 0);
        let summary = pipeline.summary().expect("capture summary");
        assert_eq!(
            summary.evidence_completeness,
            EvidenceCompleteness::Complete
        );
        assert_eq!(summary.pinned.len(), 1);
        assert!(summary.pinned[0].artifact.is_some());
    }

    #[test]
    fn summary_builder_is_deterministic_and_fail_closed_for_counts_and_pin_pairs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let mut pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(7_000),
            context(1),
            &mut sink,
        )
        .expect("pipeline");
        let artifact = pipeline
            .record_frame(
                frame_input(2, Some(PinnedFrameReason::RecognitionEvidence)),
                context(2),
                &mut sink,
            )
            .expect("persist source frame")
            .persisted
            .into_iter()
            .next()
            .expect("persisted artifact");
        let counts = CapturePipelineCounts {
            captured: 1,
            deduplicated: 0,
            dropped: 0,
            persisted: 1,
        };
        let frames = vec![PersistedFrameEvidence {
            frame_index: 2,
            pinned_reason: None,
            artifact: artifact.clone(),
        }];
        let pins = vec![
            PinnedFrameEvidence {
                frame_index: Some(2),
                reason: PinnedFrameReason::Terminal,
                artifact: Some(artifact.clone()),
            },
            PinnedFrameEvidence {
                frame_index: Some(2),
                reason: PinnedFrameReason::RecognitionEvidence,
                artifact: Some(artifact.clone()),
            },
        ];

        let first =
            build_capture_pipeline_summary(counts, pins.clone(), frames.clone()).expect("summary");
        let mut reversed = pins.clone();
        reversed.reverse();
        let second =
            build_capture_pipeline_summary(counts, reversed, frames.clone()).expect("summary");
        assert_eq!(first, second);
        assert_eq!(
            capture_summary_record(&first).expect("first record"),
            capture_summary_record(&second).expect("second record")
        );
        assert_eq!(first.evidence_completeness, EvidenceCompleteness::Complete);
        assert_eq!(first.pinned.len(), 2);

        let partial = build_capture_pipeline_summary(
            CapturePipelineCounts {
                captured: 2,
                deduplicated: 0,
                dropped: 1,
                persisted: 1,
            },
            pins.clone(),
            frames.clone(),
        )
        .expect("partial summary");
        assert_eq!(partial.evidence_completeness, EvidenceCompleteness::Partial);

        let failed = build_capture_pipeline_summary(
            counts,
            vec![PinnedFrameEvidence {
                frame_index: None,
                reason: PinnedFrameReason::Failure,
                artifact: None,
            }],
            frames.clone(),
        )
        .expect("failed summary");
        assert_eq!(failed.evidence_completeness, EvidenceCompleteness::Failed);

        let mut duplicate_pins = pins;
        duplicate_pins.push(duplicate_pins[0].clone());
        let duplicate = build_capture_pipeline_summary(counts, duplicate_pins, frames.clone())
            .expect_err("duplicate pin pair");
        assert_eq!(duplicate.code(), "capture_summary_pin_conflict");

        let count_conflict = build_capture_pipeline_summary(
            CapturePipelineCounts {
                persisted: 0,
                ..counts
            },
            Vec::new(),
            frames,
        )
        .expect_err("persisted count mismatch");
        assert_eq!(count_conflict.code(), "capture_summary_count_mismatch");
    }

    #[test]
    fn ordinary_same_page_frame_is_deduplicated_but_not_pressure_dropped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let mut pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(7_000),
            context(1),
            &mut sink,
        )
        .expect("pipeline");

        pipeline
            .record_frame(frame_input(1, None), context(2), &mut sink)
            .expect("first frame");
        let second = pipeline
            .record_frame(frame_input(2, None), context(3), &mut sink)
            .expect("second frame");

        assert!(!second.frame.retained);
        assert_eq!(pipeline.counts().deduplicated, 1);
        assert_eq!(pipeline.counts().dropped, 0);
        assert!(sink.event_types.contains(&EventType::CaptureDedupWindow));
    }

    #[test]
    fn tier3_pause_is_ledger_visible_and_pressure_skip_is_partial() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let mut pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(1_200),
            context(1),
            &mut sink,
        )
        .expect("pipeline");
        let mut pressure_frame = frame_input(1, None);
        pressure_frame.recognition_state = crate::RecognitionState::Pending;
        pipeline
            .record_frame(pressure_frame, context(2), &mut sink)
            .expect("pressure frame");

        assert!(pipeline.is_paused());
        assert!(
            sink.event_types
                .contains(&EventType::CapturePressureChanged)
        );
        pipeline.record_pressure_skip(2).expect("pressure skip");
        assert_eq!(pipeline.counts().dropped, 2);
        let summary = pipeline.finish(&mut sink).expect("finish pipeline");
        assert_eq!(summary.evidence_completeness, EvidenceCompleteness::Partial);
    }

    #[test]
    fn pinned_frame_persists_immediately_while_tier3_is_paused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let mut pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(1_200),
            context(1),
            &mut sink,
        )
        .expect("pipeline");
        let mut pressure_frame = frame_input(1, None);
        pressure_frame.recognition_state = crate::RecognitionState::Pending;
        pipeline
            .record_frame(pressure_frame, context(2), &mut sink)
            .expect("pressure frame");
        assert!(pipeline.is_paused());

        let outcome = pipeline
            .record_frame(
                frame_input(2, Some(PinnedFrameReason::Terminal)),
                context(3),
                &mut sink,
            )
            .expect("pinned frame during pause");

        assert!(outcome.frame.retained);
        assert_eq!(outcome.persisted.len(), 1);
        assert_eq!(pipeline.counts().deduplicated, 0);
        assert!(
            pipeline.summary().expect("capture summary").pinned[0]
                .artifact
                .is_some()
        );
    }

    #[test]
    fn pinned_persistence_failure_is_ledger_visible_and_evidence_failed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut sink = RecordingSink::default();
        let mut pipeline = CapturePipeline::open(
            temp.path().join("artifacts"),
            temp.path().join("frames"),
            config(7_000),
            context(1),
            &mut sink,
        )
        .expect("pipeline");

        let mut failing_sink = RecordingSink {
            fail_artifact_created: true,
            ..RecordingSink::default()
        };
        let error = pipeline
            .record_frame(
                frame_input(2, Some(PinnedFrameReason::Terminal)),
                context(3),
                &mut failing_sink,
            )
            .expect_err("pinned persistence failure");
        assert_eq!(error.code(), "injected_event_failure");
        assert_eq!(
            pipeline
                .summary()
                .expect("capture summary")
                .evidence_completeness,
            EvidenceCompleteness::Failed
        );
        assert!(
            failing_sink
                .event_types
                .contains(&EventType::ArtifactStoreFailed)
        );
    }

    fn config(max_mem_bytes: u64) -> CapturePipelineConfig {
        let mut frame_store = FrameStoreConfig::default();
        frame_store.similarity_threshold = 0.95;
        frame_store.tier1_ratio = 0.50;
        frame_store.tier2_ratio = 0.70;
        frame_store.tier3_ratio = 0.90;
        frame_store.hysteresis_ratio = 0.10;
        frame_store.max_mem_bytes = Some(max_mem_bytes);
        frame_store.os_reserve_bytes = 0;
        frame_store.flush_workspace_reserve_bytes = 1;
        let frame_store =
            frame_store.with_memory_source(crate::MemorySampleSource::fixed(crate::MemorySample {
                total_bytes: max_mem_bytes,
                available_bytes: max_mem_bytes,
            }));
        CapturePipelineConfig {
            frame_store,
            cadence_ms: DEFAULT_CAPTURE_CADENCE_MS,
            retention_class: RetentionClass::DebugFull,
            policy_reason: CapturePolicyReason::Default,
            redaction_state: ArtifactRedactionState::NotRequired,
        }
    }

    fn frame_input(
        frame_index: usize,
        pinned_reason: Option<PinnedFrameReason>,
    ) -> FrameStoreFrameInput {
        let frame = Frame::from_pixels(
            16,
            16,
            vec![17; 16 * 16 * 3],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame");
        FrameStoreFrameInput {
            frame_index,
            file_name: format!("frame-{frame_index}.png"),
            label: "steady".to_string(),
            recognition_state: crate::RecognitionState::Matched {
                page_id: "test/home".to_string(),
            },
            pinned_reason,
            frame,
        }
    }

    fn context(sequence: u64) -> ArtifactWriteContext {
        let identifiers = IdentifierIssuer::new().expect("identifiers");
        let run: IssuedRunId = identifiers.mint_run_id().expect("run");
        let frame: IssuedFrameId = identifiers.mint_frame_id().expect("frame");
        let correlation: IssuedCorrelationId =
            identifiers.mint_correlation_id().expect("correlation");
        ArtifactWriteContext::new(
            ArtifactLinksDraft::default()
                .with_run_id(run)
                .with_frame_id(frame)
                .with_correlation_id(correlation),
            EventLinksDraft::default()
                .with_run_id(run)
                .with_frame_id(frame)
                .with_correlation_id(correlation),
            1_752_147_200_000 + sequence,
        )
    }
}
