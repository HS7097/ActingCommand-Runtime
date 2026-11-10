// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-5b test 7, the ledger's half (`ledger` revision 8; register §1.291;
// contracts/selection-graph.md, "Records"; the contract's half is the test of the same name in
// `actingcommand-contract`): a judge step's `task.selection_evaluated`, with the confirmation
// `judge_only`, is appended to a formal root and read back exactly when the root is opened again.
// A reader whose vocabulary of confirmation kinds lacks the stored kind refuses the record: the
// confirmation has no catch-all, so the root does not open (`corrupt_ledger_record` at
// `decode_sqlite_record`). That is what a reader of `ledger` 7, whose vocabulary has no
// `judge_only`, does with this record; this build cannot run that reader, so the stored kind is
// renamed to a word this reader lacks.

use super::missing_material::formal_root;
use crate::global::{
    GlobalLedger, GlobalLedgerResult, LedgerMaintenance, LedgerOpenTiming,
    Sha256SecretFingerprinter,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CandidateFeature, CandidateFeatureMap, CandidateFrame, CandidateIdentitySource,
    CandidateLayoutKind, CandidateProjection, CandidateRect, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, IdentifierIssuer,
    OriginModule, PolicyReasonRecord, ProjectedCandidate, TaskPayloadDraft, TaskSelectionBlocking,
    TaskSelectionCandidateStatus, TaskSelectionConfirmation, TaskSelectionOutcome,
    TaskSelectionPolicy, TaskSelectionRecord, TaskSelectionUnknownCheck, TaskSelectionUpperBound,
    TaskSelectionVerdict, TaskSemanticFact,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use serde_json::{Value, json};
use std::sync::Arc;

// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;
const LAYOUT: &str = "layout/slots";
const PAGE: &str = "list_page";
const STEP: u32 = 0;
const OPERATION: &str = "judge_choices";
// The stored record's event type, as the `ledger_events` table indexes it.
const SELECTION_ROW: &str = "event_type='task.selection_evaluated'";

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
        "s168-5b-reopen".into(),
        None,
        &mut LedgerOpenTiming::default(),
    )
}

fn candidate_id(index: u32) -> String {
    format!("{LAYOUT}#{index:02}")
}

// Two fixed slots, each read for its member.
fn slots() -> CandidateProjection {
    let slot = |index: u32, member: &str| ProjectedCandidate {
        id: candidate_id(index),
        instance_index: index,
        actionable: true,
        rect: CandidateRect {
            x: 200 + 300 * index as i32,
            y: 200,
            width: 240,
            height: 120,
        },
        click: CandidateRect {
            x: 210 + 300 * index as i32,
            y: 210,
            width: 220,
            height: 100,
        },
        features: CandidateFeatureMap::from([(
            "member".to_owned(),
            CandidateFeature::Identity {
                value: member.to_owned(),
                variant: None,
                source: CandidateIdentitySource::OcrAlias,
                distance: 0,
                confidence: Some(950),
            },
        )]),
    };
    CandidateProjection::new(
        PAGE,
        LAYOUT,
        CandidateLayoutKind::FixedSlots,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        vec![slot(0, "member.a"), slot(1, "member.b")],
    )
    .expect("the frame projection is valid")
}

fn ranked(index: u32, rank: u32) -> TaskSelectionVerdict {
    TaskSelectionVerdict {
        candidate_id: candidate_id(index),
        status: TaskSelectionCandidateStatus::Ranked,
        score_milli: Some(0),
        rank: Some(rank),
        gates: Vec::new(),
        terms: Vec::new(),
        reasons: Vec::new(),
    }
}

