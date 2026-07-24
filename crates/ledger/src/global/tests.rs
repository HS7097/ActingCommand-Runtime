// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::PersistedEvent;
use crate::fact::StoredEventRecord;
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, DiagnosticCode, EffectDisposition,
    EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventQuery, EventSeverity, EventSource, EventType, IdentifierIssuer, InputAction,
    IssuedActionId, IssuedCausationId, IssuedCorrelationId, IssuedEventId, IssuedFrameId,
    IssuedInstanceId, IssuedLeaseId, IssuedRecognitionId, IssuedRequestId, IssuedRunId,
    IssuedTaskId, LeasePayloadDraft, OriginModule, PerformanceContext, PolicyActivitySample,
    PolicyAdmissionRecord, PolicyBudgetReceipt, PolicyDispatchEventData, PolicyExecutionEventData,
    PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition, PolicyFailureRecord,
    PolicyPayloadDraft, PolicyReasonRecord, ProjectionPayload, ProjectionProfile, SecretField,
    SecretFingerprinter, SubscriptionCursor, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn config(temp: &TempDir, owner_id: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(temp.path(), owner_id)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn identifiers() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn event_id() -> IssuedEventId {
    identifiers().mint_event_id().expect("event id")
}

fn instance_id() -> IssuedInstanceId {
    identifiers().mint_instance_id().expect("instance id")
}

fn request_id() -> IssuedRequestId {
    identifiers().mint_request_id().expect("request id")
}

fn correlation_id() -> IssuedCorrelationId {
    identifiers().mint_correlation_id().expect("correlation id")
}

fn causation_id() -> IssuedCausationId {
    identifiers().mint_causation_id().expect("causation id")
}

fn task_id() -> IssuedTaskId {
    identifiers().mint_task_id().expect("task id")
}

fn run_id() -> IssuedRunId {
    identifiers().mint_run_id().expect("run id")
}

fn lease_id() -> IssuedLeaseId {
    identifiers().mint_lease_id().expect("lease id")
}

fn frame_id() -> IssuedFrameId {
    identifiers().mint_frame_id().expect("frame id")
}

fn action_id() -> IssuedActionId {
    identifiers().mint_action_id().expect("action id")
}

fn recognition_id() -> IssuedRecognitionId {
    identifiers().mint_recognition_id().expect("recognition id")
}

#[test]
fn open_never_returns_with_a_detached_future_owner() {
    let temp = TempDir::new().expect("temp");
    let started = Instant::now();

    let ledger = GlobalLedger::open_with_store(config(&temp, "writer-delayed"), |config| {
        thread::sleep(Duration::from_millis(100));
        SegmentStore::open(config)
    })
    .expect("delayed store open must finish before return");

    assert!(started.elapsed() >= Duration::from_millis(90));
    ledger.close().expect("close delayed writer");
    let replacement = GlobalLedger::open(config(&temp, "writer-after-delay"))
        .expect("replacement writer must open immediately");
    replacement.close().expect("close replacement writer");
}

#[test]
fn store_open_failure_joins_waiting_writer() {
    let temp = TempDir::new().expect("temp");
    let error = GlobalLedger::open_with_store(config(&temp, "writer-failed"), |_| {
        Err(GlobalLedgerError::fatal(
            "test_store_open_failed",
            "open_test_store",
        ))
    })
    .expect_err("store open must fail");

    assert_eq!(error.code(), "test_store_open_failed");
    GlobalLedger::open(config(&temp, "writer-after-failure"))
        .expect("joined waiting writer must not conflict")
        .close()
        .expect("close replacement writer");
}

#[test]
fn immediate_retry_after_failed_open_has_no_writer_conflict() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("ledger-root");
    fs::write(&root, b"not a directory").expect("blocking root file");
    let failed = GlobalLedgerConfig::new(&root, "writer-failed");

    let error = GlobalLedger::open(failed).expect_err("file root must fail store open");
    assert_eq!(error.code(), "ledger_io");

    fs::remove_file(&root).expect("remove blocking root file");
    GlobalLedger::open(GlobalLedgerConfig::new(&root, "writer-retry"))
        .expect("retry must not conflict with a detached writer")
        .close()
        .expect("close retry writer");
}

#[test]
fn empty_ledger_reopens_without_treating_the_segment_as_a_blank_record() {
    let temp = TempDir::new().expect("temp");
    GlobalLedger::open(config(&temp, "first-owner"))
        .expect("first owner")
        .close()
        .expect("close first owner");

    GlobalLedger::open(config(&temp, "second-owner"))
        .expect("reopen empty ledger")
        .close()
        .expect("close second owner");
}

fn event(event_label: &str) -> actingcommand_contract::SanitizedEventDraft {
    event_with_links(event_label, EventLinksDraft::default(), AuditInput::new())
}

fn event_with_links(
    event_label: &str,
    links: EventLinksDraft,
    audit: AuditInput,
) -> actingcommand_contract::SanitizedEventDraft {
    event_with_id_and_links(event_label, event_id(), links, audit)
}

fn event_with_id_and_links(
    _event_label: &str,
    event_id: IssuedEventId,
    links: EventLinksDraft,
    audit: AuditInput,
) -> actingcommand_contract::SanitizedEventDraft {
    let payload = CommandPayloadDraft::received(EventAction::RuntimeStart, audit);
    EventDraft::new(
        event_id,
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        links,
        payload.into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"test-private-salt").expect("fingerprinter"))
    .expect("sanitize")
}

#[derive(Clone)]
struct ScheduledRecoveryDrafts {
    intent: actingcommand_contract::SanitizedEventDraft,
    lease_granted: actingcommand_contract::SanitizedEventDraft,
    admission: actingcommand_contract::SanitizedEventDraft,
    execution: actingcommand_contract::SanitizedEventDraft,
    completion: actingcommand_contract::SanitizedEventDraft,
    task_request: actingcommand_contract::SanitizedEventDraft,
    task_completed: actingcommand_contract::SanitizedEventDraft,
    task_failed_warning: actingcommand_contract::SanitizedEventDraft,
    task_failed_fatal: actingcommand_contract::SanitizedEventDraft,
    task_failed_legacy: actingcommand_contract::SanitizedEventDraft,
    task_failed_invalid: actingcommand_contract::SanitizedEventDraft,
    task_effect: actingcommand_contract::SanitizedEventDraft,
    task_release: actingcommand_contract::SanitizedEventDraft,
    duplicate_task_release: actingcommand_contract::SanitizedEventDraft,
    wrong_chain_task_release: actingcommand_contract::SanitizedEventDraft,
    interrupted_release: actingcommand_contract::SanitizedEventDraft,
    duplicate_interrupted_release: actingcommand_contract::SanitizedEventDraft,
    recovered_success_execution: actingcommand_contract::SanitizedEventDraft,
    recovered_warning_execution: actingcommand_contract::SanitizedEventDraft,
    recovered_fatal_execution: actingcommand_contract::SanitizedEventDraft,
    recovered_interrupted_execution: actingcommand_contract::SanitizedEventDraft,
}

