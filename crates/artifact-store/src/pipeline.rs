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
use std::sync::Arc;

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

/// Borrowed synchronous publication under each frame's original context.
/// Hosts use a fresh single-frame Ledger sink for each invocation.
pub type FrameArtifactPublisher<'a> =
    dyn FnMut(&ArtifactStore, ArtifactWriteRequest<'_>) -> ArtifactStoreResult<StoredArtifact> + 'a;

pub struct CapturePipeline {
    frame_store: FrameStore,
    artifact_store: Arc<ArtifactStore>,
    event_ids: IdentifierIssuer,
    run_context: ArtifactWriteContext,
    contexts: BTreeMap<usize, ArtifactWriteContext>,
    persisted: BTreeMap<usize, ArtifactReference>,
    pinned: BTreeMap<usize, PinnedFrameReason>,
    missing_pinned: BTreeSet<usize>,
    counts: CapturePipelineCounts,
    retention_class: RetentionClass,
    artifact_producer: ArtifactProducer,
    redaction_state: ArtifactRedactionState,
    paused: bool,
    pressure_refusal: Option<crate::FramePersistenceFailure>,
}

impl CapturePipeline {
    /// Retains the supplied store with the standalone capture provenance and retention profile.
    pub fn open(
        artifact_store: Arc<ArtifactStore>,
        frame_temp_root: impl AsRef<Path>,
        config: CapturePipelineConfig,
        run_context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<Self> {
        Self::open_shared_inner(
            artifact_store,
            frame_temp_root,
            config,
            run_context,
            sink,
            0,
        )
    }

    /// Host capture shares its existing material owner and protects its severity backtrace.
    pub fn open_with_store(
        artifact_store: Arc<ArtifactStore>,
        frame_temp_root: impl AsRef<Path>,
        config: CapturePipelineConfig,
        run_context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<Self> {
        Self::open_shared_inner(
            artifact_store,
            frame_temp_root,
            config,
            run_context,
            sink,
            8,
        )
    }

    fn open_shared_inner(
        artifact_store: Arc<ArtifactStore>,
        frame_temp_root: impl AsRef<Path>,
        config: CapturePipelineConfig,
        run_context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
        protected_history: usize,
    ) -> ArtifactStoreResult<Self> {
        config.validate()?;
        let mut pipeline = Self {
            frame_store: FrameStore::new(
                frame_temp_root.as_ref().to_path_buf(),
                config.frame_store,
            )?,
            artifact_store,
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
            artifact_producer: if protected_history > 0 {
                ArtifactProducer::CaptureStore
            } else {
                ArtifactProducer::CapturePipeline
            },
            redaction_state: config.redaction_state,
            paused: false,
            pressure_refusal: None,
        };
        pipeline
            .frame_store
            .protect_recent_frames(protected_history)?;
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

    /// The caller retains the original throughout this synchronous material operation.
    /// Sample before cloning and keep that original charged through every publication.
    pub fn with_frame_copy<T>(
        &mut self,
        frame: &actingcommand_device::Frame,
        operation: impl FnOnce(&mut Self, actingcommand_device::Frame) -> T,
    ) -> ArtifactStoreResult<T> {
        let previous = self.frame_store.admit_frame_copy(frame)?;
        let result = operation(self, frame.clone());
        self.frame_store.release_frame_copy(previous);
        Ok(result)
    }

    pub fn record_frame(
        &mut self,
        input: FrameStoreFrameInput,
        context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<CapturePipelineOutcome> {
        self.record_frame_with_publisher(input, context, sink, None)
    }

    pub fn record_frame_with_publisher(
        &mut self,
        input: FrameStoreFrameInput,
        context: ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
        mut publisher: Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<CapturePipelineOutcome> {
        let frame_index = input.frame_index;
        if let Some(failure) = &self.pressure_refusal {
            return Err(failure.error.clone());
        }
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

        let mut pressure_persisted = Vec::new();
        let frame_result = self.frame_store.add_frame(input, &mut |candidate| {
            let artifact = Self::publish_candidate(
                &self.artifact_store,
                &self.contexts,
                ArtifactIssuePolicy::new(
                    self.artifact_producer,
                    self.retention_class,
                    self.redaction_state,
                ),
                candidate,
                sink,
                &mut publisher,
            )?;
            let reference = artifact.reference().clone();
            pressure_persisted.push((candidate.frame_index, reference.clone()));
            Ok((Arc::clone(&self.artifact_store), reference))
        });
        let mut persisted = Vec::new();
        for (index, reference) in pressure_persisted {
            if self.frame_store.persisted_reference(index) != Some(&reference) {
                // The material owner returns its original commit/identity failure below.
                continue;
            }
            persisted.push(reference.clone());
            if self.persisted.insert(index, reference).is_none() {
                self.counts.persisted = self.counts.persisted.checked_add(1).ok_or_else(|| {
                    ArtifactStoreError::fatal(
                        "capture_summary_count_overflow",
                        "persist_capture_frame",
                        "persisted count exceeds u64",
                    )
                })?;
            }
        }
        let frame = match frame_result {
            Ok(frame) => frame,
            Err(error) => {
                if !error.is_fatal() {
                    self.paused = true;
                    self.pressure_refusal = Some(crate::FramePersistenceFailure {
                        frame_index,
                        error: error.clone(),
                    });
                }
                if let Err(recording) = self.emit_frame_store_events(&context, sink) {
                    return Err(error.with_secondary(&recording));
                }
                return Err(error);
            }
        };
        self.paused = frame.pause_required;
        if let Some(failure) = frame.frame_failures.first() {
            self.pressure_refusal = Some(failure.clone());
        }
        if let Err(recording) = self.emit_frame_store_events(&context, sink) {
            return Err(match frame.frame_failures.first() {
                Some(failure) => failure.error.clone().with_secondary(&recording),
                None => recording,
            });
        }
        if self.pressure_refusal.is_none() {
            persisted.extend(self.persist_candidates(false, sink, &mut publisher)?);
        }
        Ok(CapturePipelineOutcome {
            frame,
            persisted,
            evidence_completeness: self.evidence_completeness(),
        })
    }

    /// Preserve an exact newly referenced/backtrace frame before publishing its pin.
    pub fn pin_frame(
        &mut self,
        frame_index: usize,
        reason: PinnedFrameReason,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<ArtifactReference> {
        self.pin_frame_with_publisher(frame_index, reason, sink, None)
    }

    pub fn pin_frame_with_publisher(
        &mut self,
        frame_index: usize,
        reason: PinnedFrameReason,
        sink: &mut dyn ArtifactEventSink,
        publisher: Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<ArtifactReference> {
        self.frame_store.pin_frame(frame_index, reason)?;
        self.pinned.entry(frame_index).or_insert(reason);
        let reference = self.persist_frame_with_publisher(frame_index, sink, publisher)?;
        self.frame_store.persistence_candidate(frame_index)?;
        Ok(reference)
    }

    /// Existing capture/receipt consumers require the exact frame synchronously.
    pub fn persist_frame(
        &mut self,
        frame_index: usize,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<ArtifactReference> {
        self.persist_frame_with_publisher(frame_index, sink, None)
    }

    pub fn persist_frame_with_publisher(
        &mut self,
        frame_index: usize,
        sink: &mut dyn ArtifactEventSink,
        mut publisher: Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<ArtifactReference> {
        if let Some(reference) = self.persisted.get(&frame_index) {
            return Ok(reference.clone());
        }
        if let Some(failure) = &self.pressure_refusal {
            return Err(failure.error.clone());
        }
        let candidate = self.frame_store.persistence_candidate(frame_index)?;
        let pinned = candidate.pinned_reason.is_some();
        let result = Self::publish_candidate(
            &self.artifact_store,
            &self.contexts,
            ArtifactIssuePolicy::new(
                self.artifact_producer,
                self.retention_class,
                self.redaction_state,
            ),
            &candidate,
            sink,
            &mut publisher,
        );
        drop(candidate);
        let artifact = match result {
            Ok(artifact) => artifact,
            Err(mut error) => {
                if !error.is_fatal() {
                    self.pressure_refusal = Some(crate::FramePersistenceFailure {
                        frame_index,
                        error: error.clone(),
                    });
                    self.paused = true;
                }
                if pinned {
                    self.missing_pinned.insert(frame_index);
                    if let Err(event_error) = self.record_pinned_failure(frame_index, sink) {
                        error = error.with_secondary(&event_error);
                    }
                }
                return Err(error);
            }
        };
        let reference = artifact.reference().clone();
        self.frame_store.mark_artifact_persisted(
            frame_index,
            Arc::clone(&self.artifact_store),
            reference.clone(),
        )?;
        self.counts.persisted = self.counts.persisted.checked_add(1).ok_or_else(|| {
            ArtifactStoreError::fatal(
                "capture_summary_count_overflow",
                "persist_capture_frame",
                "persisted frame count exceeds u64",
            )
        })?;
        self.missing_pinned.remove(&frame_index);
        self.persisted.insert(frame_index, reference.clone());
        Ok(reference)
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
        self.poll_pressure_with_publisher(context, sink, None)
    }

    pub fn poll_pressure_with_publisher(
        &mut self,
        context: &ArtifactWriteContext,
        sink: &mut dyn ArtifactEventSink,
        mut publisher: Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<bool> {
        let was_paused = self.paused;
        self.frame_store.refresh_pressure()?;
        self.emit_frame_store_events(context, sink)?;
        if let Some(failure) = &self.pressure_refusal {
            let error = &failure.error;
            if error.code() == "capacity_admission_refused" {
                let mut recovered = self
                    .contexts
                    .get(&failure.frame_index)
                    .cloned()
                    .ok_or_else(|| {
                        ArtifactStoreError::fatal(
                            "missing_frame_context",
                            "recover_frame_capacity",
                            "original refused frame context is missing",
                        )
                    })?;
                let bytes = error
                    .capacity()
                    .map_or(0, |decision| decision.requested_bytes);
                match self.artifact_store.admit_new_bytes(
                    &mut recovered,
                    &self.artifact_store.root().join("artifacts"),
                    bytes,
                ) {
                    Ok(()) => self.pressure_refusal = None,
                    Err(error) if !error.is_fatal() => return Ok(false),
                    Err(error) => return Err(error),
                }
            } else if error.code() == "frame_workspace_unavailable"
                && self
                    .frame_store
                    .publication_workspace_available(failure.frame_index)
            {
                self.pressure_refusal = None;
            } else {
                return Ok(false);
            }
        }
        let mut persisted = Vec::new();
        let result = self.frame_store.flush_pressure(&mut |candidate| {
            let artifact = Self::publish_candidate(
                &self.artifact_store,
                &self.contexts,
                ArtifactIssuePolicy::new(
                    self.artifact_producer,
                    self.retention_class,
                    self.redaction_state,
                ),
                candidate,
                sink,
                &mut publisher,
            )?;
            let reference = artifact.reference().clone();
            persisted.push((candidate.frame_index, reference.clone()));
            Ok((Arc::clone(&self.artifact_store), reference))
        });
        for (index, reference) in persisted {
            if self.frame_store.persisted_reference(index) != Some(&reference) {
                continue;
            }
            if self.persisted.insert(index, reference).is_none() {
                self.counts.persisted = self.counts.persisted.checked_add(1).ok_or_else(|| {
                    ArtifactStoreError::fatal(
                        "capture_summary_count_overflow",
                        "persist_capture_frame",
                        "persisted count exceeds u64",
                    )
                })?;
            }
        }
        if let Some(failure) = result?.into_iter().next() {
            self.paused = true;
            let error = failure.error.clone();
            self.pressure_refusal = Some(failure);
            return Err(error);
        }
        self.paused = self.frame_store.is_pressure_paused();
        self.emit_frame_store_events(context, sink)?;
        Ok(was_paused && !self.paused)
    }

    pub fn record_recognition(
        &mut self,
        frame_index: usize,
        state: crate::RecognitionState,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<()> {
        self.record_recognition_with_publisher(frame_index, state, sink, None)
    }

    pub fn record_recognition_with_publisher(
        &mut self,
        frame_index: usize,
        state: crate::RecognitionState,
        sink: &mut dyn ArtifactEventSink,
        publisher: Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<()> {
        self.frame_store.record_recognition(frame_index, state)?;
        let context = self.contexts.get(&frame_index).cloned().ok_or_else(|| {
            ArtifactStoreError::fatal(
                "missing_frame_context",
                "record_capture_recognition",
                "original frame context is missing",
            )
        })?;
        self.poll_pressure_with_publisher(&context, sink, publisher)?;
        Ok(())
    }

    pub fn finish(
        &mut self,
        sink: &mut dyn ArtifactEventSink,
    ) -> ArtifactStoreResult<CapturePipelineSummary> {
        self.finish_with_publisher(sink, None)
    }

    pub fn finish_with_publisher(
        &mut self,
        sink: &mut dyn ArtifactEventSink,
        mut publisher: Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<CapturePipelineSummary> {
        self.persist_candidates(true, sink, &mut publisher)?;
        self.summary()
    }

    /// Releases resident ownership only after required material publication is complete.
    pub fn cleanup_spills(&mut self) -> ArtifactStoreResult<()> {
        if !self
            .frame_store
            .persistence_candidate_indexes(true)
            .is_empty()
        {
            return Err(ArtifactStoreError::fatal(
                "frame_spill_material_pending",
                "cleanup_frame_spills",
                "original frame publication is incomplete",
            ));
        }
        self.frame_store.cleanup_temp()
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

    pub fn frame_context(&self, index: usize) -> Option<&ArtifactWriteContext> {
        self.contexts.get(&index)
    }

    pub fn failure_context(&self, error: &ArtifactStoreError) -> Option<&ArtifactWriteContext> {
        self.pressure_refusal
            .as_ref()
            .filter(|failure| failure.error == *error)
            .and_then(|failure| self.contexts.get(&failure.frame_index))
            .or_else(|| {
                self.frame_store
                    .failure_frame_index(error)
                    .and_then(|index| self.contexts.get(&index))
            })
    }

    fn persist_candidates(
        &mut self,
        include_all_retained: bool,
        sink: &mut dyn ArtifactEventSink,
        publisher: &mut Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<Vec<ArtifactReference>> {
        let candidates = self
            .frame_store
            .persistence_candidate_indexes(include_all_retained);
        let mut stored = Vec::new();
        for frame_index in candidates {
            let reference = match publisher {
                Some(publisher) => {
                    self.persist_frame_with_publisher(frame_index, sink, Some(&mut **publisher))?
                }
                None => self.persist_frame(frame_index, sink)?,
            };
            stored.push(reference);
        }
        Ok(stored)
    }

    fn publish_candidate(
        artifact_store: &ArtifactStore,
        contexts: &BTreeMap<usize, ArtifactWriteContext>,
        policy: ArtifactIssuePolicy,
        candidate: &FramePersistenceCandidate<'_>,
        sink: &mut dyn ArtifactEventSink,
        publisher: &mut Option<&mut FrameArtifactPublisher<'_>>,
    ) -> ArtifactStoreResult<StoredArtifact> {
        let context = contexts.get(&candidate.frame_index).ok_or_else(|| {
            ArtifactStoreError::fatal(
                "missing_frame_context",
                "persist_capture_frame",
                format!("frame index {} has no typed context", candidate.frame_index),
            )
        })?;
        let request = ArtifactWriteRequest::new(
            ArtifactKind::CaptureFrame,
            &candidate.png,
            context.clone(),
            policy,
        );
        match publisher {
            Some(publish) => publish(artifact_store, request),
            None => artifact_store.put(request, sink),
        }
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
                    preserved_frame_index,
                    duplicate_count,
                    duration_ms,
                } => {
                    if preserved_frame_index.is_none() {
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
                    }
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
                    let payload = match preserved_frame_index {
                        Some(frame_index) => {
                            let frame_context =
                                self.contexts.get(&frame_index).ok_or_else(|| {
                                    ArtifactStoreError::fatal(
                                        "missing_frame_context",
                                        "emit_capture_similarity",
                                        "preserved original frame has no typed identity",
                                    )
                                })?;
                            CapturePayloadDraft::dedup_window_preserving_material(
                                frame_context.event_links(),
                                duration_ms,
                                AuditInput::new(),
                            )
                        }
                        None => CapturePayloadDraft::dedup_window(
                            duplicate_count,
                            duration_ms,
                            AuditInput::new(),
                        ),
                    };
                    (representative.event_links().clone(), payload)
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
        capacity_root: Option<std::path::PathBuf>,
        capacity_fact: std::sync::OnceLock<actingcommand_contract::CapacityFactReference>,
        event_types: Vec<EventType>,
        fail_artifact_created: bool,
    }

    impl crate::ArtifactCapacityAdmission for RecordingSink {
        fn decide(
            &self,
            path: &std::path::Path,
            bytes: u64,
        ) -> ArtifactStoreResult<actingcommand_contract::CapacityDecision> {
            use actingcommand_contract::{CapacityAdmissionOutcome, CapacityAdmissionReason};
            let root = self.capacity_root.as_deref().ok_or_else(|| {
                ArtifactStoreError::fatal(
                    "fixture_capacity_unconfigured",
                    "fixture_capacity_admission",
                    "the existing fixture must name its admitted target root",
                )
            })?;
            let root = root.to_str().expect("fixture root is UTF-8");
            let path = path.to_str().expect("fixture target is UTF-8");
            let root = std::path::Path::new(root.strip_prefix(r"\\?\").unwrap_or(root));
            let path = std::path::Path::new(path.strip_prefix(r"\\?\").unwrap_or(path));
            let fact = self.capacity_fact.get_or_init(|| {
                let ids =
                    actingcommand_contract::IdentifierIssuer::new().expect("fixture identity");
                actingcommand_contract::CapacityFactReference {
                    event_id: *ids.mint_event_id().expect("capacity event").transport(),
                    sequence: 1,
                    owner_epoch: *ids.mint_owner_epoch().expect("capacity owner").transport(),
                    observed_at_unix_ms: 1,
                    observed_at_monotonic_ms: 0,
                }
            });
            let (outcome, reason) = if !path.starts_with(root)
                || path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                (
                    CapacityAdmissionOutcome::Unknown,
                    CapacityAdmissionReason::BindingChanged,
                )
            } else if bytes > 64 * 1024 * 1024 {
                (
                    CapacityAdmissionOutcome::HardPressure,
                    CapacityAdmissionReason::HardThreshold,
                )
            } else {
                (
                    CapacityAdmissionOutcome::Allowed,
                    CapacityAdmissionReason::FreshSample,
                )
            };
            Ok(actingcommand_contract::CapacityDecision {
                owner_epoch: fact.owner_epoch,
                decided_at_unix_ms: 1,
                decided_at_monotonic_ms: 0,
                requested_bytes: bytes,
                target_volume: Some("fixture-volume".to_owned()),
                outcome,
                reason,
                fact: Some(fact.clone()),
            })
        }
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let pipeline = CapturePipeline::open(
            artifact_store,
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let mut pipeline = CapturePipeline::open(
            artifact_store,
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let mut pipeline = CapturePipeline::open(
            artifact_store,
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let mut pipeline = CapturePipeline::open(
            artifact_store,
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let mut pipeline = CapturePipeline::open(
            artifact_store,
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let mut pipeline = CapturePipeline::open(
            artifact_store,
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
        let artifact_store =
            Arc::new(ArtifactStore::open(temp.path().join("artifacts")).expect("store"));
        artifact_store
            .install_capacity_admission(Arc::new(RecordingSink {
                capacity_root: Some(artifact_store.root().to_path_buf()),
                ..RecordingSink::default()
            }))
            .expect("fixture capacity owner");
        let mut pipeline = CapturePipeline::open(
            artifact_store,
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
        let hard = max_mem_bytes.max(64 * 1024 + 32 * 1024);
        let ratio = max_mem_bytes as f64 / hard as f64;
        frame_store.tier1_ratio *= ratio;
        frame_store.tier2_ratio *= ratio;
        frame_store.tier3_ratio *= ratio;
        frame_store.max_mem_bytes = Some(hard);
        frame_store.os_reserve_bytes = 0;
        frame_store.flush_workspace_reserve_bytes = 1;
        let frame_store =
            frame_store.with_memory_source(crate::MemorySampleSource::fixed(crate::MemorySample {
                total_bytes: hard,
                available_bytes: hard,
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
