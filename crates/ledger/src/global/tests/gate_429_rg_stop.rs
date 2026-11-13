// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 RG-STOP (0.12.58; register §1.337; contracts/global-ledger-query.md, the paragraph
// on `run_recovery`; coordinator work order rd-1113/brief-429-rg-stop.md). A scheduled run stopped
// on request (`StopInstanceRun`) showed, in the run-recovery group of its page, the stop's own
// records as failures without a recovery. The required state: in the grouping, a record whose
// outcome code is of the ladder class `planned_stop` (a stop on request, a pause) is no failure,
// unless a failure kind knows it; it is not listed and causes no gap. Everything else stays: a
// warning or worse of another code is listed with the gap it had, a run read completely whose
// context holds no failure has no group, and the records' severity, the page's rows and their
// views do not change.
//
// Every page is read twice over the same records at the same snapshot: by the formal root's
// writer (the SQLite store, the Runtime's page) and by the in-memory indexes (the Segment store's
// and an offline snapshot's page without an SQLite root); both derive the groups by one function,
// and they must agree. Every timestamp is a fixed constant; nothing sleeps.

use super::missing_material::formal_root;
use crate::PersistedEvent;
use crate::global::projection::{EventIndexes, PageSelection};
use crate::global::{GlobalLedger, Sha256SecretFingerprinter};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, DiagnosticCode, DiagnosticDetailDraft, EffectDisposition, EventAction, EventActor,
    EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity,
    EventSource, IdentifierIssuer, InputPayloadDraft, IssuedRunId, LedgerEventPosition,
    LedgerFailureResolution, LedgerMaterialReadState, LedgerReadScope, LedgerReadSource,
    LedgerRecoveryGap, LedgerRecoveryState, LedgerView, OriginModule, PolicyExecutionEventData,
    PolicyExecutionOutcome, PolicyPayloadDraft, ProjectionProfile, RuntimeErrorCode,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimePayloadDraft, TaskOutcome,
    TaskPayloadDraft, TaskSemanticFact,
};

/// TCLOCK: every timestamp is this constant plus the record's place in its run.
const AT_MS: u64 = 1_752_147_200_000;
const PROFILE: ProjectionProfile = ProjectionProfile::Ui;
/// A registered code of the category `warning` and the ladder class `none`.
const WARNING_CODE: &str = "step_budget_exhausted";

fn runtime_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

fn device_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Device,
        OriginModule::DeviceProxy,
        EventActor::Runtime,
    )
}

fn policy_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Scheduler,
        OriginModule::Policy,
        EventActor::Scheduler,
    )
}

fn position(event: &PersistedEvent) -> LedgerEventPosition {
    LedgerEventPosition {
        event_id: *event.event_id(),
        sequence: event.sequence(),
    }
}

/// One scheduled run's identities, and the records it appends, linked as the host links a policy
/// run's records (`policy_run_event_links`: instance, request, correlation, task, run and lease,
/// and an action id where a record names one).
struct Run {
    ids: IdentifierIssuer,
    run_id: IssuedRunId,
    links: EventLinksDraft,
    fingerprinter: Sha256SecretFingerprinter,
}