fn scheduled_recovery_drafts() -> ScheduledRecoveryDrafts {
    let issuer = identifiers();
    let instance_id = issuer.mint_instance_id().expect("instance id");
    let request_id = issuer.mint_request_id().expect("request id");
    let task_request_id = issuer.mint_request_id().expect("task request id");
    let correlation_id = issuer.mint_correlation_id().expect("correlation id");
    let task_id = issuer.mint_task_id().expect("task id");
    let run_id = issuer.mint_run_id().expect("run id");
    let lease_id = issuer.mint_lease_id().expect("lease id");
    let dispatch_links = EventLinksDraft::default()
        .with_instance_id(instance_id)
        .with_request_id(request_id)
        .with_correlation_id(correlation_id)
        .with_task_id(task_id)
        .with_run_id(run_id)
        .with_action_id(issuer.mint_action_id().expect("dispatch action"));
    let lease_links = dispatch_links
        .clone()
        .with_lease_id(lease_id)
        .with_action_id(issuer.mint_action_id().expect("lease action"));
    let execution_links = lease_links
        .clone()
        .with_action_id(issuer.mint_action_id().expect("execution action"));
    let task_request_links = EventLinksDraft::default()
        .with_instance_id(instance_id)
        .with_request_id(task_request_id)
        .with_correlation_id(correlation_id)
        .with_task_id(task_id)
        .with_run_id(run_id);
    let task_terminal_links = task_request_links.clone().with_lease_id(lease_id);
    let task_release_links = task_terminal_links
        .clone()
        .with_action_id(issuer.mint_action_id().expect("task release action"));
    let interrupted_release_links = lease_links
        .clone()
        .with_action_id(issuer.mint_action_id().expect("interrupted release action"));
    let wrong_chain_task_release_links = EventLinksDraft::default()
        .with_instance_id(instance_id)
        .with_request_id(issuer.mint_request_id().expect("wrong request id"))
        .with_correlation_id(correlation_id)
        .with_task_id(task_id)
        .with_run_id(run_id)
        .with_lease_id(lease_id)
        .with_action_id(issuer.mint_action_id().expect("wrong-chain release action"));
    let dispatch = PolicyDispatchEventData {
        decision_id: "decision:recovery-fixture".to_owned(),
        task_id: "task:recovery-fixture".to_owned(),
        instance_id: "instance:recovery-fixture".to_owned(),
        operation_id: "operation:recovery-fixture".to_owned(),
        package_digest: format!("sha256:{}", "1".repeat(64)),
        procedure_binding_digest: format!("sha256:{}", "2".repeat(64)),
        reason_chain_id: "reason:recovery-fixture".to_owned(),
        reasons: vec![PolicyReasonRecord {
            code: "scheduled".to_owned(),
            detail: "deterministic recovery fixture".to_owned(),
        }],
        catalog_hash: format!("sha256:{}", "3".repeat(64)),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:recovery-fixture".to_owned(),
        approval_fact_ids: vec![],
        urgency_milli: 100,
    };
    let admission = PolicyAdmissionRecord {
        activity: PolicyActivitySample {
            profile_id: "profile:recovery-fixture".to_owned(),
            local_day: 1,
            window_id: "window:recovery-fixture".to_owned(),
            admitted_at_unix_ms: 1_752_147_200_100,
            seed: 7,
            interval_ms: 1_000,
            next_eligible_unix_ms: 1_752_147_201_100,
        },
        budget: PolicyBudgetReceipt {
            task_daily_used: 1,
            task_daily_limit: 1,
            task_window_used: 1,
            task_window_limit: 1,
            task_runtime_reserved_ms: 1,
            task_runtime_limit_ms: 1,
            activity_daily_used: 1,
            activity_daily_limit: 1,
            activity_window_used: 1,
            activity_window_limit: 1,
            activity_runtime_reserved_ms: 1,
            activity_runtime_limit_ms: 1,
        },
    };
    let execution = PolicyExecutionEventData {
        decision_id: dispatch.decision_id.clone(),
        task_id: dispatch.task_id.clone(),
        instance_id: dispatch.instance_id.clone(),
        observed_at_unix_ms: 1_752_147_200_400,
        outcome: PolicyExecutionOutcome::Succeeded { runtime_ms: 300 },
    };
    let policy_failure = |class, runtime_ms| PolicyFailureRecord {
        error_code: if class == PolicyFailureClass::Recoverable {
            "capture_transient"
        } else {
            "capture_fatal"
        }
        .to_owned(),
        reported_success: false,
        original_class: class,
        effective_class: class,
        consecutive_same_error: 1,
        escalation_streak: 1,
        performance_tax_exempt: false,
        retry_attempt: 0,
        disposition: if class == PolicyFailureClass::Severe {
            PolicyFailureDisposition::PausedTask
        } else {
            PolicyFailureDisposition::Continue
        },
        retry_at_unix_ms: None,
        runtime_ms,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(1_752_147_200_400)),
    };
    let recovered_execution = |outcome| PolicyExecutionEventData {
        decision_id: dispatch.decision_id.clone(),
        task_id: dispatch.task_id.clone(),
        instance_id: dispatch.instance_id.clone(),
        observed_at_unix_ms: 1_752_147_200_400,
        outcome,
    };
    let interrupted_failure = PolicyFailureRecord {
        error_code: "policy_settlement_interrupted".to_owned(),
        reported_success: false,
        original_class: PolicyFailureClass::Severe,
        effective_class: PolicyFailureClass::Severe,
        consecutive_same_error: 1,
        escalation_streak: 1,
        performance_tax_exempt: false,
        retry_attempt: 0,
        disposition: PolicyFailureDisposition::PausedTask,
        retry_at_unix_ms: None,
        runtime_ms: 0,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(1_752_147_200_350)),
    };
    let sanitize = |timestamp_unix_ms, severity, origin, links, payload: EventPayloadDraft| {
        EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            timestamp_unix_ms,
            severity,
            origin,
            links,
            payload,
        )
        .sanitize(&Sha256SecretFingerprinter::new(b"scheduled-recovery-test").expect("salt"))
        .expect("sanitize scheduled recovery fixture")
    };
    ScheduledRecoveryDrafts {
        intent: sanitize(
            1_752_147_200_100,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            dispatch_links.clone(),
            PolicyPayloadDraft::dispatch_intent(dispatch.clone(), AuditInput::new()).into(),
        ),
        lease_granted: sanitize(
            1_752_147_200_200,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            lease_links,
            LeasePayloadDraft::granted(
                EventAction::LeaseAcquire,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
        admission: sanitize(
            1_752_147_200_300,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            dispatch_links,
            PolicyPayloadDraft::dispatch_admitted(
                dispatch.clone(),
                admission.clone(),
                AuditInput::new(),
            )
            .into(),
        ),
        execution: sanitize(
            1_752_147_200_400,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            execution_links,
            PolicyPayloadDraft::execution_recorded(execution, AuditInput::new()).into(),
        ),
        completion: sanitize(
            1_752_147_200_500,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            EventLinksDraft::default(),
            PolicyPayloadDraft::dispatch_completed(
                dispatch.clone(),
                admission.clone(),
                AuditInput::new(),
            )
            .into(),
        ),
        task_request: sanitize(
            1_752_147_200_350,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            task_request_links,
            ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        ),
        task_completed: sanitize(
            1_752_147_200_400,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            task_terminal_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Success,
                    final_page: Some("home".to_owned()),
                    executed_steps: 0,
                    failure_code: None,
                },
                AuditInput::new(),
            )
            .into(),
        ),
        task_failed_warning: sanitize(
            1_752_147_200_400,
            EventSeverity::Warning,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            task_terminal_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: 0,
                    failure_code: Some("capture_transient".to_owned()),
                },
                AuditInput::new(),
            )
            .into(),
        ),
        task_failed_fatal: sanitize(
            1_752_147_200_400,
            EventSeverity::Fatal,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            task_terminal_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: 0,
                    failure_code: Some("capture_fatal".to_owned()),
                },
                AuditInput::new(),
            )
            .into(),
        ),
        task_failed_legacy: sanitize(
            1_752_147_200_400,
            EventSeverity::Error,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            task_terminal_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    outcome: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: 0,
                    failure_code: Some("capture_legacy".to_owned()),
                },
                AuditInput::new(),
            )
            .into(),
        ),
        task_failed_invalid: sanitize(
            1_752_147_200_400,
            EventSeverity::Warning,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            task_terminal_links.clone(),
            TaskPayloadDraft::failed(
                EventAction::RuntimeTaskRun,
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                AuditInput::new(),
            )
            .into(),
        ),
        task_effect: sanitize(
            1_752_147_200_325,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            task_terminal_links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EffectIntent {
                    step_index: 0,
                    operation_label: "fixture".to_owned(),
                    action: InputAction::Reset,
                },
                AuditInput::new(),
            )
            .into(),
        ),
        task_release: sanitize(
            1_752_147_200_450,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            task_release_links.clone(),
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
        duplicate_task_release: sanitize(
            1_752_147_200_451,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            task_release_links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
        wrong_chain_task_release: sanitize(
            1_752_147_200_450,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            wrong_chain_task_release_links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
        interrupted_release: sanitize(
            1_752_147_200_350,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            interrupted_release_links.clone(),
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
        duplicate_interrupted_release: sanitize(
            1_752_147_200_351,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Scheduler,
                EventActor::Scheduler,
            ),
            interrupted_release_links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        ),
        recovered_success_execution: sanitize(
            1_752_147_200_400,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            EventLinksDraft::default(),
            PolicyPayloadDraft::execution_recorded(
                recovered_execution(PolicyExecutionOutcome::Succeeded { runtime_ms: 50 }),
                AuditInput::new(),
            )
            .into(),
        ),
        recovered_warning_execution: sanitize(
            1_752_147_200_400,
            EventSeverity::Warning,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            EventLinksDraft::default(),
            PolicyPayloadDraft::execution_recorded(
                recovered_execution(PolicyExecutionOutcome::Failed {
                    failure: policy_failure(PolicyFailureClass::Recoverable, 50),
                }),
                AuditInput::new(),
            )
            .into(),
        ),
        recovered_fatal_execution: sanitize(
            1_752_147_200_400,
            EventSeverity::Error,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            EventLinksDraft::default(),
            PolicyPayloadDraft::execution_recorded(
                recovered_execution(PolicyExecutionOutcome::Failed {
                    failure: policy_failure(PolicyFailureClass::Severe, 50),
                }),
                AuditInput::new(),
            )
            .into(),
        ),
        recovered_interrupted_execution: sanitize(
            1_752_147_200_350,
            EventSeverity::Error,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            EventLinksDraft::default(),
            PolicyPayloadDraft::execution_recorded(
                PolicyExecutionEventData {
                    decision_id: dispatch.decision_id,
                    task_id: dispatch.task_id,
                    instance_id: dispatch.instance_id,
                    observed_at_unix_ms: 1_752_147_200_350,
                    outcome: PolicyExecutionOutcome::Failed {
                        failure: interrupted_failure,
                    },
                },
                AuditInput::new(),
            )
            .into(),
        ),
    }
}

fn append_scheduled_recovery_prefix(
    ledger: &GlobalLedger,
    drafts: &ScheduledRecoveryDrafts,
) -> (PersistedEvent, PersistedEvent) {
    ledger.append(drafts.intent.clone()).expect("intent");
    ledger
        .append(drafts.lease_granted.clone())
        .expect("lease grant");
    let admission = ledger.append(drafts.admission.clone()).expect("admission");
    let execution = ledger.append(drafts.execution.clone()).expect("execution");
    (admission, execution)
}

fn append_scheduled_execution_recovery_prefix(
    ledger: &GlobalLedger,
    drafts: &ScheduledRecoveryDrafts,
) {
    ledger.append(drafts.intent.clone()).expect("intent");
    ledger
        .append(drafts.lease_granted.clone())
        .expect("lease grant");
    ledger.append(drafts.admission.clone()).expect("admission");
}

fn assert_recovered_execution_chain(recovered: &PersistedEvent, drafts: &ScheduledRecoveryDrafts) {
    assert_eq!(recovered.event_type(), EventType::PolicyExecutionRecorded);
    assert_eq!(
        recovered.links().instance_id(),
        drafts.lease_granted.links().instance_id()
    );
    assert_eq!(
        recovered.links().request_id(),
        drafts.lease_granted.links().request_id()
    );
    assert_eq!(
        recovered.links().correlation_id(),
        drafts.lease_granted.links().correlation_id()
    );
    assert_eq!(
        recovered.links().task_id(),
        drafts.lease_granted.links().task_id()
    );
    assert_eq!(
        recovered.links().run_id(),
        drafts.lease_granted.links().run_id()
    );
    assert_eq!(
        recovered.links().lease_id(),
        drafts.lease_granted.links().lease_id()
    );
    assert!(recovered.links().action_id().is_some());
}

fn expect_recovered_execution_error(
    ledger: GlobalLedger,
    draft: actingcommand_contract::SanitizedEventDraft,
    expected_code: &str,
) {
    let error = ledger
        .append(draft)
        .expect_err("invalid recovery evidence must fail");
    assert_eq!(error.code(), expected_code);
    assert!(error.is_fatal());
    ledger
        .close()
        .expect_err("fatal recovery rejection terminates the ledger");
}

#[test]
fn recovered_policy_completion_preserves_the_persisted_scheduled_chain() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "recovery-writer")).expect("ledger");
    let drafts = scheduled_recovery_drafts();
    let (_, execution) = append_scheduled_recovery_prefix(&ledger, &drafts);

    let completion = ledger
        .append(drafts.completion)
        .expect("recover completion");

    assert_eq!(completion.event_type(), EventType::PolicyDispatchCompleted);
    assert_eq!(
        completion.links().instance_id(),
        execution.links().instance_id()
    );
    assert_eq!(
        completion.links().request_id(),
        execution.links().request_id()
    );
    assert_eq!(
        completion.links().correlation_id(),
        execution.links().correlation_id()
    );
    assert_eq!(completion.links().task_id(), execution.links().task_id());
    assert_eq!(completion.links().run_id(), execution.links().run_id());
    assert_eq!(completion.links().lease_id(), execution.links().lease_id());
    assert!(completion.links().action_id().is_some());
    assert_ne!(
        completion.links().action_id(),
        execution.links().action_id(),
        "recovery must mint a fresh action"
    );
    ledger.close().expect("close ledger");
}

