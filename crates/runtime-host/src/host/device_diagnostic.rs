// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    DeviceDiagnosticBudgetRecord, DeviceDiagnosticMode, DeviceDiagnosticSample,
    DeviceDiagnosticSourceField, DiagnosticDetailRecord, RuntimePayload,
};

pub(super) struct DeviceDiagnosticBudget {
    record: DeviceDiagnosticBudgetRecord,
    closed: bool,
}

impl DeviceDiagnosticBudget {
    pub(super) fn new(
        owner_epoch: actingcommand_contract::OwnerEpoch,
        mode: DeviceDiagnosticMode,
    ) -> Self {
        Self {
            record: DeviceDiagnosticBudgetRecord::new(owner_epoch, mode),
            closed: false,
        }
    }
}

impl HostShared {
    pub(super) fn observe_device_diagnostics_under_fact_gate(
        &self,
        event: &PersistedEvent,
    ) -> RuntimeHostResult<()> {
        let mut details = [None, None, None];
        match event.payload() {
            EventPayload::Input(InputPayload::Failed(value))
            | EventPayload::Capture(CapturePayload::Failed(value)) => {
                details[0] = value
                    .detail()
                    .map(|detail| (DeviceDiagnosticSourceField::Detail, detail));
                details[1] = value
                    .cleanup_cause()
                    .and_then(|cause| cause.detail())
                    .map(|detail| (DeviceDiagnosticSourceField::CleanupDetail, detail));
            }
            EventPayload::Runtime(RuntimePayload::Failed(value)) => {
                if let Some(lifecycle) = value.lifecycle_failure() {
                    details[0] = lifecycle.primary_detail().map(|detail| {
                        (DeviceDiagnosticSourceField::LifecyclePrimaryDetail, detail)
                    });
                    details[1] = lifecycle
                        .cleanup_cause()
                        .and_then(|cause| cause.detail())
                        .map(|detail| {
                            (DeviceDiagnosticSourceField::LifecycleCleanupDetail, detail)
                        });
                    details[2] = lifecycle
                        .cause()
                        .and_then(|cause| cause.detail())
                        .map(|detail| (DeviceDiagnosticSourceField::LifecycleCauseDetail, detail));
                }
            }
            _ => return Ok(()),
        }
        for (field, detail) in details.into_iter().flatten() {
            self.observe_device_detail_under_fact_gate(event, field, detail)?;
        }
        Ok(())
    }

    /// Counts one detail field into the epoch slot; the close summary is the only device
    /// diagnostic fact, so this path writes nothing to the ledger (Workflow #328).
    fn observe_device_detail_under_fact_gate(
        &self,
        event: &PersistedEvent,
        field: DeviceDiagnosticSourceField,
        detail: &DiagnosticDetailRecord,
    ) -> RuntimeHostResult<()> {
        let mut budget = lock(&self.device_diagnostics, "observe_device_diagnostic_budget")?;
        if budget.closed {
            return Err(ledger_error("device_diagnostic_budget_closed"));
        }
        let sample = DeviceDiagnosticSample {
            source_event_id: *event.event_id(),
            source_sequence: event.sequence(),
            source_module: event.origin().module(),
            source_field: field,
            detail: Some(detail.clone()),
        };
        let record = &mut budget.record;
        if record.first.is_none() {
            record.first = Some(sample.clone());
        }
        record.last = Some(sample);
        record.declared_sensitivity = record
            .declared_sensitivity
            .max(detail.declared_sensitivity());
        record.emitted_count = record
            .emitted_count
            .checked_add(1)
            .ok_or_else(|| ledger_error("device_diagnostic_count_overflow"))?;
        Ok(())
    }

    fn append_device_diagnostic_record_under_fact_gate(
        &self,
        record: DeviceDiagnosticBudgetRecord,
    ) -> RuntimeHostResult<()> {
        // This is an additional ledger fact; it never re-enters the observation hook.
        // Its append is deferred; this boundary confirms it with the same wait a
        // synchronous append had, so a failure keeps the same code and error path.
        let result =
            append_device_diagnostic_record(&self.ledger, &self.events, self.owner_epoch, record)
                .and_then(|()| {
                    confirm_deferred_appends(
                        &self.ledger,
                        GlobalLedger::REPLY_TIMEOUT,
                        "append_device_diagnostic_budget",
                    )
                });
        if let Err(error) = &result {
            self.lifecycle_append_failed.store(true, Ordering::Release);
            self.fatal.mark(error.clone())?;
        }
        result
    }

