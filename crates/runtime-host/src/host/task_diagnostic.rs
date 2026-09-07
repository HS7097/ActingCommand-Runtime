// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    MAX_TASK_DIAGNOSTIC_RECORD_BYTES, TASK_DIAGNOSTIC_SCHEMA, TaskDiagnosticHeader,
    TaskDiagnosticKind as Kind, TaskDiagnosticRecord,
};
use actingcommand_page_detector::{PageBatchResult, PageOutcome, PageTargetEvaluation};
use actingcommand_recognition_pack::{
    OcrObservationEvaluation, RecognitionPackResult, TargetEvaluation,
};
use serde_json::{Value, json};
use std::io::Write;

pub(super) struct DiagnosticStep {
    index: u32,
    action_id: ActionId,
    started_ms: u64,
}

struct RecordWriter {
    bytes: Vec<u8>,
}

impl Write for RecordWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_TASK_DIAGNOSTIC_RECORD_BYTES - self.bytes.len() {
            return Err(std::io::Error::other(
                "task diagnostic record size exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn failure(error: impl std::fmt::Display) -> RequestFailure {
    RequestFailure::poison_without_terminal(
        artifact_store_error("task_diagnostic_failed").with_native_detail(error.to_string()),
    )
}

impl RuntimeContainedTask<'_> {
    pub(super) fn begin_diagnostic(&mut self) -> Result<(), RequestFailure> {
        let header = TaskDiagnosticHeader {
            schema_version: TASK_DIAGNOSTIC_SCHEMA.to_owned(),
            request_id: self.control.request_id,
            correlation_id: self.request.correlation_id(),
            task_id: *self.task_id.transport(),
            run_id: *self.run_id.transport(),
            instance_id: self.token.instance_id(),
            lease_id: self.token.lease_id(),
        };
        let bytes = serde_json::to_vec(&header).map_err(failure)?;
        self.diagnostic_stream = Some(
            self.host
                .artifacts
                .begin_stream(
                    ArtifactKind::DiagnosticJson,
                    ArtifactWriteContext::new(
                        self.request.task_artifact_links(self.run_id),
                        self.links(),
                        unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
                    ),
                    ArtifactIssuePolicy::new(
                        ArtifactProducer::ArtifactStore,
                        RetentionClass::DebugFull,
                        ArtifactRedactionState::Pending,
                    ),
                )
                .map_err(failure)?,
        );
        let stream = self.diagnostic_stream.as_mut().expect("created stream");
        stream
            .append(&bytes[..bytes.len() - 1])
            .and_then(|()| stream.append(b",\"records\":[\n"))
            .map_err(failure)
    }

    fn diagnostic_record(
        &self,
        kind: Kind,
        parent_index: Option<u64>,
        data: Value,
    ) -> TaskDiagnosticRecord {
        TaskDiagnosticRecord {
            index: 0,
            kind,
            parent_index,
            data,
            frame_id: self.last_frame_id.map(|id| *id.transport()),
            step_action_id: self.diagnostic_step.as_ref().map(|step| step.action_id),
            physical_action_id: self.diagnostic_physical,
        }
    }

    fn write_diagnostic_record(
        &mut self,
        mut record: TaskDiagnosticRecord,
    ) -> Result<u64, RequestFailure> {
        let index = self
            .diagnostic_records
            .checked_add(1)
            .ok_or_else(|| failure("record count overflow"))?;
        record.index = index;
        let stream = self
            .diagnostic_stream
            .as_mut()
            .ok_or_else(|| failure("task diagnostic stream missing"))?;
        if self.diagnostic_records != 0 {
            stream.append(b",").map_err(failure)?;
        }
        let mut writer = RecordWriter { bytes: Vec::new() };
        serde_json::to_writer(&mut writer, &record).map_err(failure)?;
        stream.append(&writer.bytes).map_err(failure)?;
        stream.append(b"\n").map_err(failure)?;
        self.diagnostic_records = index;
        Ok(index)
    }

    fn diagnostic(
        &mut self,
        kind: Kind,
        parent: Option<u64>,
        data: Value,
    ) -> Result<u64, RequestFailure> {
        self.write_diagnostic_record(self.diagnostic_record(kind, parent, data))
    }

    pub(super) fn begin_diagnostic_step(
        &mut self,
        index: u32,
        action_id: ActionId,
        now: u64,
    ) -> Result<(), RequestFailure> {
        self.end_diagnostic_step(now, false)?;
        self.diagnostic_physical = None;
        self.diagnostic_step = Some(DiagnosticStep {
            index,
            action_id,
            started_ms: now,
        });
        self.diagnostic(
            Kind::StepStarted,
            None,
            json!({"step_index": index, "monotonic_ms": now}),
        )?;
        Ok(())
    }

    pub(super) fn end_diagnostic_step(
        &mut self,
        now: u64,
        completed: bool,
    ) -> Result<(), RequestFailure> {
        if let Some(step) = &self.diagnostic_step {
            let elapsed = now
                .checked_sub(step.started_ms)
                .ok_or_else(|| failure("step monotonic clock regressed"))?;
            self.diagnostic(
                Kind::StepElapsed,
                None,
                json!({
                    "step_index": step.index, "started_monotonic_ms": step.started_ms,
                    "ended_monotonic_ms": now, "elapsed_ms": elapsed, "completed": completed,
                }),
            )?;
            self.diagnostic_step = None;
        }
        Ok(())
    }

    pub(super) fn diagnostic_pages(
        &mut self,
        phase: &'static str,
        results: &PageBatchResult,
    ) -> Result<(), RequestFailure> {
        let outcomes = match results {
            Ok(outcomes) => outcomes,
            Err(error) => &error.completed,
        };
        for outcome in outcomes {
            self.diagnostic_page(phase, outcome)?;
        }
        if let Err(error) = results {
            self.diagnostic(
                Kind::Error,
                None,
                json!({"phase": phase, "message": error.cause.message()}),
            )?;
            for page in &error.unexecuted {
                self.diagnostic(
                    Kind::Unexecuted,
                    None,
                    json!({"phase": phase, "page": page}),
                )?;
            }
        }
        Ok(())
    }

    fn diagnostic_page(
        &mut self,
        phase: &'static str,
        outcome: &PageOutcome,
    ) -> Result<(), RequestFailure> {
        let parent = match &outcome.result {
            Ok(page) => self.diagnostic(Kind::Page, None, json!({
                "phase": phase, "index": outcome.index, "page_id": page.page_id,
                "matched": page.matched, "message": page.message,
                "required_passed": page.required_passed, "required_total": page.required_total,
                "any_of_passed": page.any_of_passed, "any_of_total": page.any_of_total,
                "optional_passed": page.optional_passed, "optional_total": page.optional_total,
                "forbidden_passed": page.forbidden_passed, "forbidden_total": page.forbidden_total,
            }))?,
            Err(error) => self.diagnostic(Kind::Page, None, json!({
                "phase": phase, "index": outcome.index, "page_id": outcome.page_id,
                "error": error.message(), "failed_target": error.failed_target,
            }))?,
        };
        let targets = match &outcome.result {
            Ok(page) => &page.target_results,
            Err(error) => &error.completed_targets,
        };
        for target in targets {
            self.diagnostic_page_target(parent, target)?;
        }
        Ok(())
    }

    fn diagnostic_page_target(
        &mut self,
        parent: u64,
        target: &PageTargetEvaluation,
    ) -> Result<(), RequestFailure> {
        self.diagnostic_target(Some(parent), &target.evaluation, json!({
            "role": target.role, "group_index": target.group_index, "target_index": target.target_index,
            "target_id": target.target_id, "passed": target.passed, "message": target.message,
        }))
    }

    pub(super) fn diagnostic_guard(
        &mut self,
        target: Option<&str>,
        result: Option<&RecognitionPackResult<TargetEvaluation>>,
        reason: &'static str,
    ) -> Result<(), RequestFailure> {
        match result {
            Some(Ok(value)) => self.diagnostic_target(None, value, json!({"phase": "guard"})),
            Some(Err(error)) => self
                .diagnostic(
                    Kind::Error,
                    None,
                    json!({"phase": "guard", "target_id": target, "error": error}),
                )
                .map(|_| ()),
            None => self
                .diagnostic(
                    Kind::Unexecuted,
                    None,
                    json!({"phase": "guard", "target_id": target, "reason": reason}),
                )
                .map(|_| ()),
        }
    }

    fn diagnostic_target(
        &mut self,
        parent: Option<u64>,
        target: &TargetEvaluation,
        source: Value,
    ) -> Result<(), RequestFailure> {
        let index = self.diagnostic(Kind::Target, parent, json!({
            "id": target.id, "kind": target.kind, "passed": target.passed, "message": target.message,
            "template": target.template, "color": target.color, "source": source,
        }))?;
        if let Some(ocr) = &target.ocr {
            let ocr_index = self.diagnostic(
                Kind::Ocr,
                Some(index),
                json!({
                    "region": ocr.region, "raw_text": ocr.raw_text, "derived_text": ocr.text,
                    "confidence": ocr.confidence, "matched_expected": ocr.matched_expected,
                    "match_mode": ocr.match_mode, "block_count": ocr.blocks.len(),
                }),
            )?;
            self.diagnostic_ocr_blocks(ocr_index, &ocr.blocks, &ocr.block_source_order)?;
        }
        if let Some(nn) = &target.nn {
            let nn_index = self.diagnostic(Kind::Nn, Some(index), json!({
                "requested_region": nn.requested_region, "selected_label": nn.selected_label,
                "selected_score": nn.selected_score, "selection": nn.selection, "label_count": nn.labels.len(),
            }))?;
            let mut labels = nn.labels.iter().enumerate().collect::<Vec<_>>();
            labels.sort_by_key(|(_, label)| label.source_index);
            for (rank, label) in labels {
                self.diagnostic(Kind::NnLabel, Some(nn_index), json!({
                    "source_index": label.source_index, "raw": {"label": label.label, "score": label.score},
                    "derived": {"candidate": label.candidate, "rank": rank},
                }))?;
            }
        }
        Ok(())
    }

    fn diagnostic_ocr_blocks(
        &mut self,
        parent: u64,
        blocks: &[actingcommand_recognition_pack::OcrTextEvidence],
        order: &[usize],
    ) -> Result<(), RequestFailure> {
        if blocks.len() != order.len() {
            return Err(failure("OCR source order mismatch"));
        }
        let mut indices = order.iter().copied().enumerate().collect::<Vec<_>>();
        indices.sort_by_key(|(_, source)| *source);
        for (rank, source) in indices {
            self.diagnostic(
                Kind::OcrBlock,
                Some(parent),
                json!({
                    "source_index": source, "derived_rank": rank, "raw": blocks[rank],
                }),
            )?;
        }
        Ok(())
    }

    pub(super) fn diagnostic_ocr(
        &mut self,
        target: &str,
        result: &RecognitionPackResult<OcrObservationEvaluation>,
    ) -> Result<(), RequestFailure> {
        match result {
            Ok(ocr) => {
                let index = self.diagnostic(Kind::Ocr, None, json!({
                    "phase": "post_admission", "target_id": target, "region": ocr.region,
                    "raw_text": ocr.raw_text, "derived_text": ocr.text, "confidence": ocr.confidence,
                    "block_count": ocr.blocks.len(), "execution": ocr.execution,
                }))?;
                self.diagnostic_ocr_blocks(index, &ocr.blocks, &ocr.block_source_order)
            }
            Err(error) => self
                .diagnostic(
                    Kind::Error,
                    None,
                    json!({"phase": "post_admission", "target_id": target, "error": error}),
                )
                .map(|_| ()),
        }
    }

    pub(super) fn finish_diagnostic(
        &mut self,
        result: &Result<ContainedTaskOutcome, ContainedTaskRunError<RequestFailure>>,
    ) -> Result<(), RequestFailure> {
        self.end_diagnostic_step(
            self.host
                .monotonic_ms()
                .map_err(RequestFailure::poison_without_terminal)?,
            false,
        )?;
        // Read a fixed ledger cut. Each existing verified artifact remains its original source.
        let through = self.host.ledger.latest_sequence().map_err(failure)?;
        let query = EventQuery {
            event_type: Some(EventType::ArtifactVerified),
            request_id: Some(self.control.request_id),
            task_id: Some(*self.task_id.transport()),
            run_id: Some(*self.run_id.transport()),
            ..EventQuery::default()
        };
        let mut after = 0;
        loop {
            let events = self
                .host
                .ledger
                .query_page(query.clone(), after, through, 256)
                .map_err(failure)?;
            if events.is_empty() {
                break;
            }
            for event in &events {
                for artifact in event.artifacts() {
                    self.write_diagnostic_record(TaskDiagnosticRecord {
                        index: 0, kind: Kind::Artifact, parent_index: None,
                        frame_id: event.links().frame_id().copied(), step_action_id: None,
                        physical_action_id: event.links().action_id().copied(),
                        data: json!({"source_sequence": event.sequence(), "artifact": artifact.project(true)}),
                    })?;
                }
            }
            after = events.last().expect("nonempty page").sequence();
        }
        let data = match result {
            Ok(outcome) => json!({"execution": "returned", "outcome": outcome.outcome,
                "executed_steps": outcome.executed_steps, "final_page": outcome.final_page}),
            Err(ContainedTaskRunError::Task(error)) => {
                json!({"execution": "task_error", "code": error.code(), "detail": error.detail(), "executed_steps": null})
            }
            Err(
                ContainedTaskRunError::Boundary(error)
                | ContainedTaskRunError::NonfatalOperation(error),
            ) => {
                json!({"execution": "operation_error", "code": error.error.code(), "executed_steps": null})
            }
        };
        self.diagnostic(Kind::Terminal, None, data)?;
        self.diagnostic_stream
            .as_mut()
            .ok_or_else(|| failure("stream missing at seal"))?
            .append(b"]}\n")
            .map_err(failure)?;
        let stream = self.diagnostic_stream.take().expect("stream ready to seal");
        let mut sink = RuntimeArtifactEventSink {
            ledger: &self.host.ledger,
            events: &self.host.events,
        };
        self.host
            .artifacts
            .seal_stream(stream, &mut sink)
            .map_err(failure)?;
        Ok(())
    }

    pub(super) fn abort_diagnostic(&mut self) -> Result<(), RequestFailure> {
        self.diagnostic_stream
            .take()
            .map_or(Ok(()), |stream| stream.abort().map_err(failure))
    }
}
