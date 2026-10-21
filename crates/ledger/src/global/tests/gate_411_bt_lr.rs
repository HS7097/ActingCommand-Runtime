// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 BT-LR, ledger revision 4 (contracts/v012-register.md §1.153; B-train plan §3.2,
// §3.3 and §5.3 BT-LR; ledger keys 54 and 55). The revision keeps SQLite format 2 and
// `actingcommand.event.v3` (no table or index change, no fresh state root):
// - T1 at the ledger: a formal root takes one run's revision-4 records (a layer with its
//   package version, a recognition with its candidate scores, a refused effect after its
//   step's completed effect, a capped operation) and reads every one back unchanged when it is
//   opened again as a start opens it.
// - T2 at the ledger: its writer refuses a refused effect without its step's completed effect,
//   a second refusal of the step and a second capped operation of the run, and writes none of
//   them; the run's other records are kept.

use super::missing_material::formal_root;
use crate::PersistedEvent;
use crate::global::{
    GlobalLedger, GlobalLedgerResult, LedgerMaintenance, LedgerOpenTiming,
    Sha256SecretFingerprinter,
};
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity,
    EventSource, EventType, IdentifierIssuer, IssuedRunId, OriginModule, RecognitionCandidateScore,
    RecognitionCandidateScores, SanitizedEventDraft, TaskEntryRecoveryRole, TaskPayloadDraft,
    TaskSemanticFact,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::sync::Arc;

// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;
const STEP: u32 = 2;
const OPERATION: &str = "confirm_choice";

// The writer of the existing formal root, opened as a start opens it.
fn reopen(database: &Arc<RuntimeDatabase>) -> GlobalLedgerResult<GlobalLedger> {
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(
        Arc::clone(database),
        "bt-lr-reopen".into(),
        None,
        &mut LedgerOpenTiming::default(),
    )
}

// One run's records, each a task fact under the run's links.
struct Run {
    ids: IdentifierIssuer,
    run: IssuedRunId,
    links: EventLinksDraft,
    fingerprinter: Sha256SecretFingerprinter,
    offset: u64,
}

impl Run {
    fn new() -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run = ids.mint_run_id().expect("run id");
        let links = EventLinksDraft::default()
            .with_instance_id(ids.mint_instance_id().expect("instance id"))
            .with_request_id(ids.mint_request_id().expect("request id"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
            .with_task_id(ids.mint_task_id().expect("task id"))
            .with_run_id(run);
        Self {
            ids,
            run,
            links,
            fingerprinter: Sha256SecretFingerprinter::new(b"gate-411-bt-lr")
                .expect("fingerprinter"),
            offset: 0,
        }
    }

    fn draft(&mut self, fact: TaskSemanticFact) -> SanitizedEventDraft {
        self.offset += 1;
        EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            AT_MS + self.offset,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            self.links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        )
        .sanitize(&self.fingerprinter)
        .expect("the run's record sanitizes")
    }

    fn records(&self, ledger: &GlobalLedger) -> Vec<PersistedEvent> {
        ledger
            .query(EventQuery {
                run_id: Some(*self.run.transport()),
                ..EventQuery::default()
            })
            .expect("the run's records")
    }
}

// The `contained_task_operation_runs_capped` info outcome as the record carries it; its code is
// registered by its writer slice.
fn capped_outcome() -> Outcome {
    serde_json::from_value(serde_json::json!({
        "code": "contained_task_operation_runs_capped",
        "category": "info",
        "values": {}
    }))
    .expect("the capped outcome decodes")
}

fn capped(step_index: u32) -> TaskSemanticFact {
    TaskSemanticFact::OperationCapped {
        step_index,
        operation_label: OPERATION.to_owned(),
        count: 4,
        limit_count: 4,
        outcome: capped_outcome(),
    }
}

fn effect_completed(step_index: u32) -> TaskSemanticFact {
    TaskSemanticFact::EffectCompleted {
        step_index,
        operation_label: OPERATION.to_owned(),
    }
}

fn effect_refused(step_index: u32) -> TaskSemanticFact {
    TaskSemanticFact::EffectRefused {
        step_index,
        operation_label: OPERATION.to_owned(),
        target_id: "marker/refused".to_owned(),
    }
}

