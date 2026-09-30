// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted), Workflow #308 G3 evidence: a realistic select-step record goes
// through the SQLite ledger append and read paths unchanged, and the golden projection of
// contracts/candidate-projection.md hashes to the value the document states.

use crate::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerReadOnlyConfig, Sha256SecretFingerprinter,
};
use actingcommand_contract::{
    AuditInput, CandidateFeature, CandidateFeatureMap, CandidateFrame, CandidateLayoutKind,
    CandidateProjection, CandidateRect, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayload, EventQuery, EventSeverity, EventSource, EventType, IdentifierIssuer,
    OriginModule, PolicyReasonRecord, ProjectedCandidate, TaskPayload, TaskPayloadDraft,
    TaskSelectionCandidateStatus, TaskSelectionConfirmation, TaskSelectionGateOutcome,
    TaskSelectionGateResult, TaskSelectionOutcome, TaskSelectionPolicy, TaskSelectionRecord,
    TaskSelectionTermOutcome, TaskSelectionTermResult, TaskSelectionVerdict, TaskSemanticFact,
    candidate_id,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const DOCUMENT: &str = include_str!("../../../../../contracts/candidate-projection.md");
const LAYOUT: &str = "layout/main_slots";

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn reason(code: &str, detail: &str) -> PolicyReasonRecord {
    PolicyReasonRecord {
        code: code.to_owned(),
        detail: detail.to_owned(),
    }
}

fn slot(index: u32, open: bool, mark_score: i64) -> ProjectedCandidate {
    let y = 204 + 96 * i32::try_from(index).expect("slot index");
    let mut features = CandidateFeatureMap::new();
    features.insert(
        "open".to_owned(),
        CandidateFeature::Boolean {
            value: open,
            confidence: None,
        },
    );
    features.insert(
        "marked".to_owned(),
        CandidateFeature::Boolean {
            value: mark_score >= 900,
            confidence: Some(mark_score),
        },
    );
    features.insert(
        "mark_score".to_owned(),
        CandidateFeature::Integer {
            value: mark_score,
            confidence: Some(mark_score),
        },
    );
    ProjectedCandidate {
        id: candidate_id(LAYOUT, index).expect("candidate id"),
        instance_index: index,
        actionable: true,
        rect: CandidateRect {
            x: 214,
            y,
            width: 176,
            height: 90,
        },
        click: CandidateRect {
            x: 224,
            y: y + 10,
            width: 156,
            height: 60,
        },
        features,
    }
}

fn ranked(index: u32, marked: bool, rank: u32) -> TaskSelectionVerdict {
    let transformed = if marked { 1_000 } else { 0 };
    let mut reasons = vec![
        reason("candidate.scored", &format!("score_milli={transformed}")),
        reason("candidate.ranked", &format!("rank={rank}")),
    ];
    if rank == 1 {
        reasons.push(reason("candidate.selected", "chosen"));
    }
    TaskSelectionVerdict {
        candidate_id: candidate_id(LAYOUT, index).expect("candidate id"),
        status: TaskSelectionCandidateStatus::Ranked,
        score_milli: Some(transformed),
        rank: Some(rank),
        gates: vec![TaskSelectionGateResult {
            gate_id: "open".to_owned(),
            outcome: TaskSelectionGateOutcome::Passed,
        }],
        terms: vec![TaskSelectionTermResult {
            term_id: "preferred_slot".to_owned(),
            outcome: TaskSelectionTermOutcome::Scored {
                transformed_milli: transformed,
            },
            weight_milli: 1_000,
            contribution_milli: transformed,
        }],
        reasons,
    }
}

fn record() -> TaskSelectionRecord {
    let projection = CandidateProjection::new(
        "list_page",
        LAYOUT,
        CandidateLayoutKind::FixedSlots,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        vec![
            slot(0, true, 412),
            slot(1, true, 388),
            slot(2, true, 991),
            slot(3, true, 405),
            slot(4, false, 397),
        ],
    )
    .expect("realistic projection");
    let evaluated = projection.candidate_set_sha256().to_owned();
    let policy_sha256 = format!("sha256:{}", digest(b"canonical main_slots policy"));
    let input_sha256 = format!("sha256:{}", digest(b"candidates, facts and instant"));
    TaskSelectionRecord {
        layout_id: LAYOUT.to_owned(),
        page_id: "list_page".to_owned(),
        projection,
        policy: TaskSelectionPolicy {
            path: "policies/main_slots.json".to_owned(),
            package_sha256: digest(b"main_slots policy document bytes"),
            policy_sha256: policy_sha256.clone(),
            policy_id: "main_slots".to_owned(),
        },
        fact_snapshot_id: "snapshot:fact:selection".to_owned(),
        input_ledger_position: 42,
        now_unix_ms: 1_759_300_000_000,
        input_sha256: input_sha256.clone(),
        outcome: TaskSelectionOutcome::Selected { count: 1 },
        outcome_key: "selected".to_owned(),
        selected: vec![candidate_id(LAYOUT, 2).expect("candidate id")],
        verdicts: vec![
            ranked(0, false, 2),
            ranked(1, false, 3),
            ranked(2, true, 1),
            ranked(3, false, 4),
            TaskSelectionVerdict {
                candidate_id: candidate_id(LAYOUT, 4).expect("candidate id"),
                status: TaskSelectionCandidateStatus::GateRejected,
                score_milli: None,
                rank: None,
                gates: vec![TaskSelectionGateResult {
                    gate_id: "open".to_owned(),
                    outcome: TaskSelectionGateOutcome::Failed,
                }],
                terms: Vec::new(),
                reasons: vec![reason("gate.rejected", "gate `open` does not hold")],
            },
        ],
        reasons: vec![
            reason("policy.identity", &policy_sha256),
            reason("input.identity", &input_sha256),
            reason("selection.requirement", "ExactlyOne required_count=1"),
            reason("selection.selected", "1 of 4 surviving"),
            reason("selection.outcome_key", "selected"),
        ],
        confirmation: TaskSelectionConfirmation::Matched {
            candidate_set_sha256: evaluated,
        },
    }
}

#[test]
fn gc3_golden_projection_hash_equals_the_document() {
    let open = DOCUMENT.find("```json").expect("golden block");
    let body = open + DOCUMENT[open..].find('\n').expect("golden block line") + 1;
    let end = body + DOCUMENT[body..].find("```").expect("golden block end");
    let golden: CandidateProjection =
        serde_json::from_str(&DOCUMENT[body..end]).expect("golden projection decodes");
    golden.validate().expect("golden projection validates");
    let marker = "and its hash is `";
    let at = DOCUMENT.find(marker).expect("stated hash") + marker.len();
    let stated = &DOCUMENT[at..at + 64];
    let computed = golden
        .compute_candidate_set_sha256()
        .expect("golden candidate-set hash");
    println!(
        "gc3 golden: document states {stated}; sealed {}; recomputed {computed}",
        golden.candidate_set_sha256()
    );
    assert_eq!(computed, stated);
    assert_eq!(golden.candidate_set_sha256(), stated);
}

#[test]
fn gc3_selection_record_round_trips_through_the_sqlite_ledger() {
    let root = tempfile::tempdir().expect("root");
    let database = super::sqlite_contract::database(root.path());
    let record = record();
    record.validate_for_append().expect("realistic record");
    let record_bytes = serde_json::to_vec(&record).expect("record JSON").len();
    let candidates = record.projection.candidates().len();
    let verdicts = record.verdicts.len();
    let evaluated = record.projection.candidate_set_sha256().to_owned();
    let fact = TaskSemanticFact::SelectionEvaluated {
        step_index: 3,
        operation_label: "choose_slot".to_owned(),
        selection: Box::new(record),
    };
    let expected_bytes = serde_json::to_vec(&fact).expect("fact JSON");

    let issuer = IdentifierIssuer::new().expect("issuer");
    let links = EventLinksDraft::default()
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"))
        .with_instance_id(issuer.mint_instance_id().expect("instance id"))
        .with_lease_id(issuer.mint_lease_id().expect("lease id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"));
    let draft = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        1_759_300_000_000,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        links,
        TaskPayloadDraft::semantic(fact.clone(), AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"gc3-selection").expect("salt"))
    .expect("sanitize selection record");

    let writer = GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), "gc3-writer"),
        Arc::clone(&database),
    )
    .expect("writer");
    let appended = writer.append(draft).expect("append selection record");
    writer.close().expect("close writer");

    let query = EventQuery {
        event_type: Some(EventType::TaskSelectionEvaluated),
        ..EventQuery::default()
    };
    let reader = GlobalLedger::open_sqlite_candidate_read_only(
        GlobalLedgerReadOnlyConfig::new(root.path()),
        Arc::clone(&database),
        |_| None,
    )
    .expect("read-only reader");
    let read = reader.query(&query);
    assert_eq!(read, vec![appended.clone()]);
    let reopened = GlobalLedger::open_sqlite_candidate(
        GlobalLedgerConfig::new(root.path(), "gc3-reopen"),
        Arc::clone(&database),
    )
    .expect("reopened writer");
    let reread = reopened.query(query).expect("reopened query");
    assert_eq!(reread, vec![appended.clone()]);
    reopened.close().expect("close reopened writer");

    for event in [&read[0], &reread[0]] {
        let EventPayload::Task(TaskPayload::Semantic(payload)) = event.payload() else {
            panic!("task semantic payload expected");
        };
        assert_eq!(payload.fact(), &fact);
        assert_eq!(
            serde_json::to_vec(payload.fact()).expect("read fact JSON"),
            expected_bytes
        );
    }
    println!(
        "gc3 record: fixed_slots, {candidates} candidates, {verdicts} verdicts, selected layout/main_slots#02, confirmation matched {evaluated}, record {record_bytes} bytes compact"
    );
    println!(
        "gc3 round trip: sequence {}, event type {}, read-only reader and reopened writer return the appended fact; fact JSON identical ({} bytes)",
        appended.sequence(),
        serde_json::to_string(&appended.event_type()).expect("event type JSON"),
        expected_bytes.len()
    );
}