impl Run {
    fn new() -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run_id = ids.mint_run_id().expect("run id");
        let links = EventLinksDraft::default()
            .with_instance_id(ids.mint_instance_id().expect("instance id"))
            .with_request_id(ids.mint_request_id().expect("request id"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
            .with_task_id(ids.mint_task_id().expect("task id"))
            .with_run_id(run_id)
            .with_lease_id(ids.mint_lease_id().expect("lease id"));
        Self {
            ids,
            run_id,
            links,
            fingerprinter: Sha256SecretFingerprinter::new(b"gate-429-rg-stop")
                .expect("fingerprinter"),
        }
    }

    /// The run's links with a new action id.
    fn action_links(&self) -> EventLinksDraft {
        self.links
            .clone()
            .with_action_id(self.ids.mint_action_id().expect("action id"))
    }

    /// Appends one record. Its stored severity is its outcome's category, or its event type's
    /// when it carries none (ledger revision 3): the requested `Info` is not kept.
    fn append(
        &self,
        ledger: &GlobalLedger,
        offset_ms: u64,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            AT_MS + offset_ms,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("the record sanitizes");
        ledger.append(draft).expect("the record is appended")
    }

    /// The run's terminal: `task.failed` with `outcome`, as every scheduled run's failed or
    /// cancelled end writes it.
    fn terminal(&self, ledger: &GlobalLedger, offset_ms: u64, outcome: Outcome) -> PersistedEvent {
        self.append(
            ledger,
            offset_ms,
            runtime_origin(),
            self.links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: Some(0),
                    outcome: Some(outcome),
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// A `runtime.failed` record of the run with `outcome`, as the policy driver records a
    /// scheduled run's failure.
    fn runtime_failed(
        &self,
        ledger: &GlobalLedger,
        offset_ms: u64,
        links: EventLinksDraft,
        outcome: Outcome,
    ) -> PersistedEvent {
        self.append(
            ledger,
            offset_ms,
            runtime_origin(),
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                outcome,
                DiagnosticDetailDraft::new(
                    "fixture_category",
                    crate::LedgerLocation::AppendEvent.location(),
                    "fixture_backend",
                    "fixture_operation",
                ),
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// The records a stopped scheduled run writes, from `offset_ms` on: the Runtime's own reset
    /// of the device (`input.intent` and `input.committed` of `input.reset`), the run's terminal
    /// (`task.failed` with `contained_task_cancelled`), the policy driver's `runtime.failed` with
    /// the same code and the settlement, `policy.execution_recorded` of the kind `stopped`. The
    /// last three are returned: each an error of a code of the class `planned_stop`.
    fn stop(&self, ledger: &GlobalLedger, offset_ms: u64) -> Vec<PersistedEvent> {
        let reset = self.action_links();
        self.append(
            ledger,
            offset_ms,
            device_origin(),
            reset.clone(),
            InputPayloadDraft::intent(EventAction::InputReset, AuditInput::new()).into(),
        );
        self.append(
            ledger,
            offset_ms + 1,
            device_origin(),
            reset,
            InputPayloadDraft::committed(
                EventAction::InputReset,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        );
        let stopped = || Outcome::new(RuntimeErrorCode::ContainedTaskCancelled);
        let terminal = self.terminal(ledger, offset_ms + 2, stopped());
        let failure = self.runtime_failed(ledger, offset_ms + 3, self.action_links(), stopped());
        let settlement = self.append(
            ledger,
            offset_ms + 4,
            policy_origin(),
            self.action_links(),
            PolicyPayloadDraft::execution_recorded(
                PolicyExecutionEventData {
                    decision_id: "decision:rg-stop-fixture".to_owned(),
                    task_id: "task:rg-stop-fixture".to_owned(),
                    instance_id: "instance:rg-stop-fixture".to_owned(),
                    observed_at_unix_ms: AT_MS + offset_ms + 4,
                    outcome: PolicyExecutionOutcome::Stopped {
                        outcome: stopped(),
                        runtime_ms: 50,
                    },
                },
                AuditInput::new(),
            )
            .into(),
        );
        vec![terminal, failure, settlement]
    }

    fn query(&self) -> EventQuery {
        EventQuery {
            run_id: Some(*self.run_id.transport()),
            ..EventQuery::default()
        }
    }
}

/// The run's page as the formal root's writer answers it, checked against the in-memory page of
/// the same records at the same snapshot: their groups are equal.
fn page(ledger: &GlobalLedger, run: &Run) -> RuntimeEventQueryPage {
    let query = run.query();
    let page = ledger
        .project_view_page(
            query.clone(),
            PROFILE,
            RuntimeEventQueryPageRequest::default(),
        )
        .expect("the run's page");
    let snapshot = page.snapshot_ledger_position();
    let events = ledger
        .query(EventQuery {
            from_sequence: Some(1),
            to_sequence: Some(snapshot),
            ..EventQuery::default()
        })
        .expect("every record through the page's snapshot");
    let head = events.last().map_or(0, PersistedEvent::sequence);
    assert_eq!(head, snapshot, "the records through the snapshot");
    let in_memory = EventIndexes::from_events(&events)
        .project_view_page(
            &events,
            &query,
            PROFILE,
            &RuntimeEventQueryPageRequest::default(),
            LedgerReadScope {
                source: LedgerReadSource::Runtime,
                material_read: LedgerMaterialReadState::NotRequested,
                scanned_through_position: head,
                read_complete: true,
                limits: Vec::new(),
            },
            PageSelection::from(head),
        )
        .expect("the in-memory page");
    assert_eq!(
        in_memory.run_recovery(),
        page.run_recovery(),
        "the in-memory groups are the store's"
    );
    page
}

// T1: a run that was only stopped has no group. The stop's three records stay errors, rows of the
// page in the Errors view.
#[test]
fn rg_stop_t1_a_run_that_was_only_stopped_has_no_group() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let run = Run::new();
    let stop = run.stop(&ledger, 10);
    for record in &stop {
        assert_eq!(record.severity(), EventSeverity::Error, "{record:?}");
    }

    let page = page(&ledger, &run);
    assert!(page.run_recovery().is_empty(), "{:?}", page.run_recovery());
    assert_eq!(
        page.events().len(),
        5,
        "the reset's two records and the stop's three"
    );
    for record in &stop {
        let row = page
            .events()
            .iter()
            .find(|row| row.event_id == *record.event_id())
            .expect("the stop's record is a row of the page");
        assert_eq!(row.severity, EventSeverity::Error);
        assert!(row.views.contains(&LedgerView::Errors), "{:?}", row.views);
    }
    ledger.close().expect("close the writer");
}

// T2: an input failure that its completion recovered, then a stop: one group, recovered, whose
// evidence is the input failure alone.
#[test]
fn rg_stop_t2_a_recovered_input_failure_then_a_stop_is_recovered_by_the_failure_alone() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let run = Run::new();
    let input = run.action_links();
    let failure = run.append(
        &ledger,
        1,
        device_origin(),
        input.clone(),
        InputPayloadDraft::failed(
            EventAction::InputTap,
            DiagnosticCode::InputFailed,
            EffectDisposition::Indeterminate,
            AuditInput::new(),
        )
        .into(),
    );
    let recovered = run.append(
        &ledger,
        2,
        device_origin(),
        input,
        InputPayloadDraft::completed(EventAction::InputTap, AuditInput::new()).into(),
    );
    run.stop(&ledger, 10);

    let page = page(&ledger, &run);
    let [group] = page.run_recovery() else {
        panic!("one recovery group: {:?}", page.run_recovery());
    };
    assert_eq!(group.run_id, *run.run_id.transport());
    assert_eq!(group.state, LedgerRecoveryState::Recovered);
    assert!(group.gaps.is_empty(), "{:?}", group.gaps);
    assert_eq!(
        group.evidence,
        vec![LedgerFailureResolution {
            failure: position(&failure),
            success: Some(position(&recovered)),
        }]
    );
    ledger.close().expect("close the writer");
}

// T3: a warning without a recovery relation, then a stop: the group lists the warning alone,
// with the gap it had before the stop.
#[test]
fn rg_stop_t3_a_warning_then_a_stop_keeps_the_warnings_group() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let run = Run::new();
    let warning = run.runtime_failed(
        &ledger,
        1,
        run.links.clone(),
        Outcome::relayed(WARNING_CODE),
    );
    assert_eq!(warning.severity(), EventSeverity::Warning);
    let before = page(&ledger, &run);

    run.stop(&ledger, 10);
    let after = page(&ledger, &run);
    assert_eq!(
        after.run_recovery(),
        before.run_recovery(),
        "the stop changed the group"
    );
    let [group] = after.run_recovery() else {
        panic!("one recovery group: {:?}", after.run_recovery());
    };
    assert_eq!(group.state, LedgerRecoveryState::Unknown);
    assert_eq!(group.gaps, vec![LedgerRecoveryGap::MissingRelation]);
    assert_eq!(
        group.evidence,
        vec![LedgerFailureResolution {
            failure: position(&warning),
            success: None,
        }]
    );
    ledger.close().expect("close the writer");
}

// T4: a run that ended with a failure code of another class (`contained_task_deadline_exceeded`,
// `budget_or_permission`) keeps its group as before: its terminal and its `runtime.failed` are
// listed, neither recovered, and the group is unknown for its missing relation.
#[test]
fn rg_stop_t4_a_run_failed_with_another_class_keeps_its_group() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let run = Run::new();
    let deadline = || Outcome::new(RuntimeErrorCode::ContainedTaskDeadlineExceeded);
    let terminal = run.terminal(&ledger, 1, deadline());
    let failure = run.runtime_failed(&ledger, 2, run.action_links(), deadline());

    let page = page(&ledger, &run);
    let [group] = page.run_recovery() else {
        panic!("one recovery group: {:?}", page.run_recovery());
    };
    assert_eq!(group.state, LedgerRecoveryState::Unknown);
    assert_eq!(group.gaps, vec![LedgerRecoveryGap::MissingRelation]);
    assert_eq!(
        group.evidence,
        vec![
            LedgerFailureResolution {
                failure: position(&terminal),
                success: None,
            },
            LedgerFailureResolution {
                failure: position(&failure),
                success: None,
            },
        ]
    );
    ledger.close().expect("close the writer");
}

// T5: a paused run's records of `contained_task_paused` are not listed: the run has no group,
// and the records stay errors.
#[test]
fn rg_stop_t5_a_paused_runs_records_are_not_listed() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let run = Run::new();
    let paused = || Outcome::new(RuntimeErrorCode::ContainedTaskPaused);
    let records = [
        run.terminal(&ledger, 1, paused()),
        run.runtime_failed(&ledger, 2, run.action_links(), paused()),
    ];
    for record in &records {
        assert_eq!(record.severity(), EventSeverity::Error, "{record:?}");
    }

    let page = page(&ledger, &run);
    assert!(page.run_recovery().is_empty(), "{:?}", page.run_recovery());
    assert_eq!(page.events().len(), records.len());
    ledger.close().expect("close the writer");
}