#[test]
fn recovered_policy_execution_accepts_only_the_four_structured_outcome_lanes() {
    let success_root = TempDir::new().expect("success temp");
    let success =
        GlobalLedger::open(config(&success_root, "success-recovery-writer")).expect("ledger");
    let success_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&success, &success_drafts);
    success
        .append(success_drafts.task_request.clone())
        .expect("task request");
    let completed = success
        .append(success_drafts.task_completed.clone())
        .expect("task completed");
    success
        .append(success_drafts.task_release.clone())
        .expect("task release");
    let recovered = success
        .append(success_drafts.recovered_success_execution.clone())
        .expect("recover successful execution");
    assert_recovered_execution_chain(&recovered, &success_drafts);
    assert_eq!(recovered.timestamp_unix_ms(), completed.timestamp_unix_ms());
    success.close().expect("close success ledger");

    let recoverable_root = TempDir::new().expect("recoverable temp");
    let recoverable = GlobalLedger::open(config(&recoverable_root, "recoverable-recovery-writer"))
        .expect("ledger");
    let recoverable_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&recoverable, &recoverable_drafts);
    recoverable
        .append(recoverable_drafts.task_request.clone())
        .expect("task request");
    recoverable
        .append(recoverable_drafts.task_failed_warning.clone())
        .expect("recoverable task failure");
    recoverable
        .append(recoverable_drafts.task_release.clone())
        .expect("task release");
    let recovered = recoverable
        .append(recoverable_drafts.recovered_warning_execution.clone())
        .expect("recover recoverable execution");
    assert_recovered_execution_chain(&recovered, &recoverable_drafts);
    assert_eq!(recovered.severity(), EventSeverity::Warning);
    recoverable.close().expect("close recoverable ledger");

    let severe_root = TempDir::new().expect("severe temp");
    let severe =
        GlobalLedger::open(config(&severe_root, "severe-recovery-writer")).expect("ledger");
    let severe_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&severe, &severe_drafts);
    severe
        .append(severe_drafts.task_request.clone())
        .expect("task request");
    severe
        .append(severe_drafts.task_failed_fatal.clone())
        .expect("severe task failure");
    severe
        .append(severe_drafts.task_release.clone())
        .expect("task release");
    let recovered = severe
        .append(severe_drafts.recovered_fatal_execution.clone())
        .expect("recover severe execution");
    assert_recovered_execution_chain(&recovered, &severe_drafts);
    assert_eq!(recovered.severity(), EventSeverity::Error);
    severe.close().expect("close severe ledger");

    let interrupted_root = TempDir::new().expect("interrupted temp");
    let interrupted = GlobalLedger::open(config(&interrupted_root, "interrupted-recovery-writer"))
        .expect("ledger");
    let interrupted_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&interrupted, &interrupted_drafts);
    let release = interrupted
        .append(interrupted_drafts.interrupted_release.clone())
        .expect("failure release");
    let recovered = interrupted
        .append(interrupted_drafts.recovered_interrupted_execution.clone())
        .expect("recover interrupted settlement");
    assert_recovered_execution_chain(&recovered, &interrupted_drafts);
    assert_eq!(recovered.timestamp_unix_ms(), release.timestamp_unix_ms());
    interrupted.close().expect("close interrupted ledger");
}

