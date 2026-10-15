// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #357 DB-3s (ledger gate 48), frozen model M-357b v1.0 §5.3, §5.4 and §12.6 T16: a
// multi select taps once per pick. Under the step's one action, its tap intents (each linked to
// its pick's confirmation frame) and a scan move between them come first, its selection record
// comes after them (linked to the last tap check's frame), and one `task.effect_completed`
// closes the step. A formal root takes them in that order and reads every one back when it is
// opened again: no reader of revision 3 requires the selection record before the step's first
// intent.

use super::missing_material::formal_root;
use crate::global::{
    GlobalLedger, GlobalLedgerResult, LedgerMaintenance, LedgerOpenTiming,
    Sha256SecretFingerprinter,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CandidateFeature, CandidateFeatureMap, CandidateFrame, CandidateIdentitySource,
    CandidateRect, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventPayload,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, EventType, FrameId,
    IdentifierIssuer, InputAction, IssuedFrameId, OriginModule, PolicyReasonRecord, ScanAxis,
    ScanCandidate, ScanCandidateProjection, ScanCandidateProjectionParts, ScanCoverage,
    ScanCoverageState, ScanEndEvidence, ScanGatedOut, ScanReposition, ScanStartEvidence,
    ScanViewport, ScanViewportMotion, TaskPayload, TaskPayloadDraft, TaskSelectionBlocking,
    TaskSelectionCandidateStatus, TaskSelectionConfirmation, TaskSelectionMultiSelect,
    TaskSelectionOutcome, TaskSelectionPick, TaskSelectionPickOrder, TaskSelectionPolicy,
    TaskSelectionRecord, TaskSelectionUnknownCheck, TaskSelectionUpperBound, TaskSelectionVerdict,
    TaskSemanticFact,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::sync::Arc;

// TCLOCK: every timestamp is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;
const LAYOUT: &str = "list/rows";
const PAGE: &str = "list_page";
const STEP: u32 = 3;
const OPERATION: &str = "pick_members";

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
        "db3s-reopen".into(),
        None,
        &mut LedgerOpenTiming::default(),
    )
}

fn candidate_id(index: u32) -> String {
    format!("{LAYOUT}#{index:02}")
}

// Six members of a list scanned in two viewports, each with its identity and its mark.
fn scanned_list(viewports: [IssuedFrameId; 2]) -> ScanCandidateProjection {
    let members = (0..6)
        .map(|index| ScanCandidate {
            id: candidate_id(index),
            instance_index: index,
            actionable: true,
            content_rect: CandidateRect {
                x: 40,
                y: 100 + 200 * index as i32,
                width: 1200,
                height: 180,
            },
            fixed_viewport: index / 3,
            features: CandidateFeatureMap::from([
                (
                    "member".to_owned(),
                    CandidateFeature::Identity {
                        value: format!("member_{index:02}"),
                        variant: None,
                        source: CandidateIdentitySource::OcrAlias,
                        distance: 0,
                        confidence: Some(950),
                    },
                ),
                (
                    "picked_mark".to_owned(),
                    CandidateFeature::Boolean {
                        value: false,
                        confidence: Some(950),
                    },
                ),
            ]),
        })
        .collect();
    let [first, second] = viewports;
    ScanCandidateProjection::new(ScanCandidateProjectionParts {
        page_id: PAGE.to_owned(),
        layout_id: LAYOUT.to_owned(),
        axis: ScanAxis::Vertical,
        frame: CandidateFrame {
            width: 1280,
            height: 720,
        },
        coverage: ScanCoverage {
            state: ScanCoverageState::Complete,
            start: Some(ScanStartEvidence::StartCheck),
            end: Some(ScanEndEvidence::EndCheck),
            extent_px: Some(1_300),
        },
        viewports: vec![
            ScanViewport {
                motion: ScanViewportMotion::Start,
                content_offset_px: 0,
                tracked_instances: 4,
                readable_instances: 3,
                unknown_instances: 0,
                frame_id: *first.transport(),
            },
            ScanViewport {
                motion: ScanViewportMotion::Forward,
                content_offset_px: 580,
                tracked_instances: 4,
                readable_instances: 3,
                unknown_instances: 0,
                frame_id: *second.transport(),
            },
        ],
        reposition: ScanReposition {
            rewind_viewports: 0,
            return_viewports: 1,
        },
        candidates: members,
        unknown: Vec::new(),
        gated_out: ScanGatedOut {
            count: 0,
            identities_sha256: "0".repeat(64),
        },
    })
    .expect("the scan projection is valid")
}

