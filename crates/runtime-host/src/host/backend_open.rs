// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::BackendObservationStatus;
use actingcommand_device::BackendOpenObservation;

impl HostShared {
    pub(super) fn append_backend_open_failure_observations(
        &self,
        error: &ExecutionKernelError,
        links: EventLinksDraft,
        source: EventSource,
        module: OriginModule,
    ) -> RuntimeHostResult<()> {
        self.append_backend_open_observations(
            error.failure_context().backend_open_observations(),
            links,
            source,
            module,
        )
        .map_err(|writer| {
            writer.with_complete_failure(
                crate::error::RuntimeFailureRelation::DiagnosticArchive,
                RuntimeHostError::execution("backend_open_source", error),
            )
        })
    }

    pub(super) fn append_backend_open_observations(
        &self,
        observations: &[BackendOpenObservation],
        links: EventLinksDraft,
        source: EventSource,
        module: OriginModule,
    ) -> RuntimeHostResult<()> {
        if observations.is_empty() {
            return Ok(());
        }
        let gate = lock(&self.fact_write_gate, "append_backend_open_observations")?;
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error("append_backend_open_observations"));
        }
        let mut persisted = Vec::new();
        let mut recorded = Vec::new();
        for observation in observations {
            let receipt = observation.occurrence.recorded_event::<EventId>();
            if receipt.get().is_some() {
                continue;
            }
            let event = self
                .append_event_under_fact_gate(
                    if observation.report.status == BackendObservationStatus::Failed
                        || observation.report.capture_check == BackendObservationStatus::Failed
                        || observation.report.input_check == BackendObservationStatus::Failed
                    {
                        EventSeverity::Error
                    } else if !observation.report.warnings.is_empty()
                        || observation
                            .report
                            .attempts
                            .iter()
                            .any(|attempt| attempt.status == BackendObservationStatus::Failed)
                    {
                        EventSeverity::Warning
                    } else {
                        EventSeverity::Info
                    },
                    source,
                    module,
                    EventActor::Runtime,
                    links.clone(),
                    RuntimePayloadDraft::backend_open_observed(
                        self.owner_epoch,
                        observation.report.clone(),
                    ),
                )
                .inspect_err(|_| {
                    self.lifecycle_append_failed.store(true, Ordering::Release);
                })?;
            let _ = receipt.set(*event.event_id());
            persisted.push(event);
            recorded.push(&observation.report);
        }
        if !persisted.is_empty() {
            self.synchronize_fact_store_under_gate().inspect_err(|_| {
                self.lifecycle_append_failed.store(true, Ordering::Release);
            })?;
        }
        drop(gate);
        for event in persisted {
            self.observe_pipeline_event(&event)?;
        }
        if recorded.is_empty() {
            return Ok(());
        }
        // Workflow #317 sc1: every recorded open becomes its instance's self-check facts.
        let instance_id = *links.instance_id().ok_or_else(|| {
            RuntimeHostError::fatal(
                "backend_selfcheck_instance_missing",
                "record_backend_selfcheck_facts",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?;
        for report in recorded {
            self.record_backend_selfcheck_facts(instance_id, report)?;
        }
        Ok(())
    }
}