#[test]
fn recovered_policy_execution_rejects_ambiguous_task_failure_evidence() {
    let legacy_root = TempDir::new().expect("legacy temp");
    let legacy =
        GlobalLedger::open(config(&legacy_root, "legacy-recovery-writer")).expect("ledger");
    let legacy_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&legacy, &legacy_drafts);
    legacy
        .append(legacy_drafts.task_request.clone())
        .expect("task request");
    legacy
        .append(legacy_drafts.task_failed_legacy.clone())
        .expect("legacy task failure");
    legacy
        .append(legacy_drafts.task_release.clone())
        .expect("task release");
    expect_recovered_execution_error(
        legacy,
        legacy_drafts.recovered_warning_execution,
        "scheduled_execution_recovery_failure_severity_ambiguous",
    );

    let invalid_root = TempDir::new().expect("invalid temp");
    let invalid =
        GlobalLedger::open(config(&invalid_root, "invalid-recovery-writer")).expect("ledger");
    let invalid_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&invalid, &invalid_drafts);
    invalid
        .append(invalid_drafts.task_request.clone())
        .expect("task request");
    invalid
        .append(invalid_drafts.task_failed_invalid.clone())
        .expect("invalid task failure");
    invalid
        .append(invalid_drafts.task_release.clone())
        .expect("task release");
    expect_recovered_execution_error(
        invalid,
        invalid_drafts.recovered_warning_execution,
        "scheduled_execution_recovery_failure_invalid",
    );

    let duplicate_root = TempDir::new().expect("duplicate terminal temp");
    let duplicate = GlobalLedger::open(config(
        &duplicate_root,
        "duplicate-terminal-recovery-writer",
    ))
    .expect("ledger");
    let duplicate_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&duplicate, &duplicate_drafts);
    duplicate
        .append(duplicate_drafts.task_request.clone())
        .expect("task request");
    duplicate
        .append(duplicate_drafts.task_failed_warning.clone())
        .expect("warning failure");
    duplicate
        .append(duplicate_drafts.task_failed_fatal.clone())
        .expect("conflicting fatal failure");
    duplicate
        .append(duplicate_drafts.task_release.clone())
        .expect("task release");
    expect_recovered_execution_error(
        duplicate,
        duplicate_drafts.recovered_warning_execution,
        "scheduled_execution_recovery_terminal_not_unique",
    );
}

#[test]
fn recovered_policy_execution_requires_one_matching_release() {
    let missing_root = TempDir::new().expect("missing release temp");
    let missing =
        GlobalLedger::open(config(&missing_root, "missing-release-writer")).expect("ledger");
    let missing_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&missing, &missing_drafts);
    missing
        .append(missing_drafts.task_request.clone())
        .expect("task request");
    missing
        .append(missing_drafts.task_failed_warning.clone())
        .expect("task failure");
    expect_recovered_execution_error(
        missing,
        missing_drafts.recovered_warning_execution,
        "scheduled_execution_recovery_release_not_unique",
    );

    let duplicate_root = TempDir::new().expect("duplicate release temp");
    let duplicate =
        GlobalLedger::open(config(&duplicate_root, "duplicate-release-recovery-writer"))
            .expect("ledger");
    let duplicate_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&duplicate, &duplicate_drafts);
    duplicate
        .append(duplicate_drafts.task_request.clone())
        .expect("task request");
    duplicate
        .append(duplicate_drafts.task_failed_warning.clone())
        .expect("task failure");
    duplicate
        .append(duplicate_drafts.task_release.clone())
        .expect("task release");
    duplicate
        .append(duplicate_drafts.duplicate_task_release.clone())
        .expect("duplicate task release");
    expect_recovered_execution_error(
        duplicate,
        duplicate_drafts.recovered_warning_execution,
        "scheduled_execution_recovery_release_not_unique",
    );

    let wrong_chain_root = TempDir::new().expect("wrong-chain release temp");
    let wrong_chain = GlobalLedger::open(config(
        &wrong_chain_root,
        "wrong-chain-release-recovery-writer",
    ))
    .expect("ledger");
    let wrong_chain_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&wrong_chain, &wrong_chain_drafts);
    wrong_chain
        .append(wrong_chain_drafts.task_request.clone())
        .expect("task request");
    wrong_chain
        .append(wrong_chain_drafts.task_failed_warning.clone())
        .expect("task failure");
    wrong_chain
        .append(wrong_chain_drafts.wrong_chain_task_release.clone())
        .expect("wrong-chain task release");
    expect_recovered_execution_error(
        wrong_chain,
        wrong_chain_drafts.recovered_warning_execution,
        "scheduled_execution_recovery_failure_conflict",
    );
}

#[test]
fn interrupted_policy_execution_recovery_rejects_effects_and_duplicate_release() {
    let effect_root = TempDir::new().expect("effect temp");
    let effect =
        GlobalLedger::open(config(&effect_root, "effect-recovery-writer")).expect("ledger");
    let effect_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&effect, &effect_drafts);
    effect
        .append(effect_drafts.task_effect.clone())
        .expect("task effect");
    effect
        .append(effect_drafts.interrupted_release.clone())
        .expect("failure release");
    expect_recovered_execution_error(
        effect,
        effect_drafts.recovered_interrupted_execution,
        "scheduled_execution_recovery_interruption_conflict",
    );

    let duplicate_root = TempDir::new().expect("duplicate interrupted release temp");
    let duplicate = GlobalLedger::open(config(
        &duplicate_root,
        "duplicate-interrupted-release-writer",
    ))
    .expect("ledger");
    let duplicate_drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&duplicate, &duplicate_drafts);
    duplicate
        .append(duplicate_drafts.interrupted_release.clone())
        .expect("failure release");
    duplicate
        .append(duplicate_drafts.duplicate_interrupted_release.clone())
        .expect("duplicate failure release");
    expect_recovered_execution_error(
        duplicate,
        duplicate_drafts.recovered_interrupted_execution,
        "scheduled_execution_recovery_release_not_unique",
    );
}

#[test]
fn recovered_policy_completion_requires_a_complete_local_chain() {
    let first_root = TempDir::new().expect("first temp");
    let second_root = TempDir::new().expect("second temp");
    let first = GlobalLedger::open(config(&first_root, "first-writer")).expect("first ledger");
    let second = GlobalLedger::open(config(&second_root, "second-writer")).expect("second ledger");
    let drafts = scheduled_recovery_drafts();
    append_scheduled_recovery_prefix(&first, &drafts);

    let error = second
        .append(drafts.completion)
        .expect_err("a recovery draft cannot import another ledger's facts");

    assert_eq!(error.code(), "scheduled_recovery_intent_not_unique");
    assert!(error.is_fatal());
    first.close().expect("close first ledger");
    second
        .close()
        .expect_err("fatal recovery rejection terminates the second ledger");
}

#[test]
fn recovered_policy_completion_rejects_missing_execution_and_lease_facts() {
    let missing_execution_root = TempDir::new().expect("missing execution temp");
    let missing_execution =
        GlobalLedger::open(config(&missing_execution_root, "missing-execution-writer"))
            .expect("ledger");
    let drafts = scheduled_recovery_drafts();
    append_scheduled_execution_recovery_prefix(&missing_execution, &drafts);
    let error = missing_execution
        .append(drafts.completion.clone())
        .expect_err("missing execution must fail");
    assert_eq!(error.code(), "scheduled_recovery_execution_not_unique");
    assert!(error.is_fatal());
    missing_execution
        .close()
        .expect_err("fatal missing-execution rejection terminates the ledger");

    let missing_root = TempDir::new().expect("missing temp");
    let missing = GlobalLedger::open(config(&missing_root, "missing-writer")).expect("ledger");
    let drafts = scheduled_recovery_drafts();
    missing.append(drafts.intent).expect("intent");
    missing.append(drafts.admission).expect("admission");
    missing.append(drafts.execution).expect("execution");
    let error = missing
        .append(drafts.completion)
        .expect_err("missing lease fact must fail");
    assert_eq!(error.code(), "scheduled_recovery_lease_not_unique");
    assert!(error.is_fatal());
    missing
        .close()
        .expect_err("fatal missing-evidence rejection terminates the ledger");
}