fn ranked(index: u32, score_milli: i64, rank: u32) -> TaskSelectionVerdict {
    TaskSelectionVerdict {
        candidate_id: candidate_id(index),
        status: TaskSelectionCandidateStatus::Ranked,
        score_milli: Some(score_milli),
        rank: Some(rank),
        gates: Vec::new(),
        terms: Vec::new(),
        reasons: Vec::new(),
    }
}

// A scan pick: its identity confirmed on the fresh frame `confirmed`, its tap checked as
// marked on `checked`.
fn pick(index: u32, confirmed: IssuedFrameId, checked: IssuedFrameId) -> TaskSelectionPick {
    TaskSelectionPick {
        candidate_id: candidate_id(index),
        confirmation: TaskSelectionConfirmation::IdentityMatched {
            candidate_id: candidate_id(index),
            frame_id: *confirmed.transport(),
            content_offset_px: 200 * index as i32,
        },
        tap_check: Some(TaskSelectionConfirmation::TapMarked {
            frame_id: *checked.transport(),
        }),
    }
}

// The scan chose #04 and #01 (rank order) and tapped #01, then #04 (content order).
fn selection(viewports: [IssuedFrameId; 2], picks: Vec<TaskSelectionPick>) -> TaskSelectionRecord {
    TaskSelectionRecord {
        layout_id: LAYOUT.to_owned(),
        page_id: PAGE.to_owned(),
        projection: scanned_list(viewports).into(),
        policy: TaskSelectionPolicy {
            path: "policy.json".to_owned(),
            policy_sha256: format!("sha256:{}", "d".repeat(64)),
            policy_id: "policy-a".to_owned(),
        },
        fact_snapshot_id: "snapshot-a".to_owned(),
        input_ledger_position: 1,
        input_fact_revisions: None,
        now_unix_ms: AT_MS,
        input_sha256: format!("sha256:{}", "e".repeat(64)),
        selection_result: TaskSelectionOutcome::Selected { count: 2 },
        outcome_key: "selected".to_owned(),
        selected: vec![candidate_id(4), candidate_id(1)],
        verdicts: vec![ranked(4, 900, 1), ranked(1, 700, 2)],
        reasons: vec![PolicyReasonRecord(Outcome::relayed("selection_selected"))],
        confirmation: TaskSelectionConfirmation::PicksConfirmed { count: 2 },
        unknown_check: Some(TaskSelectionUnknownCheck {
            unknown_instances: 0,
            blocking: TaskSelectionBlocking::None,
            upper_bound: TaskSelectionUpperBound::NoneUnknown,
        }),
        multi_select: Some(TaskSelectionMultiSelect {
            declared_count: 2,
            required_count: 2,
            free_slots: None,
            order: TaskSelectionPickOrder::Content,
            mark_feature: "picked_mark".to_owned(),
            pick_set_sha256: None,
            picks,
        }),
    }
}

fn confirmation_frame(confirmation: &TaskSelectionConfirmation) -> Option<&FrameId> {
    match confirmation {
        TaskSelectionConfirmation::IdentityMatched { frame_id, .. }
        | TaskSelectionConfirmation::PickMatched { frame_id } => Some(frame_id),
        _ => None,
    }
}