// T1 at the ledger (key 54): one run's revision-4 records are written to a formal root and read
// back unchanged, in order, after the root is opened again.
#[test]
fn bt_lr_t1_a_formal_root_reads_back_a_runs_revision_4_records() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let mut run = Run::new();
    let facts = [
        TaskSemanticFact::EntryRecoveryPackageAdmitted {
            package_label: "neutral.layer".to_owned(),
            role: Some(TaskEntryRecoveryRole::Prerequisite),
            package_version: Some("2.1.0".to_owned()),
        },
        TaskSemanticFact::EntryRecoveryCompleted {
            package_label: "neutral.layer".to_owned(),
            final_page: "home".to_owned(),
            executed_steps: 1,
            role: Some(TaskEntryRecoveryRole::Prerequisite),
            package_version: Some("2.1.0".to_owned()),
        },
        TaskSemanticFact::RecognitionCompleted {
            candidate_pages: vec!["home".to_owned(), "list_page".to_owned()],
            matched_page: Some("list_page".to_owned()),
            frame_width: 1280,
            frame_height: 720,
            targets: Vec::new(),
            candidate_scores: Some(Box::new(RecognitionCandidateScores {
                pages: vec![
                    RecognitionCandidateScore {
                        page_id: "home".to_owned(),
                        passed: false,
                        best_score_milli: Some(410),
                        first_failed_target_id: Some("home/title".to_owned()),
                    },
                    RecognitionCandidateScore {
                        page_id: "list_page".to_owned(),
                        passed: true,
                        best_score_milli: Some(930),
                        first_failed_target_id: None,
                    },
                ],
                omitted: 0,
            })),
        },
        effect_completed(STEP),
        effect_refused(STEP),
        capped(STEP),
    ];
    let mut written = Vec::new();
    for fact in facts {
        let draft = run.draft(fact);
        written.push(ledger.append(draft).expect("the run's record is appended"));
    }
    ledger.close().expect("close the writer");

    let reopened = reopen(&database).expect("the root this build wrote opens");
    let read = run.records(&reopened);
    reopened.close().expect("close the reopened writer");
    assert_eq!(read, written);
    assert_eq!(
        read.iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        [
            EventType::TaskEntryPreflight,
            EventType::TaskEntryPreflight,
            EventType::TaskRecognitionCompleted,
            EventType::TaskEffectCompleted,
            EventType::TaskEffectRefused,
            EventType::TaskOperationCapped,
        ]
    );
}

// T2 at the ledger (key 55): the writer refuses a fact that breaks its run's rules and writes
// nothing for it; the records it admitted are the run's only ones.
#[test]
fn bt_lr_t2_the_writer_refuses_a_fact_out_of_its_runs_order() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let mut run = Run::new();
    let effect_invalid = SanitizationCode::InvalidTaskEffect.code().as_str();
    let payload_invalid = SanitizationCode::InvalidTaskSemanticPayload.code().as_str();
    // (label, fact, the code it is refused with, or None when it is admitted)
    let rows: [(&str, TaskSemanticFact, Option<&str>); 7] = [
        (
            "a refused effect before any completed effect",
            effect_refused(STEP),
            Some(effect_invalid),
        ),
        (
            "the previous step's completed effect",
            effect_completed(STEP - 1),
            None,
        ),
        (
            "a refused effect without its own step's completed effect",
            effect_refused(STEP),
            Some(effect_invalid),
        ),
        ("the step's completed effect", effect_completed(STEP), None),
        ("the step's refused effect", effect_refused(STEP), None),
        (
            "a second refused effect of the step",
            effect_refused(STEP),
            Some(effect_invalid),
        ),
        ("the run's capped operation", capped(STEP), None),
    ];
    let mut admitted = Vec::new();
    for (label, fact, refusal) in rows {
        let draft = run.draft(fact);
        match (ledger.append(draft), refusal) {
            (Ok(event), None) => admitted.push(event),
            (Err(error), Some(code)) => assert_eq!(error.code(), code, "{label}"),
            (result, _) => panic!("{label}: {result:?}"),
        }
    }
    let second = run.draft(capped(STEP + 1));
    let error = ledger
        .append(second)
        .expect_err("a second capped operation in the run is refused");
    assert_eq!(error.code(), payload_invalid);
    assert_eq!(run.records(&ledger), admitted);
    ledger.close().expect("close the writer");
}