#[test]
fn recovered_policy_completion_rejects_duplicate_execution_evidence() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "duplicate-writer")).expect("ledger");
    let drafts = scheduled_recovery_drafts();
    append_scheduled_recovery_prefix(&ledger, &drafts);
    let duplicate = scheduled_recovery_drafts();
    ledger
        .append(duplicate.execution)
        .expect("append conflicting execution");

    let error = ledger
        .append(drafts.completion)
        .expect_err("duplicate execution must fail");

    assert_eq!(error.code(), "scheduled_recovery_execution_not_unique");
    assert!(error.is_fatal());
    ledger
        .close()
        .expect_err("fatal duplicate rejection terminates the ledger");
}

#[test]
fn query_filters_by_sequence_and_all_typed_correlation_ids() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-before")).expect("append before");
    let links = EventLinksDraft::default()
        .with_instance_id(instance_id())
        .with_request_id(request_id())
        .with_correlation_id(correlation_id())
        .with_causation_id(causation_id())
        .with_task_id(task_id())
        .with_run_id(run_id())
        .with_lease_id(lease_id())
        .with_frame_id(frame_id())
        .with_action_id(action_id())
        .with_recognition_id(recognition_id());
    let correlated = ledger
        .append(event_with_links(
            "evt-correlated",
            links.clone(),
            AuditInput::new(),
        ))
        .expect("append correlated");
    ledger.append(event("evt-after")).expect("append after");

    let filters = [
        EventQuery {
            instance_id: links.instance_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            request_id: links.request_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            correlation_id: links.correlation_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            causation_id: links.causation_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            task_id: links.task_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            run_id: links.run_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            lease_id: links.lease_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            frame_id: links.frame_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            action_id: links.action_id().copied(),
            ..EventQuery::default()
        },
        EventQuery {
            recognition_id: links.recognition_id().copied(),
            ..EventQuery::default()
        },
    ];
    for filter in filters {
        assert_eq!(
            ledger.query(filter).expect("query"),
            vec![correlated.clone()]
        );
    }
    assert_eq!(
        ledger
            .query(EventQuery {
                from_sequence: Some(correlated.sequence()),
                to_sequence: Some(correlated.sequence()),
                ..EventQuery::default()
            })
            .expect("sequence query"),
        vec![correlated]
    );
}

#[test]
fn query_pages_are_bounded_ordered_and_pinned_to_the_requested_snapshot() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-paged-query")).expect("ledger");
    let mut expected = Vec::new();
    for index in 0..9 {
        expected.push(
            ledger
                .append(event(&format!("evt-page-{index}")))
                .expect("append paged event"),
        );
    }
    let snapshot = expected[6].sequence();

    let first = ledger
        .query_page(EventQuery::default(), 0, snapshot, 3)
        .expect("first page");
    let second = ledger
        .query_page(
            EventQuery::default(),
            first.last().expect("first tail").sequence(),
            snapshot,
            3,
        )
        .expect("second page");
    let third = ledger
        .query_page(
            EventQuery::default(),
            second.last().expect("second tail").sequence(),
            snapshot,
            3,
        )
        .expect("third page");
    let collected = first
        .into_iter()
        .chain(second)
        .chain(third)
        .collect::<Vec<_>>();
    assert_eq!(collected, expected[..7]);
    assert!(
        ledger
            .query_page(EventQuery::default(), snapshot, snapshot, 3)
            .expect("terminal page")
            .is_empty()
    );
    assert_eq!(
        ledger
            .query_page(EventQuery::default(), 0, snapshot, 0)
            .expect_err("zero page size")
            .code(),
        "invalid_query_page"
    );
}

#[test]
fn indexed_event_pages_visit_history_once_and_retain_only_each_page() {
    const EVENT_COUNT: u64 = 4_096;
    const PAGE_EVENTS: usize = 64;

    let events = (1..=EVENT_COUNT)
        .map(|sequence| {
            PersistedEvent::from_sanitized(sequence, event("evt-linear-page"))
                .expect("persisted event")
        })
        .collect::<Vec<_>>();
    let indexes = projection::EventIndexes::from_events(&events);
    let query = EventQuery {
        event_type: Some(EventType::CommandReceived),
        ..EventQuery::default()
    };
    let mut after_sequence = 0;
    let mut total_visited = 0;
    let mut max_retained = 0;
    let mut page_count = 0;

    while after_sequence < EVENT_COUNT {
        let (page, visited) = indexes.query_page_with_visit_count(
            &events,
            &query,
            after_sequence,
            EVENT_COUNT,
            PAGE_EVENTS,
        );
        assert!(!page.is_empty());
        total_visited += visited;
        max_retained = max_retained.max(page.len());
        page_count += 1;
        after_sequence = page.last().expect("page tail").sequence();
    }

    assert_eq!(page_count, EVENT_COUNT as usize / PAGE_EVENTS);
    assert_eq!(total_visited, EVENT_COUNT as usize);
    assert_eq!(max_retained, PAGE_EVENTS);
    let (missing, visited) = indexes.query_page_with_visit_count(
        &events,
        &EventQuery {
            event_type: Some(EventType::FactPublished),
            ..EventQuery::default()
        },
        0,
        EVENT_COUNT,
        PAGE_EVENTS,
    );
    assert!(missing.is_empty());
    assert_eq!(visited, 0);
}

#[test]
fn subscription_replays_after_cursor_then_receives_live_events() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let first = ledger.append(event("evt-one")).expect("append one");
    let replay = ledger.append(event("evt-two")).expect("append two");

    let mut subscription = ledger
        .subscribe(SubscriptionCursor {
            after_sequence: first.sequence(),
        })
        .expect("subscribe");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("replay event"),
        replay
    );

    let live = ledger.append(event("evt-three")).expect("append live");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("live event"),
        live
    );
}

mod subscription {
    use super::*;

    #[test]
    fn subscription_registration_does_not_clone_unbounded_history() {
        let temp = TempDir::new().expect("temp");
        let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
        for index in 0..32 {
            ledger
                .append(event(&format!("evt-registration-{index}")))
                .expect("append history");
        }

        let subscription = ledger
            .subscribe_with_options(
                SubscriptionCursor::default(),
                SubscriptionOptions::new(2).expect("subscription options"),
            )
            .expect("subscribe");

        assert!(subscription.replay.is_empty());
        assert_eq!(subscription.replay_through_sequence(), 32);
    }

