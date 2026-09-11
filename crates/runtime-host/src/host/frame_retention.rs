// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_artifact_store::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreResult, try_artifact_delete_guard,
};
use actingcommand_contract::{
    ArtifactId, ArtifactKind, ArtifactPayloadDraft, ArtifactPinReason, ArtifactPinRecord,
    ArtifactRetentionFact, ArtifactRetentionIdentity, AuditInput, EventActor, EventLinksDraft,
    EventSeverity, EventSource, EventType, FRAME_RETENTION_POLICY_VERSION, OriginModule,
    OwnerEpoch, RETENTION_ROUND_BYTES, RETENTION_ROUND_OBJECTS, RETENTION_ROUND_START_BUDGET_MS,
    RuntimeErrorCode, TerminalEvent,
};
use actingcommand_ledger::{
    ArtifactEvictionAdmission, GlobalLedger, GlobalLedgerError, PersistedEvent,
};
use std::time::{Duration, Instant};

use crate::{RuntimeHostError, RuntimeHostResult, events::RuntimeEvents};

/// The publication sink calls this while the original material publication guard is held.
pub(super) fn pin_published_frames(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    owner_epoch: OwnerEpoch,
    verified: &PersistedEvent,
    reason: ArtifactPinReason,
) -> ArtifactStoreResult<()> {
    if verified.event_type() != EventType::ArtifactVerified {
        return Err(pin_failure("artifact_pin_source_not_verified"));
    }
    let links = verified.links();
    for artifact in verified
        .artifacts()
        .iter()
        .filter(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
    {
        let identity = ArtifactRetentionIdentity {
            artifact: artifact.project(true),
            owner_epoch,
            instance_id: *links
                .instance_id()
                .ok_or_else(|| pin_failure("artifact_pin_instance_missing"))?,
            request_id: *links
                .request_id()
                .ok_or_else(|| pin_failure("artifact_pin_request_missing"))?,
            correlation_id: *links
                .correlation_id()
                .ok_or_else(|| pin_failure("artifact_pin_correlation_missing"))?,
            run_id: links.run_id().copied(),
            lease_id: links.lease_id().copied(),
            policy_version: FRAME_RETENTION_POLICY_VERSION,
        };
        let draft = events
            .draft(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::ArtifactStore,
                EventActor::Runtime,
                EventLinksDraft::default(),
                ArtifactPayloadDraft::retention(
                    ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
                        identity,
                        reason,
                        trigger: TerminalEvent {
                            event_id: *verified.event_id(),
                            sequence: verified.sequence(),
                        },
                    }),
                    AuditInput::new(),
                ),
            )
            .and_then(|draft| events.sanitize(draft))
            .map_err(|error| {
                ArtifactStoreError::fatal(error.code(), "pin_published_frame", error.to_string())
            })?;
        let draft = links
            .artifact_retention_source()
            .apply_to(draft)
            .map_err(|error| {
                ArtifactStoreError::fatal(
                    "artifact_pin_source_conflict",
                    "pin_published_frame",
                    error.to_string(),
                )
            })?;
        ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "pin_published_frame", error.to_string())
        })?;
    }
    Ok(())
}

fn pin_failure(code: &'static str) -> ArtifactStoreError {
    ArtifactStoreError::fatal(
        code,
        "pin_published_frame",
        "verified frame source lacks its originating identity",
    )
}

/// Only the scan cursor lives here. Eligibility and recovery state belong to GlobalLedger.
#[derive(Default)]
pub(super) struct FrameRetention {
    after: Option<ArtifactId>,
}

struct RetentionRound {
    recovering: bool,
    completed: usize,
}

impl FrameRetention {
    /// Called by the existing performance loop after its sampling locks are released.
    pub(super) fn maintain(
        &mut self,
        ledger: &GlobalLedger,
        artifacts: &ArtifactStore,
        stopping: impl Fn() -> bool,
    ) -> RuntimeHostResult<()> {
        self.round(ledger, artifacts, &stopping, false)?;
        Ok(())
    }

    /// This completes before provider assembly or normal work admission.
    pub(super) fn recover(
        ledger: &GlobalLedger,
        artifacts: &ArtifactStore,
        deadline: Instant,
    ) -> RuntimeHostResult<()> {
        let mut retention = Self::default();
        loop {
            if Instant::now() >= deadline {
                return Err(failure("artifact_eviction_recovery_deadline"));
            }
            // Each round starts from the remaining original intents. No new intent is admitted.
            retention.after = None;
            let round = retention.round(ledger, artifacts, &|| Instant::now() >= deadline, true)?;
            if !round.recovering {
                return Ok(());
            }
            if round.completed == 0 {
                return Err(failure("artifact_eviction_recovery_deferred"));
            }
        }
    }

    fn round(
        &mut self,
        ledger: &GlobalLedger,
        artifacts: &ArtifactStore,
        stopping: &impl Fn() -> bool,
        recovery: bool,
    ) -> RuntimeHostResult<RetentionRound> {
        ledger.check_writer_health().map_err(ledger_failure)?;
        let started = Instant::now();
        let candidates = ledger
            .retention_candidates(self.after)
            .map_err(ledger_failure)?;
        if candidates.references.len() > RETENTION_ROUND_OBJECTS {
            return Err(failure("artifact_retention_candidate_bound_exceeded"));
        }
        let mut round = RetentionRound {
            recovering: candidates.recovery_pending,
            completed: 0,
        };
        if round.recovering && !recovery {
            return Err(failure("artifact_eviction_startup_recovery_required"));
        }
        if !round.recovering && recovery {
            return Ok(round);
        }
        let mut removed_bytes = 0_u64;
        for reference in &candidates.references {
            if stopping()
                || started.elapsed() >= Duration::from_millis(RETENTION_ROUND_START_BUDGET_MS)
            {
                return Ok(round);
            }
            self.after = Some(reference.artifact_id);
            if reference.byte_count > RETENTION_ROUND_BYTES.saturating_sub(removed_bytes) {
                continue;
            }
            // The writer never waits for this cross-process, try-only material guard.
            let Some(guard) = try_artifact_delete_guard(artifacts.root(), reference)
                .map_err(RuntimeHostError::artifact)?
            else {
                continue;
            };
            if stopping() {
                return Ok(round);
            }
            match ledger
                .admit_artifact_eviction(guard)
                .map_err(ledger_failure)?
            {
                ArtifactEvictionAdmission::Deferred => {}
                ArtifactEvictionAdmission::Committed(permit) => {
                    // Once intent is durable, shutdown waits for this outcome or its explicit error.
                    ledger
                        .finish_artifact_eviction(permit)
                        .map_err(ledger_failure)?;
                    removed_bytes = removed_bytes
                        .checked_add(reference.byte_count)
                        .ok_or_else(|| failure("artifact_retention_byte_count_overflow"))?;
                    round.completed += 1;
                }
            }
        }
        self.after = candidates.next_after;
        Ok(round)
    }
}

fn failure(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(
        code,
        "maintain_frame_retention",
        RuntimeErrorCode::LedgerFailure,
    )
}

fn ledger_failure(error: GlobalLedgerError) -> RuntimeHostError {
    RuntimeHostError::fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    )
    .with_native_detail(error.to_string())
}