// The judge judged both slots worth, in rank order, with nothing unknown.
fn judged() -> TaskSelectionRecord {
    TaskSelectionRecord {
        layout_id: LAYOUT.to_owned(),
        page_id: PAGE.to_owned(),
        projection: slots().into(),
        policy: TaskSelectionPolicy {
            path: "operations/task/policies/judge.json".to_owned(),
            policy_sha256: format!("sha256:{}", "d".repeat(64)),
            policy_id: "choices".to_owned(),
        },
        fact_snapshot_id: "snapshot-a".to_owned(),
        input_ledger_position: 1,
        input_fact_revisions: None,
        now_unix_ms: AT_MS,
        input_sha256: format!("sha256:{}", "e".repeat(64)),
        selection_result: TaskSelectionOutcome::Selected { count: 2 },
        outcome_key: "choices_selected".to_owned(),
        selected: vec![candidate_id(0), candidate_id(1)],
        verdicts: vec![ranked(0, 1), ranked(1, 2)],
        reasons: vec![PolicyReasonRecord(Outcome::relayed("selection_selected"))],
        confirmation: TaskSelectionConfirmation::JudgeOnly,
        unknown_check: Some(TaskSelectionUnknownCheck {
            unknown_instances: 0,
            blocking: TaskSelectionBlocking::None,
            upper_bound: TaskSelectionUpperBound::NoneUnknown,
        }),
        multi_select: None,
    }
}

#[test]
fn judge_record_reader_rules() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let step = ids.mint_action_id().expect("step action id");
    let links = EventLinksDraft::default()
        .with_instance_id(ids.mint_instance_id().expect("instance id"))
        .with_request_id(ids.mint_request_id().expect("request id"))
        .with_correlation_id(ids.mint_correlation_id().expect("correlation id"))
        .with_task_id(ids.mint_task_id().expect("task id"))
        .with_run_id(ids.mint_run_id().expect("run id"))
        .with_action_id(step)
        .with_frame_id(ids.mint_frame_id().expect("frame id"));
    let record = judged();
    record
        .validate_for_append()
        .expect("the writer's checks pass");
    let fingerprinter = Sha256SecretFingerprinter::new(b"gate-428-s168-5b").expect("fingerprinter");
    let draft = EventDraft::new(
        ids.mint_event_id().expect("event id"),
        AT_MS,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        links,
        TaskPayloadDraft::semantic(
            TaskSemanticFact::SelectionEvaluated {
                step_index: STEP,
                operation_label: OPERATION.to_owned(),
                selection: Box::new(record.clone()),
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter)
    .expect("the judge's record sanitizes");
    let written = ledger.append(draft).expect("the record is appended");
    ledger.close().expect("close the writer");

    // A reader of revision 8 takes it.
    let reopened = reopen(&database).expect("the root this build wrote opens");
    let read = reopened
        .query(EventQuery {
            action_id: Some(*step.transport()),
            ..EventQuery::default()
        })
        .expect("the step's records");
    reopened.close().expect("close the reopened writer");
    assert_eq!(read, [written]);

    // The stored confirmation is the one word `judge_only`, with no member.
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("fixture connection");
    let stored: Vec<u8> = connection
        .query_row(
            &format!("SELECT canonical_record FROM ledger_events WHERE {SELECTION_ROW}"),
            [],
            |row| row.get(0),
        )
        .expect("the stored record");
    let mut stored: Value = serde_json::from_slice(&stored).expect("stored record JSON");
    let confirmation = stored
        .pointer_mut("/payload/payload/data/fact/selection/confirmation")
        .expect("the stored confirmation");
    assert_eq!(*confirmation, json!({"kind": "judge_only"}));

    // A reader whose vocabulary lacks the stored kind does not read the record.
    *confirmation = json!({"kind": "judge_only_of_a_later_reader"});
    let rewritten = serde_json::to_vec(&stored).expect("record JSON");
    assert_eq!(
        connection
            .execute(
                &format!("UPDATE ledger_events SET canonical_record=?1 WHERE {SELECTION_ROW}"),
                rusqlite::params![rewritten],
            )
            .expect("rewrite the stored record"),
        1
    );
    drop(connection);
    let Err(refused) = reopen(&database) else {
        panic!("a root whose record holds a confirmation kind the reader lacks is refused");
    };
    assert_eq!(
        (refused.code(), refused.operation()),
        ("corrupt_ledger_record", "decode_sqlite_record")
    );
}