    #[test]
    fn replay_pages_are_bounded_and_writer_remains_responsive() {
        let temp = TempDir::new().expect("temp");
        let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
        for index in 0..8 {
            ledger
                .append(event(&format!("evt-page-{index}")))
                .expect("append history");
        }
        let mut subscription = ledger
            .subscribe_with_options(
                SubscriptionCursor::default(),
                SubscriptionOptions::new(2).expect("subscription options"),
            )
            .expect("subscribe");

        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(1))
                .expect("first replay")
                .sequence(),
            1
        );
        assert!(subscription.replay.len() <= 1);

        let live = ledger
            .append(event("evt-writer-responsive"))
            .expect("writer remains responsive");
        let mut sequences = vec![1];
        while sequences.len() < 9 {
            sequences.push(
                subscription
                    .recv_timeout(Duration::from_secs(1))
                    .expect("remaining replay or live")
                    .sequence(),
            );
            assert!(subscription.replay.len() <= 1);
        }
        assert_eq!(sequences, (1..=live.sequence()).collect::<Vec<_>>());
    }

    #[test]
    fn paged_replay_then_live_is_gap_free_and_ordered() {
        let temp = TempDir::new().expect("temp");
        let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
        for index in 0..5 {
            ledger
                .append(event(&format!("evt-ordered-{index}")))
                .expect("append history");
        }
        let mut subscription = ledger
            .subscribe_with_options(
                SubscriptionCursor { after_sequence: 1 },
                SubscriptionOptions::new(2).expect("subscription options"),
            )
            .expect("subscribe");
        let live = ledger
            .append(event("evt-ordered-live"))
            .expect("append live");

        let mut sequences = Vec::new();
        while sequences.len() < 5 {
            sequences.push(
                subscription
                    .recv_timeout(Duration::from_secs(1))
                    .expect("ordered event")
                    .sequence(),
            );
        }

        assert_eq!(sequences, vec![2, 3, 4, 5, live.sequence()]);
    }

    #[test]
    fn subscription_lag_is_absorbing_and_discards_buffered_events() {
        let temp = TempDir::new().expect("temp");
        let ledger = GlobalLedger::open(config(&temp, "writer-one").with_subscription_capacity(1))
            .expect("ledger");
        ledger.append(event("evt-replay")).expect("append replay");
        let mut subscription = ledger
            .subscribe_with_options(
                SubscriptionCursor::default(),
                SubscriptionOptions::new(1).expect("subscription options"),
            )
            .expect("subscribe");

        ledger.append(event("evt-live-one")).expect("first live");
        ledger.append(event("evt-live-two")).expect("second live");

        let first = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect_err("lag must preempt buffered events");
        let second = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect_err("lag must remain terminal");
        assert_eq!(first.code(), "subscription_lagged");
        assert_eq!(first, second);
        assert!(subscription.replay.is_empty());
    }

    #[test]
    fn terminal_writer_failure_preempts_replay_and_is_stable() {
        let temp = TempDir::new().expect("temp");
        let mut ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
        for index in 0..3 {
            ledger
                .append(event(&format!("evt-terminal-{index}")))
                .expect("append history");
        }
        let mut subscription = ledger
            .subscribe_with_options(
                SubscriptionCursor::default(),
                SubscriptionOptions::new(2).expect("subscription options"),
            )
            .expect("subscribe");
        let terminal = GlobalLedgerError::fatal("test_terminal", "test_writer_failure");
        ledger
            .sender
            .as_ref()
            .expect("writer sender")
            .send(WriterCommand::TestTerminalFailure {
                error: terminal.clone(),
            })
            .expect("inject terminal failure");

        let first = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect_err("terminal error must preempt replay");
        let second = subscription
            .recv_timeout(Duration::from_secs(1))
            .expect_err("terminal error must be stable");
        assert_eq!(first, terminal);
        assert_eq!(second, terminal);
        assert!(subscription.replay.is_empty());

        ledger.sender.take();
        let writer = ledger.writer.take().expect("writer handle");
        assert_eq!(
            writer
                .join()
                .expect("writer must not panic")
                .expect_err("writer must return terminal error"),
            terminal
        );
    }

    #[test]
    fn resume_cursor_recovers_every_missing_event() {
        let temp = TempDir::new().expect("temp");
        let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
        for index in 0..6 {
            ledger
                .append(event(&format!("evt-resume-{index}")))
                .expect("append history");
        }
        let options = SubscriptionOptions::new(4).expect("subscription options");
        let mut first = ledger
            .subscribe_with_options(SubscriptionCursor::default(), options)
            .expect("first subscription");
        assert_eq!(
            first
                .recv_timeout(Duration::from_secs(1))
                .expect("first event")
                .sequence(),
            1
        );
        assert_eq!(
            first
                .recv_timeout(Duration::from_secs(1))
                .expect("second event")
                .sequence(),
            2
        );
        let resume = first.resume_cursor();
        drop(first);

        let mut resumed = ledger
            .subscribe_with_options(resume, options)
            .expect("resumed subscription");
        let mut sequences = Vec::new();
        while sequences.len() < 4 {
            sequences.push(
                resumed
                    .recv_timeout(Duration::from_secs(1))
                    .expect("resumed event")
                    .sequence(),
            );
        }
        assert_eq!(sequences, vec![3, 4, 5, 6]);
    }

    #[test]
    fn future_cursor_remains_gap_free() {
        let temp = TempDir::new().expect("temp");
        let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
        let mut subscription = ledger
            .subscribe_with_options(
                SubscriptionCursor { after_sequence: 3 },
                SubscriptionOptions::new(1).expect("subscription options"),
            )
            .expect("subscribe");

        for index in 1..=3 {
            ledger
                .append(event(&format!("evt-future-{index}")))
                .expect("append suppressed event");
        }
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_millis(50))
                .expect_err("events through the future cursor stay suppressed")
                .code(),
            "subscription_timeout"
        );
        let visible = ledger
            .append(event("evt-future-visible"))
            .expect("append visible event");
        assert_eq!(
            subscription
                .recv_timeout(Duration::from_secs(1))
                .expect("visible event"),
            visible
        );
        assert_eq!(subscription.resume_cursor().after_sequence, 4);
    }

    #[test]
    fn invalid_replay_page_size_is_rejected() {
        for page_size in [0, 1025] {
            let error = SubscriptionOptions::new(page_size).expect_err("invalid page size");
            assert_eq!(error.code(), "invalid_replay_page_size");
            assert!(error.is_fatal());
        }
        assert!(SubscriptionOptions::new(1).is_ok());
        assert!(SubscriptionOptions::new(1024).is_ok());
    }

    #[test]
    fn local_terminal_state_detaches_writer_registration() {
        let (sender, _commands) = mpsc::sync_channel(1);
        let (_live, live) = mpsc::sync_channel(1);
        let (_terminal, terminal) = mpsc::sync_channel(1);
        let liveness = Arc::new(());
        let weak = Arc::downgrade(&liveness);
        let mut subscription = LedgerSubscription {
            sender,
            replay: VecDeque::new(),
            replay_fetch_after_sequence: 0,
            last_delivered_sequence: 0,
            replay_through_sequence: 0,
            replay_page_events: 1,
            live,
            terminal,
            terminal_error: None,
            liveness: Some(liveness),
        };

        let terminal =
            GlobalLedgerError::fatal("subscription_replay_invalid", "validate_replay_page");
        assert_eq!(subscription.latch_terminal(terminal.clone()), terminal);
        assert!(weak.upgrade().is_none());
        assert_eq!(subscription.terminal_error, Some(terminal));
    }
}

#[test]
fn subscription_reports_timeout_and_clean_close_separately() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");

    let timeout = subscription
        .recv_timeout(Duration::from_millis(50))
        .expect_err("empty subscription must time out");
    assert_eq!(timeout.code(), "subscription_timeout");
    assert!(!timeout.is_fatal());

    ledger
        .append(event("evt-buffered-before-close"))
        .expect("append buffered event");
    ledger.close().expect("close ledger");
    let closed = subscription
        .recv_timeout(Duration::from_millis(50))
        .expect_err("closed subscription must report closure");
    assert_eq!(closed.code(), "subscription_closed");
    assert!(!closed.is_fatal());
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_millis(50))
            .expect_err("clean closure must remain latched"),
        closed
    );
}

#[test]
fn dropped_subscription_does_not_block_remaining_live_subscribers() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one").with_subscription_capacity(1))
        .expect("ledger");
    drop(
        ledger
            .subscribe(SubscriptionCursor::default())
            .expect("dropped subscription"),
    );
    let mut active = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("active subscription");

    let event = ledger.append(event("evt-active")).expect("append event");
    assert_eq!(
        active
            .recv_timeout(Duration::from_secs(1))
            .expect("active subscriber event"),
        event
    );
}

#[test]
fn dropped_subscription_response_does_not_register_a_live_sender() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let (response, dropped_receiver) = mpsc::sync_channel(1);
    drop(dropped_receiver);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::Subscribe {
            cursor: SubscriptionCursor::default(),
            response,
        })
        .expect("enqueue dropped subscription response");
    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("enqueue subscriber count");

    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count"),
        0
    );
}

#[test]
fn dropped_future_cursor_subscription_is_pruned_before_cursor_is_crossed() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    drop(
        ledger
            .subscribe(SubscriptionCursor {
                after_sequence: 100,
            })
            .expect("future subscription"),
    );

    ledger
        .append(event("evt-before-future-cursor"))
        .expect("append before cursor");
    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("request subscriber count");

    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count"),
        0
    );
}

#[test]
fn subscription_reports_terminal_writer_failure() {
    let temp = TempDir::new().expect("temp");
    let mut ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");
    let terminal = GlobalLedgerError::fatal("test_terminal", "test_writer_failure");

    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("request subscriber count");
    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count"),
        1
    );

    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestTerminalFailure {
            error: terminal.clone(),
        })
        .expect("inject terminal failure");

    let received = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect_err("terminal writer error must reach subscription");
    assert_eq!(received, terminal);
    assert!(received.is_fatal());

    ledger.sender.take();
    let writer = ledger.writer.take().expect("writer handle");
    assert_eq!(
        writer
            .join()
            .expect("writer must not panic")
            .expect_err("writer must return terminal error"),
        terminal
    );
}