// T16: two tap intents with a scan move between them, then the record, then the step's one
// effect completion; written, read back, and linked as M-357b §5.4 pairs them.
#[test]
fn db3s_t16_a_multi_select_record_follows_its_tap_intents_and_reads_back() {
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
        .with_action_id(step);
    let frame = || ids.mint_frame_id().expect("frame id");
    let viewports = [frame(), frame()];
    let (first_confirmed, first_checked) = (frame(), frame());
    let (second_confirmed, second_checked) = (frame(), frame());
    let record = selection(
        viewports,
        vec![
            pick(1, first_confirmed, first_checked),
            pick(4, second_confirmed, second_checked),
        ],
    );
    record
        .validate_for_append()
        .expect("the writer's checks pass");

    let fingerprinter = Sha256SecretFingerprinter::new(b"gate-357-db3s").expect("fingerprinter");
    let intent = |action: InputAction| -> EventPayloadDraft {
        TaskPayloadDraft::semantic(
            TaskSemanticFact::EffectIntent {
                step_index: STEP,
                operation_label: OPERATION.to_owned(),
                action,
                frame_extent: None,
            },
            AuditInput::new(),
        )
        .into()
    };
    let drafts: [(Option<IssuedFrameId>, EventPayloadDraft); 5] = [
        // Pick 1's tap, decided on its confirmation frame.
        (
            Some(first_confirmed),
            intent(InputAction::Tap { x: 640, y: 290 }),
        ),
        // The return to pick 2 drags from pick 1's tap-check frame.
        (
            Some(first_checked),
            intent(InputAction::Swipe {
                x1: 640,
                y1: 600,
                x2: 640,
                y2: 200,
                duration_ms: 550,
            }),
        ),
        // Pick 2's tap, decided on its confirmation frame.
        (
            Some(second_confirmed),
            intent(InputAction::Tap { x: 640, y: 330 }),
        ),
        // The record, after the last tap check, linked to its frame.
        (
            Some(second_checked),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::SelectionEvaluated {
                    step_index: STEP,
                    operation_label: OPERATION.to_owned(),
                    selection: Box::new(record.clone()),
                },
                AuditInput::new(),
            )
            .into(),
        ),
        // The step's one effect completion.
        (
            None,
            TaskPayloadDraft::semantic(
                TaskSemanticFact::EffectCompleted {
                    step_index: STEP,
                    operation_label: OPERATION.to_owned(),
                },
                AuditInput::new(),
            )
            .into(),
        ),
    ];
    let mut written = Vec::new();
    for (offset, (frame_id, payload)) in (0_u64..).zip(drafts) {
        let event_links = match frame_id {
            Some(frame_id) => links.clone().with_frame_id(frame_id),
            None => links.clone(),
        };
        let draft = EventDraft::new(
            ids.mint_event_id().expect("event id"),
            AT_MS + offset,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            event_links,
            payload,
        )
        .sanitize(&fingerprinter)
        .expect("the step's record sanitizes");
        written.push(ledger.append(draft).expect("the record is appended"));
    }
    ledger.close().expect("close the writer");

    let reopened = reopen(&database).expect("the root this build wrote opens");
    let read = reopened
        .query(EventQuery {
            action_id: Some(*step.transport()),
            ..EventQuery::default()
        })
        .expect("the step's records");
    reopened.close().expect("close the reopened writer");
    assert_eq!(read, written);
    assert_eq!(
        read.iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        [
            EventType::TaskEffectIntent,
            EventType::TaskEffectIntent,
            EventType::TaskEffectIntent,
            EventType::TaskSelectionEvaluated,
            EventType::TaskEffectCompleted,
        ]
    );
    assert!(
        read.iter()
            .all(|event| event.links().action_id() == Some(step.transport()))
    );

    let EventPayload::Task(TaskPayload::Semantic(semantic)) = read[3].payload() else {
        panic!("the selection record is a task fact");
    };
    let TaskSemanticFact::SelectionEvaluated { selection, .. } = semantic.fact() else {
        panic!("the selection record");
    };
    assert_eq!(**selection, record);
    let picks = &selection
        .multi_select
        .as_ref()
        .expect("the record keeps its picks")
        .picks;
    // Pick k's tap intent is the step's intent linked to its confirmation frame.
    for (tap, pick) in [&read[0], &read[2]].into_iter().zip(picks) {
        assert_eq!(
            tap.links().frame_id(),
            confirmation_frame(&pick.confirmation)
        );
    }
    // The scan move links to no confirmation frame.
    let move_frame = read[1].links().frame_id().expect("the move's frame");
    assert!(
        picks
            .iter()
            .all(|pick| confirmation_frame(&pick.confirmation) != Some(move_frame))
    );
    // The record's frame is the last tap check's.
    let Some(TaskSelectionConfirmation::TapMarked { frame_id }) = &picks[1].tap_check else {
        panic!("the last pick was checked");
    };
    assert_eq!(read[3].links().frame_id(), Some(frame_id));
}
