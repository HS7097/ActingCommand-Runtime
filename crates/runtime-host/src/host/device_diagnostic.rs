// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    DEVICE_DIAGNOSTIC_DETAIL_LIMIT, DeviceDiagnosticBudgetRecord, DeviceDiagnosticMode,
    DeviceDiagnosticSample, DeviceDiagnosticSourceField, DiagnosticDetailRecord, RuntimePayload,
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
        links: &EventLinksDraft,
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
            self.observe_device_detail_under_fact_gate(event, links, field, detail)?;
        }
        Ok(())
    }

    fn observe_device_detail_under_fact_gate(
        &self,
        event: &PersistedEvent,
        links: &EventLinksDraft,
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
        if record.emitted_count < DEVICE_DIAGNOSTIC_DETAIL_LIMIT {
            let mut emitted = record.clone();
            emitted.emitted_count += 1;
            self.append_device_diagnostic_record_under_fact_gate(emitted, false, links.clone())?;
            record.emitted_count += 1;
        } else {
            // Reserve room for the emitted count so the total remains representable.
            record.folded_count = record
                .folded_count
                .checked_add(1)
                .filter(|count| count.checked_add(u64::from(record.emitted_count)).is_some())
                .ok_or_else(|| ledger_error("device_diagnostic_count_overflow"))?;
        }
        Ok(())
    }

    fn append_device_diagnostic_record_under_fact_gate(
        &self,
        record: DeviceDiagnosticBudgetRecord,
        summary: bool,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        // This is an additional ledger fact; it never re-enters the observation hook.
        let result = append_device_diagnostic_record(
            &self.ledger,
            &self.events,
            self.owner_epoch,
            record,
            summary,
            links,
        );
        if let Err(error) = &result {
            self.lifecycle_append_failed.store(true, Ordering::Release);
            self.fatal.mark(error.clone())?;
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
        self.append_device_diagnostic_record_under_fact_gate(
            budget.record.clone(),
            true,
            EventLinksDraft::default(),
        )?;
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
    let mut failure = original.unwrap_or_else(|| error.clone()).into_fatal();
    failure.lifecycle.incomplete_device_diagnostic_summary =
        Some((error.code(), error.operation()));
    failure
}

pub(super) fn append_device_diagnostic_record(
    ledger: &GlobalLedger,
    events: &RuntimeEvents,
    owner_epoch: actingcommand_contract::OwnerEpoch,
    record: DeviceDiagnosticBudgetRecord,
    summary: bool,
    links: EventLinksDraft,
) -> RuntimeHostResult<()> {
    events
        .draft(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::device_diagnostics(owner_epoch, record, summary),
        )
        .and_then(|draft| events.sanitize(draft))
        .and_then(|draft| {
            ledger
                .append(draft)
                .map(|_| ())
                .map_err(|_| ledger_error("append_device_diagnostic_budget"))
        })
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
            let mut preserved = failure.take().unwrap_or_else(|| error.clone()).into_fatal();
            preserved.lifecycle.incomplete_device_diagnostic_summary =
                error.lifecycle.incomplete_device_diagnostic_summary;
            *failure = Some(preserved);
        }
        result => record_failure(failure, result),
    }
}