#[test]
fn slow_subscription_receives_bounded_lag_failure() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one").with_subscription_capacity(1))
        .expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");

    ledger.append(event("evt-lag-one")).expect("first event");
    ledger.append(event("evt-lag-two")).expect("second event");

    let (count_response, count_receiver) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount {
            response: count_response,
        })
        .expect("request subscriber count after lag");
    assert_eq!(
        count_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("subscriber count after lag"),
        0
    );

    let first = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect_err("lagged subscriber must receive fatal status");
    let second = subscription
        .recv_timeout(Duration::from_secs(1))
        .expect_err("lag failure must remain terminal");
    assert_eq!(first.code(), "subscription_lagged");
    assert!(first.is_fatal());
    assert_eq!(second, first);
}

#[test]
fn subscription_suppresses_events_before_a_future_cursor() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor { after_sequence: 3 })
        .expect("subscribe");

    for event_id in ["evt-one", "evt-two", "evt-three"] {
        ledger
            .append(event(event_id))
            .expect("append suppressed event");
    }
    let timeout = subscription
        .recv_timeout(Duration::from_millis(50))
        .expect_err("future cursor must suppress earlier live events");
    assert_eq!(timeout.code(), "subscription_timeout");

    let visible = ledger
        .append(event("evt-four"))
        .expect("append visible event");
    assert_eq!(
        subscription
            .recv_timeout(Duration::from_secs(1))
            .expect("future cursor event"),
        visible
    );
}

#[test]
fn cli_projection_is_concise_and_correlated() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let correlation = correlation_id();
    let expected_correlation = *correlation.transport();
    ledger
        .append(event_with_links(
            "evt-cli",
            EventLinksDraft::default().with_correlation_id(correlation),
            AuditInput::new(),
        ))
        .expect("append");

    let projected = ledger
        .project(
            EventQuery {
                correlation_id: Some(expected_correlation),
                ..EventQuery::default()
            },
            ProjectionProfile::Cli,
        )
        .expect("project");

    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].links.correlation_id(),
        Some(&expected_correlation)
    );
    assert!(matches!(projected[0].payload, ProjectionPayload::Omitted));
}

#[test]
fn ui_projection_exposes_sanitized_state_without_secret_fields() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let secret = "C:\\private\\token";
    ledger
        .append(event_with_links(
            "evt-ui",
            EventLinksDraft::default(),
            AuditInput::new().with_account(secret),
        ))
        .expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("project");

    assert_eq!(projected.len(), 1);
    let payload = serde_json::to_string(&projected[0].payload).expect("sanitized payload");
    assert!(matches!(projected[0].payload, ProjectionPayload::Public(_)));
    assert!(!payload.contains(secret));
}

#[test]
fn ui_projection_hides_forensic_fields_while_lab_retains_them() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let persisted = ledger
        .append(event_with_links(
            "evt-projection-separation",
            EventLinksDraft::default(),
            AuditInput::new()
                .with_account("secret-value")
                .with_authentication("authentication-value")
                .with_machine_path("internal-value"),
        ))
        .expect("append");

    let ui = ledger
        .project(EventQuery::default(), ProjectionProfile::Ui)
        .expect("UI project");
    let ui_payload = serde_json::to_string(&ui[0].payload).expect("UI payload");
    assert!(!ui_payload.contains("internal-value"));
    assert!(!ui_payload.contains("sha256:"));
    assert!(!ui_payload.contains("authentication_redacted"));

    let normal = ledger
        .project(EventQuery::default(), ProjectionProfile::Normal)
        .expect("Normal project");
    assert_eq!(normal[0].payload, ui[0].payload);

    let lab = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("Lab project");
    let lab_payload = serde_json::to_string(&lab[0].payload).expect("Lab payload");
    assert!(!lab_payload.contains("internal-value"));
    assert!(lab_payload.contains("[redacted]"));
    assert!(lab_payload.contains("sha256:"));
    assert!(lab_payload.contains("authentication_redacted"));
    assert_eq!(lab[0].schema_version, persisted.schema_version());
    assert_eq!(lab[0].sensitivity, persisted.sensitivity());
}

#[test]
fn lab_projection_exposes_full_sanitized_fact() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let persisted = ledger
        .append(event_with_links(
            "evt-lab",
            EventLinksDraft::default().with_run_id(run_id()),
            AuditInput::new(),
        ))
        .expect("append");

    let projected = ledger
        .project(EventQuery::default(), ProjectionProfile::Lab)
        .expect("project");

    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].sequence, persisted.sequence());
    assert_eq!(projected[0].schema_version, persisted.schema_version());
    assert_eq!(projected[0].sensitivity, persisted.sensitivity());
    assert_eq!(&projected[0].links, persisted.links());
    assert_eq!(
        projected[0].payload,
        ProjectionPayload::Full(Box::new(persisted.payload().clone()))
    );
    assert!(projected[0].artifacts.is_empty());
}

#[test]
fn indexes_rebuild_after_reopen() {
    let temp = TempDir::new().expect("temp");
    let links = EventLinksDraft::default()
        .with_request_id(request_id())
        .with_correlation_id(correlation_id());
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first ledger");
    let appended = first
        .append(event_with_links(
            "evt-reopen",
            links.clone(),
            AuditInput::new(),
        ))
        .expect("append");
    first.close().expect("close first");

    let reopened = GlobalLedger::open(config(&temp, "writer-two")).expect("reopen");
    assert_eq!(
        reopened
            .query(EventQuery {
                request_id: links.request_id().copied(),
                correlation_id: links.correlation_id().copied(),
                ..EventQuery::default()
            })
            .expect("query rebuilt index"),
        vec![appended]
    );
}

#[test]
fn query_intersects_multiple_links_in_sequence_order() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let instance = instance_id();
    let request = request_id();
    let other_request = request_id();
    let expected_instance = *instance.transport();
    let expected_request = *request.transport();
    let first = ledger
        .append(event_with_links(
            "evt-intersection-first",
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(request),
            AuditInput::new(),
        ))
        .expect("first matching event");
    ledger
        .append(event_with_links(
            "evt-intersection-other",
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(other_request),
            AuditInput::new(),
        ))
        .expect("nonmatching event");
    let second = ledger
        .append(event_with_links(
            "evt-intersection-second",
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(request),
            AuditInput::new(),
        ))
        .expect("second matching event");

    let matching = ledger
        .query(EventQuery {
            instance_id: Some(expected_instance),
            request_id: Some(expected_request),
            ..EventQuery::default()
        })
        .expect("intersection query");
    assert_eq!(matching, vec![first.clone(), second.clone()]);
    assert_eq!(
        ledger
            .query(EventQuery {
                instance_id: Some(expected_instance),
                request_id: Some(expected_request),
                from_sequence: Some(second.sequence()),
                ..EventQuery::default()
            })
            .expect("bounded intersection query"),
        vec![second]
    );
}

fn segment_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root.join("segments"))
        .expect("segments")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_events(root: &Path) -> Vec<PersistedEvent> {
    let mut events = Vec::new();
    for path in segment_paths(root) {
        let text = fs::read_to_string(path).expect("segment text");
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).expect("line JSON");
            let stored: StoredEventRecord =
                serde_json::from_value(value["event"].clone()).expect("stored event");
            events.push(stored.into_event().expect("persisted event"));
        }
    }
    events
}

fn write_owner_metadata(root: &Path, active: bool, valid: bool) {
    fs::create_dir_all(root).expect("root");
    let content = if valid {
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v2",
            "owner_id": "previous-owner",
            "pid": 999_999_u32,
            "active": active,
            "started_at_unix_ms": 1_u64,
            "closed_at_unix_ms": Value::Null
        })
        .to_string()
    } else {
        "{not-json".to_string()
    };
    fs::write(root.join("writer.lock"), content).expect("owner metadata");
}

#[test]
fn sha256_fingerprinter_requires_non_empty_private_salt() {
    let error = Sha256SecretFingerprinter::new([]).expect_err("empty fingerprinter salt must fail");

    assert_eq!(error.code(), "invalid_redactor_config");
}