    /// Confirms every deferred append at a host boundary; a failed reply sets the existing
    /// lifecycle latch and surfaces through `operation` exactly as a synchronous failure.
    pub(super) fn confirm_deferred_appends(
        &self,
        deadline: Duration,
        operation: &'static str,
    ) -> RuntimeHostResult<()> {
        let result = confirm_deferred_appends(&self.ledger, deadline, operation);
        if result.is_err() {
            self.lifecycle_append_failed.store(true, Ordering::Release);
        }
        result
    }

    fn close_device_diagnostic_budget(&self) -> RuntimeHostResult<()> {
        let _gate = lock(&self.fact_write_gate, "close_device_diagnostic_budget")?;
        let mut budget = lock(&self.device_diagnostics, "close_device_diagnostic_budget")?;
        if budget.closed {
            return Ok(());
        }
        if self.lifecycle_append_failed.load(Ordering::Acquire) {
            return Err(ledger_error("close_device_diagnostic_budget"));
        }
        self.append_device_diagnostic_record_under_fact_gate(budget.record.clone())?;
        budget.closed = true;
        Ok(())
    }

    pub(super) fn finish_device_diagnostics(&self, failure: &mut Option<RuntimeHostError>) {
        if let Err(error) = self.close_device_diagnostic_budget() {
            *failure = Some(summary_incomplete(failure.take(), &error));
        }
    }
}

pub(super) fn summary_incomplete(
    original: Option<RuntimeHostError>,
    error: &RuntimeHostError,
) -> RuntimeHostError {
    let mut failure = match original {
        Some(original) => original.with_complete_failure(
            crate::error::RuntimeFailureRelation::LifecycleRecord,
            error.clone(),
        ),
        None => error.clone(),
    }
    .into_fatal();
    failure.lifecycle.incomplete_device_diagnostic_summary =
        Some((error.code(), error.operation()));
    if let Some(complete) = &mut failure.lifecycle.complete_failure {
        complete
            .primary
            .lifecycle
            .incomplete_device_diagnostic_summary =
            failure.lifecycle.incomplete_device_diagnostic_summary;
    }
    failure
}

/// Hands the close summary to the ledger without awaiting its commit. Acceptance is not
/// persistence: the calling boundary confirms it through `confirm_deferred_appends`, and
/// nothing reads the summary's event id before that.
pub(super) fn append_device_diagnostic_record(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    owner_epoch: actingcommand_contract::OwnerEpoch,
    record: DeviceDiagnosticBudgetRecord,
) -> RuntimeHostResult<()> {
    events
        .draft(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            EventLinksDraft::default(),
            RuntimePayloadDraft::device_diagnostics(owner_epoch, record),
        )
        .and_then(|draft| events.sanitize(draft))
        .and_then(|draft| {
            ledger
                .append_deferred(draft)
                .map(|_| ())
                .map_err(|_| ledger_error("append_device_diagnostic_budget"))
        })
}

/// One confirmation boundary: a failed deferred reply fails the boundary through the same
/// ledger error a synchronous append failure produced; pending replies are left to the
/// ledger's own shutdown, which refuses to drop them silently.
pub(super) fn confirm_deferred_appends(
    ledger: &GlobalLedger,
    deadline: Duration,
    operation: &'static str,
) -> RuntimeHostResult<()> {
    let summary = ledger
        .confirm_deferred(deadline)
        .map_err(|_| ledger_error(operation))?;
    if summary.failed.is_empty() {
        Ok(())
    } else {
        Err(ledger_error(operation))
    }
}

pub(super) fn record_host_close_result(
    failure: &mut Option<RuntimeHostError>,
    result: RuntimeHostResult<()>,
) {
    match result {
        Err(error)
            if error
                .lifecycle
                .incomplete_device_diagnostic_summary
                .is_some() =>
        {
            let summary = error.lifecycle.incomplete_device_diagnostic_summary;
            record_failure(failure, Err(error));
            if let Some(preserved) = failure {
                preserved.lifecycle.incomplete_device_diagnostic_summary = summary;
                if let Some(complete) = &mut preserved.lifecycle.complete_failure {
                    complete
                        .primary
                        .lifecycle
                        .incomplete_device_diagnostic_summary = summary;
                }
            }
        }
        result => record_failure(failure, result),
    }
}