#[test]
fn sha256_fingerprinter_returns_fixed_lowercase_fingerprint() {
    let fingerprinter = Sha256SecretFingerprinter::new(b"private-salt").expect("fingerprinter");

    let fingerprint = fingerprinter
        .fingerprint(SecretField::AccountIdentity, "secret-value")
        .expect("fingerprint");

    assert!(fingerprint.as_str().starts_with("sha256:"));
    assert_eq!(fingerprint.as_str().len(), 71);
    assert!(
        fingerprint.as_str()[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    );
    assert!(!fingerprint.as_str().contains("secret-value"));
}

#[test]
fn config_debug_redacts_owner_and_root() {
    let temp = TempDir::new().expect("temp");
    let owner = "private-writer-owner";
    let config = config(&temp, owner);

    let debug = format!("{config:?}");

    assert!(!debug.contains(&temp.path().display().to_string()));
    assert!(!debug.contains(owner));
    assert!(debug.contains("<redacted-root>"));
    assert!(debug.contains("<redacted-owner-id>"));
}

#[test]
fn shutdown_waits_for_a_full_ingress_queue_to_drain() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (prefill_response, _prefill_receiver) = mpsc::sync_channel(1);
    sender
        .send(WriterCommand::Shutdown {
            response: prefill_response,
        })
        .expect("fill queue");
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let _ = receiver.recv().expect("prefill");
        if let WriterCommand::Shutdown { response } = receiver.recv().expect("shutdown") {
            response.send(Ok(())).expect("shutdown response");
        }
        Ok(())
    });
    let ledger = GlobalLedger {
        sender: Some(sender),
        writer: Some(writer),
    };
    let (done_sender, done_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = done_sender.send(ledger.close());
    });

    let result = done_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("close must not deadlock");

    result.expect("close");
}

#[test]
fn second_writer_is_rejected_while_first_is_alive() {
    let temp = TempDir::new().expect("temp");
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first writer");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("second writer must fail");

    assert_eq!(error.code(), "writer_conflict");
    first.close().expect("close first");
}

#[test]
fn malformed_writer_metadata_is_fatal_without_path_disclosure() {
    let temp = TempDir::new().expect("temp");
    write_owner_metadata(temp.path(), true, false);

    let error =
        GlobalLedger::open(config(&temp, "writer-one")).expect_err("malformed metadata must fail");

    assert_eq!(error.code(), "malformed_owner_metadata");
    assert!(
        !error
            .to_string()
            .contains(&temp.path().display().to_string())
    );
}

#[test]
fn contradictory_writer_metadata_is_fatal() {
    let cases = [
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v2",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": true,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 11
        }),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v2",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": Value::Null
        }),
    ];
    for metadata in cases {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("writer.lock"), metadata.to_string()).expect("metadata");

        let error = GlobalLedger::open(config(&temp, "writer-new"))
            .expect_err("contradictory metadata must fail");

        assert_eq!(error.code(), "malformed_owner_metadata");
    }
}

#[test]
fn backward_close_wall_clock_is_valid_diagnostic_metadata() {
    let temp = TempDir::new().expect("temp");
    fs::write(
        temp.path().join("writer.lock"),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v2",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 9
        })
        .to_string(),
    )
    .expect("metadata");

    GlobalLedger::open(config(&temp, "writer-new"))
        .expect("wall-clock ordering is diagnostic only")
        .close()
        .expect("close writer");
}

#[test]
fn stale_active_owner_is_recovered_explicitly() {
    let temp = TempDir::new().expect("temp");
    write_owner_metadata(temp.path(), true, true);

    GlobalLedger::open(config(&temp, "writer-new"))
        .expect("recover writer")
        .close()
        .expect("close");

    let events = read_events(temp.path());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), EventType::LedgerRecovered);
    assert_eq!(events[0].sequence(), 1);
}

#[test]
fn append_assigns_contiguous_sequences_across_reopen() {
    let temp = TempDir::new().expect("temp");
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first");
    assert_eq!(
        first
            .append(event("evt-one"))
            .expect("append one")
            .sequence(),
        1
    );
    first.close().expect("close first");

    let second = GlobalLedger::open(config(&temp, "writer-two")).expect("second");
    assert_eq!(
        second
            .append(event("evt-two"))
            .expect("append two")
            .sequence(),
        2
    );
    second.close().expect("close second");

    let sequences = read_events(temp.path())
        .into_iter()
        .map(|event| event.sequence())
        .collect::<Vec<_>>();
    assert_eq!(sequences, vec![1, 2]);
}

#[test]
fn truncated_final_tail_is_quarantined_and_reported() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let first = event("evt-one");
    let first_event_id = *first.event_id();
    ledger.append(first).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment")
        .write_all(br#"{"line_type":"event""#)
        .expect("write tail");

    let recovered = GlobalLedger::open(config(&temp, "writer-two")).expect("recover");
    let after_recovery = event("evt-after-recovery");
    let after_recovery_event_id = *after_recovery.event_id();
    assert_eq!(
        recovered
            .append(after_recovery)
            .expect("append after recovery")
            .sequence(),
        3
    );
    recovered.close().expect("close recovered");

    let events = read_events(temp.path());
    assert_eq!(events[0].event_id(), &first_event_id);
    assert_eq!(events[1].event_type(), EventType::LedgerRecovered);
    assert_eq!(events[2].event_id(), &after_recovery_event_id);
    let quarantine_count = fs::read_dir(temp.path().join("quarantine"))
        .expect("quarantine")
        .count();
    assert_eq!(quarantine_count, 1);
}

#[test]
fn complete_corrupt_line_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    let mut file = OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment");
    file.write_all(b"not-json\n").expect("write corrupt line");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("complete corruption must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn complete_blank_line_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment")
        .write_all(b"\n")
        .expect("write blank record");

    let error = GlobalLedger::open(config(&temp, "writer-two"))
        .expect_err("blank complete record must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn duplicate_json_key_is_fatal_without_disclosing_the_hidden_value() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    let secret = r"C:\hidden\duplicate-subject";
    let text = fs::read_to_string(&segment).expect("segment text");
    let encoded_secret = serde_json::to_string(secret).expect("encode secret");
    let forged = text.replacen(
        r#""action":"runtime.start""#,
        &format!(r#""action":{encoded_secret},"action":"runtime.start""#),
        1,
    );
    assert_ne!(forged, text);
    fs::write(segment, forged).expect("write duplicate-key line");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("duplicate key must fail");

    assert_eq!(error.code(), "corrupt_segment");
    assert!(!error.to_string().contains(secret));
}

#[test]
fn non_final_segment_corruption_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let small = GlobalLedgerConfig::new(temp.path(), "writer-one")
        .with_segment_max_bytes(256)
        .with_ingress_capacity(8);
    let ledger = GlobalLedger::open(small).expect("ledger");
    ledger.append(event("evt-one")).expect("one");
    ledger.append(event("evt-two")).expect("two");
    ledger.append(event("evt-three")).expect("three");
    ledger.close().expect("close");
    let segments = segment_paths(temp.path());
    assert!(segments.len() >= 2);
    OpenOptions::new()
        .append(true)
        .open(&segments[0])
        .expect("first segment")
        .write_all(b"truncated")
        .expect("write corrupt tail");

    let error = GlobalLedger::open(config(&temp, "writer-two"))
        .expect_err("non-final corruption must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn empty_non_final_segment_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let segments = temp.path().join("segments");
    fs::create_dir_all(&segments).expect("segments");
    fs::write(segments.join("segment-000001.jsonl"), []).expect("empty first segment");
    fs::write(segments.join("segment-000002.jsonl"), []).expect("empty final segment");

    let error = GlobalLedger::open(config(&temp, "writer-one"))
        .expect_err("empty non-final segment must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn sole_final_empty_segment_is_valid() {
    let temp = TempDir::new().expect("temp");
    let segments = temp.path().join("segments");
    fs::create_dir_all(&segments).expect("segments");
    fs::write(segments.join("segment-000001.jsonl"), []).expect("empty final segment");

    GlobalLedger::open(config(&temp, "writer-one"))
        .expect("sole final empty segment is valid")
        .close()
        .expect("close writer");
}

#[test]
fn duplicate_event_id_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    let duplicate_id = event_id();
    ledger
        .append(event_with_id_and_links(
            "evt-duplicate",
            duplicate_id,
            EventLinksDraft::default(),
            AuditInput::new(),
        ))
        .expect("first");

    let error = ledger
        .append(event_with_id_and_links(
            "evt-duplicate",
            duplicate_id,
            EventLinksDraft::default(),
            AuditInput::new(),
        ))
        .expect_err("duplicate event must fail");

    assert_eq!(error.code(), "duplicate_event_id");
    ledger.close().expect("close");
}
