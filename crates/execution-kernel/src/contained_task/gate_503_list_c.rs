// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-C (register §1.541; contracts/selection-graph.md, "Declaration", "Select over
// a scan", "An end that must also be stationary"; contracts/resource-readings.md, "List readings";
// the coordinator's rulings 1 to 10 in the work order of board #503): a scan's `end` may write
// `require_stationary: true`. Its end check then ends the coverage only on the forward viewport on
// which the list stopped moving by the session's own judgement (the second consecutive forward
// viewport that repeats the one before it) and the check passes there. A pass in the middle of the
// list after which the list moves on counts for nothing; a list that stops where the check does
// not pass fails `selection_scan_end_unconfirmed` as before; the two extra viewports count against
// `max_viewports`. Both readers of the session go on as before after the coverage, and the
// offline entries answer what a run would have.
//
// Every expected value comes from those rulings and the fixtures below, never from the code. Two
// kinds of fixture:
//
// - A device stand-in for the session (`Device`), on the neutral grid of the SEL-L3 gate: three
//   columns at x 20, 140 and 260, a row pitch of 100 px from content y 60, cards of 100 x 80 px, a
//   readable band from y 60 to 540 and a tracking area from y 40 to 560 in a 400 x 600 frame; an
//   advance declared 180 to 400 px, a reposition at most 480 px, a tolerance of 4 px. The stand-in
//   shows the list scrolled by its offset: an advance moves it 300 px on (a page flip one page on),
//   a corrective or a return's short move toward the start as far back, a rewind or a return's long
//   move 480 px back, each within 0 and the list's foot. Its start check passes at offset 0, its
//   end check at the offsets a test names (the foot, and a look-alike in the middle); each capture
//   costs the settle interval and has a frame identity of its own.
// - Synthetic `neutral` packages admitted by the kernel's whole admission
//   (`PreparedContainedTask::load_with_vision_provider`, the declaration gate included), on
//   S168-LK's 64 x 60 frames: pixel (0, 0) names the page (home, list, detail, done); on the list
//   pixel (1, 0) is green at the list's start, pixel (2, 0) blue where its end check passes and
//   pixel (3, 0) green (the guard of the list's steps). Seven cards in one column, card k at
//   content y 10 + 12 k: a noise marker (the anchor template, 8 x 6 px) at x 4 and a 4 x 4 patch of
//   its own colour to its right, from which a scripted OCR provider reads its name. The list scrolls
//   from 0 to 48 px; at 24 px its end mark shows too where a test says so (the look-alike). The
//   path `open`, `read_members`, `close` of S168-LK reads it (a step plan); the step `pick` selects
//   over it the one card the policy wants. Runs play scripted frames on the kernel's scripted
//   clock: the device shows `screens[n]` after `n` inputs.

use super::list_reading::ListRead;
use super::scan::{
    ScanDeclaration, ScanDrag, ScanDragMotion, ScanDryRun, ScanEnd, ScanReadInstance, ScanReading,
    ScanSelectEnd, ScanSelection, ScanSession, ScanSource,
};
use super::selection::scan_selection_record;
use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateIdentitySource, CandidateRect, FactScalar,
    FrameId, InstanceFactContext, InstanceFactSnapshot, ResourceDeclarationReason,
    ScanCandidateProjection, ScanCoverageState, ScanEndEvidence, ScanViewportMotion,
    TaskSelectionConfirmation, TaskSelectionProjection, TaskSelectionRecord,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition_pack::{
    CandidateLayout, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_selection_policy::{SelectionFactSnapshot, SelectionOutcome, SelectionPolicy};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};
use std::time::{Duration, SystemTime};
use zip::{ZipWriter, write::FileOptions};

const END_UNCONFIRMED: &str = ExecutionKernelCode::SelectionScanEndUnconfirmed.as_str();
const BOUNDS_EXCEEDED: &str = ExecutionKernelCode::SelectionScanBoundsExceeded.as_str();
const FRAME_MISSING: &str = ExecutionKernelCode::SelectionScanReplayFrameMissing.as_str();
const PROGRAM_INVALID: &str = ExecutionKernelCode::ContainedTaskProgramInvalid.as_str();
const READING_INVALID: &str = ExecutionKernelCode::ContainedTaskResourceReadingInvalid.as_str();

// ---------------------------------------------------------------------------------------------
// The session over a device stand-in.
// ---------------------------------------------------------------------------------------------

const GRID_FRAME: (u32, u32) = (400, 600);
const COLUMNS: [i32; 3] = [20, 140, 260];
const POLICY_PATH: &str = "operations/task/policies/select.json";
// How far the stand-in moves the list on an advance, and on a reposition.
const STEP: i32 = 300;
const LONG: i32 = 480;
// The page step of a list paged by page-order evidence: five rows a page.
const PAGE: i32 = 500;

// One card of the grid, in content coordinates: its top edge, its left edge, its identity, its
// signature colour and its gate feature.
#[derive(Debug, Clone)]
struct Card {
    axis: i32,
    cross: i32,
    identity: String,
    color: [u8; 3],
    open: bool,
}

// `count` rows of three distinct open cards, the first row at content y 60.
fn cards(count: usize) -> Vec<Card> {
    (0..count * 3)
        .map(|index| Card {
            axis: 60 + 100 * i32::try_from(index / 3).expect("row"),
            cross: COLUMNS[index % 3],
            identity: format!("member-{index:02}"),
            color: [
                u8::try_from(index % 32).expect("level"),
                u8::try_from((index / 32) % 32).expect("level"),
                9,
            ],
            open: true,
        })
        .collect()
}

// Only the card of index `open` is open; every other one is closed (gated out).
fn only_open(mut list: Vec<Card>, open: usize) -> Vec<Card> {
    for (index, card) in list.iter_mut().enumerate() {
        card.open = index == open;
    }
    list
}

fn domain(list: &[Card]) -> Vec<String> {
    list.iter()
        .map(|card| card.identity.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn digest(color: [u8; 3]) -> ColorDigest {
    ColorDigest::from_hex(
        ColorDigestGrid::new(1, 1).expect("grid"),
        &format!("{:02x}{:02x}{:02x}", color[0], color[1], color[2]),
    )
    .expect("digest")
}

fn features(card: &Card) -> CandidateFeatureMap {
    BTreeMap::from([
        (
            "member".to_owned(),
            CandidateFeature::Identity {
                value: card.identity.clone(),
                variant: None,
                source: CandidateIdentitySource::OcrAlias,
                distance: 0,
                confidence: Some(1000),
            },
        ),
        (
            "open".to_owned(),
            CandidateFeature::Boolean {
                value: card.open,
                confidence: None,
            },
        ),
    ])
}

// The grid at content offset `offset`: every card that lies in the frame, actionable inside the
// readable band, with its features only then.
fn instances(list: &[Card], offset: i32) -> Vec<ScanReadInstance> {
    list.iter()
        .filter_map(|card| {
            let y = card.axis - offset;
            (y >= 0 && y + 80 <= 600).then(|| {
                let actionable = y >= 60 && y + 80 <= 540;
                ScanReadInstance {
                    rect: CandidateRect {
                        x: card.cross,
                        y,
                        width: 100,
                        height: 80,
                    },
                    signature: Some(digest(card.color)),
                    actionable,
                    features: if actionable {
                        features(card)
                    } else {
                        CandidateFeatureMap::new()
                    },
                    passed_over: false,
                    name_read: None,
                }
            })
        })
        .collect()
}

// The identity of the n-th capture's frame.
fn frame_id(capture: u32) -> FrameId {
    serde_json::from_value(json!(format!("frame_{capture:032x}"))).expect("frame id")
}

// The device stand-in (see the header).
struct Device {
    list: Vec<Card>,
    // The offset at the list's foot, beyond which no move scrolls it.
    foot: i32,
    // How far an advance moves the list: `STEP`, or `PAGE` for a list paged by page-order evidence.
    step: i32,
    // The offsets at which the end check passes.
    end_at: Vec<i32>,
    offset: i32,
    captures: u32,
    clock: Duration,
    drags: Vec<ScanDragMotion>,
    // The offset of the frame each `end_passed` named, in order.
    ends: Vec<i32>,
}

// A stand-in over `list` scrolled to its top, its foot at `foot`, its end check passing at
// `end_at`.
fn stand_in(list: Vec<Card>, foot: i32, end_at: &[i32]) -> Device {
    Device {
        list,
        foot,
        step: STEP,
        end_at: end_at.to_vec(),
        offset: 0,
        captures: 0,
        clock: Duration::ZERO,
        drags: Vec::new(),
        ends: Vec::new(),
    }
}

impl Device {
    fn anchors(&self) -> Vec<CandidateRect> {
        instances(&self.list, self.offset)
            .into_iter()
            .map(|instance| instance.rect)
            .collect()
    }
}

impl ScanSource for Device {
    type Error = String;

    fn elapsed(&mut self) -> Duration {
        self.clock
    }

    fn dispatched(&mut self) -> Result<Vec<CandidateRect>, ContainedTaskRunError<String>> {
        Ok(self.anchors())
    }

    fn capture(
        &mut self,
        interval: Duration,
    ) -> Result<Vec<CandidateRect>, ContainedTaskRunError<String>> {
        self.clock += interval;
        self.captures += 1;
        Ok(self.anchors())
    }

    fn viewport(&mut self) -> Result<ScanReading, ContainedTaskRunError<String>> {
        Ok(ScanReading {
            instances: instances(&self.list, self.offset),
            start_check: self.offset == 0,
            end_check: self.end_at.contains(&self.offset),
            frame_id: Some(frame_id(self.captures)),
        })
    }

    fn drag(&mut self, drag: ScanDrag) -> Result<(), ContainedTaskRunError<String>> {
        self.drags.push(drag.motion);
        let by = match drag.motion {
            ScanDragMotion::Advance
            | ScanDragMotion::Return {
                reposition: false,
                toward_start: false,
            } => self.step,
            ScanDragMotion::Corrective
            | ScanDragMotion::Return {
                reposition: false,
                toward_start: true,
            } => -self.step,
            ScanDragMotion::Rewind
            | ScanDragMotion::Return {
                reposition: true,
                toward_start: true,
            } => -LONG,
            ScanDragMotion::Return {
                reposition: true,
                toward_start: false,
            } => LONG,
        };
        self.offset = (self.offset + by).clamp(0, self.foot);
        Ok(())
    }

    // The page the list shows, one page check per page of `step`.
    fn page_checks(
        &mut self,
        _checks: &[String],
    ) -> Result<Vec<u32>, ContainedTaskRunError<String>> {
        Ok(vec![
            u32::try_from(self.offset / self.step).expect("a page"),
        ])
    }

    fn end_passed(&mut self) -> Result<(), ContainedTaskRunError<String>> {
        self.ends.push(self.offset);
        Ok(())
    }
}

// The SEL-L3 gate's declaration over the grid, its end requiring the list to be stationary when
// `stationary`; `patch` changes it further.
fn grid_scan(stationary: bool, patch: impl FnOnce(&mut Value)) -> ScanDeclaration {
    let mut declared = json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 150, "y": 450, "width": 100, "height": 20},
                    "to": {"x": 150, "y": 150, "width": 100, "height": 20},
                    "duration_ms": 500, "min_content_step_px": 180, "max_content_step_px": 400},
        "reposition": {"kind": "drag",
                       "from": {"x": 150, "y": 500, "width": 100, "height": 20},
                       "to": {"x": 150, "y": 80, "width": 100, "height": 20},
                       "duration_ms": 400, "max_content_step_px": 480},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 40, "width": 400, "height": 520},
                  "edge_margin_px": 12,
                  "signature": {"rect": {"x": 0, "y": 0, "width": 20, "height": 20},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 4,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 120, "columns": 3, "max_trailing_gap_px": 200,
                              "max_cross_lead_px": 40, "max_cross_gap_px": 140}},
        "start": {"check": "check/list_start", "rewind": true},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 32, "max_reposition_viewports": 8, "max_ms": 240000}
    });
    if stationary {
        declared["end"]["require_stationary"] = json!(true);
    }
    patch(&mut declared);
    ScanDeclaration::for_test(declared)
}

fn grid_layout(identities: &[String]) -> CandidateLayout {
    serde_json::from_value(json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 100, "height": 80},
        "click": {"x": 10, "y": 10, "width": 80, "height": 40},
        "readable_band": {"x": 0, "y": 60, "width": 400, "height": 480},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 50},
             "identity": {
                 "entries": identities.iter().map(|id| json!({"id": id, "aliases": [id]}))
                     .collect::<Vec<_>>(),
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 80, "y": 0}}
        ]
    }))
    .expect("layout")
}

// A member set: every open card is a member.
fn member_set(identities: &[String]) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-open",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": identities}},
            {"name": "open", "value_type": {"type": "boolean"}}
        ],
        "facts": [],
        "gates": [{"gate_id": "open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "open"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("policy");
    policy.validate().expect("valid policy");
    policy
}

fn no_facts() -> SelectionFactSnapshot {
    SelectionFactSnapshot {
        snapshot_id: "snapshot-empty".into(),
        snapshot_at_unix_ms: 0,
        facts: BTreeMap::new(),
    }
}

// A select step's session over the stand-in, to its coverage checks.
struct Scanned {
    result: Result<ScanEnd, ContainedTaskRunError<String>>,
    summary: ScanDryRun,
    device: Device,
}

fn scan_on(scan: &ScanDeclaration, mut device: Device) -> Scanned {
    let identities = domain(&device.list);
    let layout = grid_layout(&identities);
    let policy = member_set(&identities);
    let facts = no_facts();
    let mut session = ScanSession::new(
        scan,
        &layout,
        &policy,
        &facts,
        1_000,
        GRID_FRAME,
        Duration::from_secs(1_800),
    )
    .expect("session");
    let result = session.run(&mut device);
    let summary = session.summary(result.as_ref().ok());
    Scanned {
        result,
        summary,
        device,
    }
}

// A list reading's session over the stand-in, its one column the gate feature `open`.
struct Listed {
    list: Result<ListRead, ContainedTaskRunError<String>>,
    summary: ScanDryRun,
    device: Device,
}

fn read_on(scan: &ScanDeclaration, mut device: Device) -> Listed {
    let identities = domain(&device.list);
    let layout = grid_layout(&identities);
    let mut session = ScanSession::for_list(
        scan,
        &layout,
        vec!["open".to_owned()],
        GRID_FRAME,
        Duration::from_secs(1_800),
    )
    .expect("list session");
    let columns = [("open".to_owned(), "open".to_owned())];
    let list = session.read_list(&mut device, "id", &columns);
    let summary = session.summary(None);
    Listed {
        list,
        summary,
        device,
    }
}

// A select over the stand-in, to its confirmation, and the record it writes, validated.
struct Selected {
    selection: ScanSelection<String>,
    record: TaskSelectionRecord,
    device: Device,
}

fn select_on(scan: &ScanDeclaration, mut device: Device) -> Selected {
    let identities = domain(&device.list);
    let layout = grid_layout(&identities);
    let policy = member_set(&identities);
    let facts = no_facts();
    let mut session = ScanSession::new(
        scan,
        &layout,
        &policy,
        &facts,
        1_000,
        GRID_FRAME,
        Duration::from_secs(1_800),
    )
    .expect("session");
    let selection = session
        .select(&mut device, false)
        .unwrap_or_else(|error| panic!("expected a decision, got {error:?}"));
    let taken = session
        .projection(selection.coverage)
        .expect("the scan's projection");
    let record = scan_selection_record(
        POLICY_PATH,
        (1, 1_000),
        &selection.decision,
        taken,
        selection.unknown_check,
        selection.confirmation.clone(),
    )
    .expect("the record");
    record.validate_for_append().expect("an appendable record");
    Selected {
        selection,
        record,
        device,
    }
}

fn offsets(summary: &ScanDryRun) -> Vec<i64> {
    summary
        .viewports
        .iter()
        .map(|viewport| viewport.content_offset_px)
        .collect()
}

fn end_checks(summary: &ScanDryRun) -> Vec<bool> {
    summary
        .viewports
        .iter()
        .map(|viewport| viewport.end_check)
        .collect()
}

fn motions(summary: &ScanDryRun) -> Vec<ScanViewportMotion> {
    summary
        .viewports
        .iter()
        .map(|viewport| viewport.motion)
        .collect()
}

// A start viewport and `moves` forward ones.
fn forward(moves: usize) -> Vec<ScanViewportMotion> {
    std::iter::once(ScanViewportMotion::Start)
        .chain(std::iter::repeat_n(ScanViewportMotion::Forward, moves))
        .collect()
}

fn advances(count: usize) -> Vec<ScanDragMotion> {
    vec![ScanDragMotion::Advance; count]
}

fn assert_covered(case: &str, scanned: &Scanned) {
    assert!(
        matches!(scanned.result, Ok(ScanEnd::Covered)),
        "{case}: expected complete coverage, got {:?}",
        scanned.result
    );
    assert_eq!(
        scanned.summary.coverage,
        ScanCoverageState::Complete,
        "{case}"
    );
}

// The task failure a session ended with.
fn failure<'a>(
    case: &str,
    result: &'a Result<ScanEnd, ContainedTaskRunError<String>>,
) -> &'a ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("{case}: expected a task failure, got {other:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// The rows a list reading over the stand-in gives `list`: `{id, open}`, in ascending byte order of
// the key.
fn rows_of(list: &[Card]) -> Vec<BTreeMap<String, FactScalar>> {
    let mut ordered = list.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.identity.cmp(&right.identity));
    ordered
        .into_iter()
        .map(|card| {
            BTreeMap::from([
                ("id".to_owned(), FactScalar::String(card.identity.clone())),
                ("open".to_owned(), FactScalar::Boolean(card.open)),
            ])
        })
        .collect()
}

// The list a list reading read whole: its rows and their confidence.
fn whole(case: &str, list: &Result<ListRead, ContainedTaskRunError<String>>) -> ListReadingRows {
    match list {
        Ok(ListRead::Whole(rows)) => rows.clone(),
        other => panic!("{case}: expected the list whole, got {other:?}"),
    }
}

// The grid of the look-alike case: sixteen rows, its foot at 1160 (the last row at y 400), its end
// check passing at the foot and at 600, a position in the middle where the frame is the foot's in
// all but its cards. An advance of 300 px reaches 600 on its second move.
fn look_alike_grid() -> (Vec<Card>, i32, [i32; 2]) {
    (cards(16), 1160, [600, 1160])
}

// Test 2 (the case it is for), the select reader's half and the list reader's on the stand-in: see
// `list_c_both_readers_go_on_as_before_after_the_coverage`. Test 3: the true end reached by a move
// of 160 px, shorter than the advance's smallest step of 180, is aligned in the window an end gives
// (any step up to the largest), and two moves that do not scroll the list complete the coverage on
// the second: viewports at 0, 300, 600, 760, 760 and 760, the end check on the last three, five
// advances, the end named once, on the last viewport. The list reader reads every row.
#[test]
fn list_c_a_short_last_move_onto_the_foot_then_two_repeats_completes() {
    let scan = grid_scan(true, |_| {});
    let scanned = scan_on(&scan, stand_in(cards(12), 760, &[760]));
    assert_covered("a short last move", &scanned);
    assert_eq!(offsets(&scanned.summary), [0, 300, 600, 760, 760, 760]);
    assert_eq!(
        end_checks(&scanned.summary),
        [false, false, false, true, true, true]
    );
    assert_eq!(motions(&scanned.summary), forward(5));
    assert_eq!(scanned.device.drags, advances(5));
    assert_eq!(scanned.device.ends, [760]);

    let read = read_on(&scan, stand_in(cards(12), 760, &[760]));
    let list = whole("a short last move", &read.list);
    assert_eq!(list.rows, rows_of(&cards(12)));
    assert_eq!(list.confidence_milli, 1000);
    assert_eq!(offsets(&read.summary), [0, 300, 600, 760, 760, 760]);
    assert_eq!(read.device.ends, [760]);
}

// Test 4: a list of one screen, four rows: the start and end checks both pass on its first viewport,
// which does not end the coverage; two advances do not scroll it, and the coverage is complete on
// the second: viewports at 0, 0 and 0, each with its end check, the end named once, on the last.
// The list holds the twelve cards of the screen. Without the member it ends at once, with no move.
#[test]
fn list_c_a_list_of_one_screen_completes_after_two_advances_that_do_not_move_it() {
    let read = read_on(&grid_scan(true, |_| {}), stand_in(cards(4), 0, &[0]));
    let list = whole("one screen", &read.list);
    assert_eq!(list.rows, rows_of(&cards(4)));
    assert_eq!(offsets(&read.summary), [0, 0, 0]);
    assert_eq!(end_checks(&read.summary), [true, true, true]);
    assert_eq!(motions(&read.summary), forward(2));
    assert_eq!(read.device.drags, advances(2));
    assert_eq!(read.device.ends, [0]);

    let before = read_on(&grid_scan(false, |_| {}), stand_in(cards(4), 0, &[0]));
    assert_eq!(
        whole("one screen as before", &before.list).rows,
        rows_of(&cards(4))
    );
    assert_eq!(offsets(&before.summary), [0]);
    assert!(before.device.drags.is_empty());
    assert_eq!(before.device.ends, [0]);
}

// Test 5: the list stops where the end check does not pass. Thirteen rows, the foot at 900, reached
// by full steps (0, 300, 600, 900, then 900 twice): with no end check anywhere the scan fails
// `selection_scan_end_unconfirmed` at the stage `select_scan_end` on viewport 5, the second repeat,
// as it does without the member; with the end check passing only at 300, a look-alike, it fails the
// same, where without the member the coverage was complete on viewport 1, eight rows of thirteen.
#[test]
fn list_c_a_list_that_stops_where_the_end_check_fails_is_end_unconfirmed() {
    for (case, end_at) in [
        ("no end check", &[][..]),
        ("a look-alike at 300", &[300][..]),
    ] {
        let scanned = scan_on(&grid_scan(true, |_| {}), stand_in(cards(13), 900, end_at));
        let error = failure(case, &scanned.result);
        assert_eq!(error.code().as_str(), END_UNCONFIRMED, "{case}: {error}");
        assert_eq!(value(error, "stage"), json!("select_scan_end"), "{case}");
        assert_eq!(value(error, "viewport_index"), json!(5), "{case}");
        assert_eq!(
            offsets(&scanned.summary),
            [0, 300, 600, 900, 900, 900],
            "{case}"
        );
        assert!(scanned.device.ends.is_empty(), "{case}");
    }
    let without = scan_on(&grid_scan(false, |_| {}), stand_in(cards(13), 900, &[]));
    let error = failure("no end check, as before", &without.result);
    assert_eq!(error.code().as_str(), END_UNCONFIRMED);
    assert_eq!(value(error, "stage"), json!("select_scan_end"));
    assert_eq!(value(error, "viewport_index"), json!(5));
    let short = read_on(&grid_scan(false, |_| {}), stand_in(cards(13), 900, &[300]));
    assert_eq!(
        whole("a look-alike at 300, as before", &short.list).rows,
        rows_of(&cards(8))
    );
    assert_eq!(offsets(&short.summary), [0, 300]);
}

// Test 6: the bounds do not move. The look-alike grid with `max_viewports` 6 holds the list to its
// foot (viewports 0 to 4) and its first repeat, not the second: the sixth advance is refused,
// `selection_scan_bounds_exceeded` at `select_scan_end`, the bound `coverage_viewports`, limit 6.
// With 7 the coverage is complete.
#[test]
fn list_c_the_two_repeats_count_against_max_viewports() {
    let (list, foot, end_at) = look_alike_grid();
    let bounded = grid_scan(true, |declared| {
        declared["bounds"]["max_viewports"] = json!(6);
    });
    let scanned = scan_on(&bounded, stand_in(list.clone(), foot, &end_at));
    let error = failure("max_viewports 6", &scanned.result);
    assert_eq!(error.code().as_str(), BOUNDS_EXCEEDED, "{error}");
    assert_eq!(value(error, "stage"), json!("select_scan_end"));
    assert_eq!(value(error, "scan_bound"), json!("coverage_viewports"));
    assert_eq!(value(error, "limit_count"), json!(6));
    assert_eq!(offsets(&scanned.summary), [0, 300, 600, 900, 1160, 1160]);

    let enough = grid_scan(true, |declared| {
        declared["bounds"]["max_viewports"] = json!(7);
    });
    assert_covered(
        "max_viewports 7",
        &scan_on(&enough, stand_in(list, foot, &end_at)),
    );
}

// Test 7 and test 2 on the stand-in: both readers over the look-alike grid. Without the member the
// coverage is complete on viewport 2, at 600, where the end check passes in the middle: the list
// reader writes its first eleven rows as the whole list (today's behaviour, the tail of five rows
// missing), and the select, whose one open card is card 33 in row 11, decides `empty` over complete
// coverage. With it the scan reads on: viewports at 0, 300, 600, 900, 1160, 1160 and 1160, the end
// named once, on the last; the list holds all sixteen rows. The select decides `selected` with
// card 33 (`layout/cards#00`, the one member), returns from 1160 toward the start by one short move
// (the advance reversed, to 860, the card at y 300) and confirms it there on a fresh frame, the
// last capture, at content offset 860: the record's coverage is complete with the end evidence
// `end_check`, its seven coverage viewports the repeats included, one return viewport.
#[test]
fn list_c_both_readers_go_on_as_before_after_the_coverage() {
    let (list, foot, end_at) = look_alike_grid();

    let before = read_on(
        &grid_scan(false, |_| {}),
        stand_in(list.clone(), foot, &end_at),
    );
    assert_eq!(
        whole("the list, as before", &before.list).rows,
        rows_of(&list[..33])
    );
    assert_eq!(offsets(&before.summary), [0, 300, 600]);
    assert_eq!(before.device.ends, [600]);
    let read = read_on(
        &grid_scan(true, |_| {}),
        stand_in(list.clone(), foot, &end_at),
    );
    assert_eq!(whole("the list", &read.list).rows, rows_of(&list));
    assert_eq!(offsets(&read.summary), [0, 300, 600, 900, 1160, 1160, 1160]);
    assert_eq!(
        end_checks(&read.summary),
        [false, false, true, false, true, true, true]
    );
    assert_eq!(read.device.ends, [1160]);

    let wanted = only_open(list, 33);
    let missed = select_on(
        &grid_scan(false, |_| {}),
        stand_in(wanted.clone(), foot, &end_at),
    );
    assert_eq!(missed.selection.coverage, ScanCoverageState::Complete);
    assert_eq!(missed.selection.decision.outcome, SelectionOutcome::Empty);
    assert_eq!(viewport_offsets(projection(&missed.record)), [0, 300, 600]);

    let selected = select_on(&grid_scan(true, |_| {}), stand_in(wanted, foot, &end_at));
    let selection = &selected.selection;
    assert_eq!(selection.coverage, ScanCoverageState::Complete);
    assert!(
        matches!(
            selection.decision.outcome,
            SelectionOutcome::Selected { count: 1 }
        ),
        "{:?}",
        selection.decision.outcome
    );
    assert_eq!(selection.decision.selected, ["layout/cards#00"]);
    match &selection.end {
        ScanSelectEnd::Confirmed { identity, rect } => {
            assert_eq!(identity, "member-33");
            assert_eq!(
                *rect,
                CandidateRect {
                    x: 20,
                    y: 300,
                    width: 100,
                    height: 80
                }
            );
        }
        ScanSelectEnd::Failed(error) => panic!("expected the member confirmed, got {error:?}"),
        ScanSelectEnd::Empty => panic!("expected the member confirmed, got an empty end"),
    }
    let mut drags = advances(6);
    drags.push(ScanDragMotion::Return {
        reposition: false,
        toward_start: true,
    });
    assert_eq!(selected.device.drags, drags);
    assert_eq!(selected.device.captures, 17);
    assert_eq!(
        selection.confirmation,
        TaskSelectionConfirmation::IdentityMatched {
            candidate_id: "layout/cards#00".to_owned(),
            frame_id: frame_id(17),
            content_offset_px: 860,
        }
    );
    assert_eq!(selected.record.confirmation, selection.confirmation);
    let recorded = projection(&selected.record);
    let coverage = recorded.coverage();
    assert_eq!(coverage.state, ScanCoverageState::Complete);
    assert_eq!(coverage.end, Some(ScanEndEvidence::EndCheck));
    assert_eq!(
        viewport_offsets(recorded),
        [0, 300, 600, 900, 1160, 1160, 1160]
    );
    assert_eq!(recorded.reposition().return_viewports, 1);
}

// The scan's projection a select over a scan records.
fn projection(record: &TaskSelectionRecord) -> &ScanCandidateProjection {
    match &record.projection {
        TaskSelectionProjection::V2(projection) => projection,
        TaskSelectionProjection::V1(_) => panic!("a select over a scan records the v2 projection"),
    }
}

// The content offsets of a record's coverage viewports.
fn viewport_offsets(projection: &ScanCandidateProjection) -> Vec<i32> {
    projection
        .viewports()
        .iter()
        .map(|viewport| viewport.content_offset_px)
        .collect()
}

// Test 8, nothing moved: the SEL-L3 gate's list of twelve rows read to its foot at 760 without the
// member is covered at the first viewport whose end check passes, as `gate_308_l3`'s
// `l3_a_list_at_its_top_is_covered_to_its_end` pins it: viewports 0, 300, 600 and 760, three
// advances, the end named once, on the last viewport.
#[test]
fn list_c_a_scan_without_the_member_ends_as_before() {
    let scanned = scan_on(&grid_scan(false, |_| {}), stand_in(cards(12), 760, &[760]));
    assert_covered("without the member", &scanned);
    assert_eq!(offsets(&scanned.summary), [0, 300, 600, 760]);
    assert_eq!(end_checks(&scanned.summary), [false, false, false, true]);
    assert_eq!(scanned.device.drags, advances(3));
    assert_eq!(scanned.device.ends, [760]);
}

// Ruling 5: a scan placed by page-order evidence has the stopped-moving judgement too (a forward
// viewport that shows the same page as the one before it repeats it), so the member means the same
// there. Fifteen rows on three pages of five rows (page step 500), the end check on the last page:
// with the member, viewports at 0, 500, 1000, 1000 and 1000, the end named once, on the last;
// without it, 0, 500 and 1000.
#[test]
fn list_c_a_scan_paged_by_page_order_evidence_ends_on_its_second_repeat_too() {
    let paged = |stationary: bool| {
        grid_scan(stationary, |declared| {
            declared["advance"] = json!({
                "kind": "rect",
                "rect": {"x": 150, "y": 560, "width": 100, "height": 30},
                "back_rect": {"x": 150, "y": 10, "width": 100, "height": 30},
                "page_order": {"checks": ["check/page_0", "check/page_1", "check/page_2"],
                               "page_step_px": PAGE}
            });
            declared["reposition"] = json!({
                "kind": "rect",
                "rect": {"x": 150, "y": 560, "width": 100, "height": 30},
                "back_rect": {"x": 150, "y": 10, "width": 100, "height": 30},
                "max_content_step_px": 480
            });
        })
    };
    let paged_device = || Device {
        step: PAGE,
        ..stand_in(cards(15), 1000, &[1000])
    };
    let scanned = scan_on(&paged(true), paged_device());
    assert_covered("paged, with the member", &scanned);
    assert_eq!(offsets(&scanned.summary), [0, 500, 1000, 1000, 1000]);
    assert_eq!(scanned.device.ends, [1000]);
    let before = scan_on(&paged(false), paged_device());
    assert_covered("paged, without the member", &before);
    assert_eq!(offsets(&before.summary), [0, 500, 1000]);
}

// ---------------------------------------------------------------------------------------------
// The packages: the declaration, a list reading step's run and the offline entries.
// ---------------------------------------------------------------------------------------------

const WIDTH: u32 = 64;
const HEIGHT: u32 = 60;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const VALID_FOR_MS: u64 = 86_400_000;
const LIST_PAGE: &str = "neutral/list";
const DONE_PAGE: &str = "neutral/done";
const NAMES: [&str; 7] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
];
const MEMBERS: [&str; 7] = [
    "member-a", "member-b", "member-c", "member-d", "member-e", "member-f", "member-g",
];
const CARDS: [u32; 7] = [0, 1, 2, 3, 4, 5, 6];
const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DETAIL: [u8; 3] = [0, 0, 160];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];
const NOW_UNIX_MS: u64 = 1_000_000;
const INSTANCE: &str = "neutral.instance";

// Deterministic pixel noise, the SEL-L6b gate's: a shifted copy of it correlates with it near
// zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        (state & 0xff) as u8
    };
    [next(), next(), next()]
}

// The red level of card k's patch; the green and blue follow from it.
fn level(card: u32) -> u8 {
    u8::try_from(30 + 36 * card).expect("level")
}

// One screen: its page's colour; on the list, whether its start and end marks show, and the
// content moved up by `offset` pixels.
#[derive(Debug, Clone, Copy)]
struct Screen {
    page: [u8; 3],
    start: bool,
    end: bool,
    offset: u32,
}

fn page(colour: [u8; 3]) -> Screen {
    Screen {
        page: colour,
        start: false,
        end: false,
        offset: 0,
    }
}

// The list at content offset `offset`, its start mark at 0, its end mark where `end`.
fn list_at(offset: u32, end: bool) -> Screen {
    Screen {
        page: LIST,
        start: offset == 0,
        end,
        offset,
    }
}

fn pixel(screen: Screen, x: u32, y: u32) -> [u8; 3] {
    if y == 0 && x < 4 {
        return match x {
            0 => screen.page,
            1 if screen.start => GREEN,
            2 if screen.end => BLUE,
            3 if screen.page == LIST => GREEN,
            _ => BACKGROUND,
        };
    }
    if screen.page != LIST {
        return BACKGROUND;
    }
    let content = y + screen.offset;
    for card in CARDS {
        let top = 10 + 12 * card;
        if (top..top + 6).contains(&content) && (4..12).contains(&x) {
            return noise(x - 4, content - top);
        }
        if (top..top + 4).contains(&content) && (12..16).contains(&x) {
            let red = level(card);
            return [red, 220 - red / 2, 90];
        }
    }
    BACKGROUND
}

fn pixels(screen: Screen) -> Vec<[u8; 3]> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| pixel(screen, x, y)))
        .collect()
}

// The screen as the scripted device shows it.
fn scripted(screen: Screen) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, HEIGHT, &pixels(screen))
}

// The screen as a saved frame.
fn saved(screen: Screen) -> Frame {
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels(screen).concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("saved frame")
}

// The saved frames of a run over the viewports `screens`, as the offline entries take them: the
// dispatched frame, then every capture of the scan, each viewport settling on two equal captures
// (v0, v0, v1, v1, …).
fn settled(screens: &[Screen]) -> Vec<Frame> {
    screens
        .iter()
        .flat_map(|screen| [saved(*screen), saved(*screen)])
        .collect()
}

// The list's viewports: its top, 24 px on with the end mark where `look_alike`, its foot at 48 px
// with the end mark, and the foot `repeats` times more.
fn viewports(look_alike: bool, repeats: usize) -> Vec<Screen> {
    let mut screens = vec![
        list_at(0, false),
        list_at(24, look_alike),
        list_at(48, true),
    ];
    screens.extend(std::iter::repeat_n(list_at(48, true), repeats));
    screens
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    let pixels = (0..6)
        .flat_map(|y| (0..8).flat_map(move |x| noise(x, y)))
        .collect::<Vec<u8>>();
    Frame::from_pixels(
        8,
        6,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("marker frame")
    .png_for_artifact()
    .expect("marker PNG")
}

// Reads a card's name from the colour of its patch: the name region starts at the card's anchor,
// and the patch lies 8 px to its right.
#[derive(Debug)]
struct CardNames;

impl VisionProvider for CardNames {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let x = usize::try_from(request.region.x + 9).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let red = request.frame.rgb8_pixels[(y * width + x) * 3];
        let text = CARDS
            .iter()
            .position(|card| level(*card) == red)
            .map_or("", |card| NAMES[card]);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: Vec::new(),
            confidence: Some(0.99),
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// The cards as a `repeated_anchor` layout on the list page, each naming its member, readable over
// the whole tracking area.
fn card_layout() -> Value {
    let entries = MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    json!({
        "id": "layout/cards", "page_id": LIST_PAGE, "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity", "target": "card/name",
                      "offset": {"x": 0, "y": 0},
                      "identity": {
                          "entries": entries,
                          "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                          "minimum_margin": 1,
                                          "minimum_confidence_milli": 800}}}]
    })
}

// S168-LK's scan of the list, a vertical drag forward of 12 to 48 px, no rewind, the tracking area
// the frame below its first row; its end writing `require_stationary` as `stationary` gives it
// (absent for `None`).
fn list_scan(stationary: Option<Value>) -> Value {
    let mut scan = json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 40, "width": 20, "height": 10},
                    "to": {"x": 20, "y": 14, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12, "max_content_step_px": 48},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 50, "width": 20, "height": 10},
                       "to": {"x": 20, "y": 8, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 56},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
                  "edge_margin_px": 2,
                  "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 1,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 20, "columns": 1,
                              "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                              "max_cross_gap_px": 10}},
        "start": {"check": "check/list_start", "rewind": false},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 120_000}
    });
    if let Some(stationary) = stationary {
        scan["end"]["require_stationary"] = stationary;
    }
    scan
}

// The scan of the list paged by page-order evidence (SEL-L3b): a tap pages it on by 48 px, one page
// check per page; its end requires the list to be stationary.
fn paged_scan() -> Value {
    let mut scan = list_scan(Some(json!(true)));
    scan["advance"] = json!({
        "kind": "rect",
        "rect": {"x": 20, "y": 50, "width": 10, "height": 6},
        "back_rect": {"x": 20, "y": 6, "width": 10, "height": 6},
        "page_order": {"checks": ["check/page_0", "check/page_1"], "page_step_px": 48}
    });
    scan["reposition"] = json!({
        "kind": "rect",
        "rect": {"x": 20, "y": 50, "width": 10, "height": 6},
        "back_rect": {"x": 20, "y": 6, "width": 10, "height": 6},
        "max_content_step_px": 48
    });
    scan
}

// A trusted tap `id` from `from` to `to`.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 1, "y": 1},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": 1000}})
}

// `read_members`, the reading of the list over `scan` into the list key, guarded by the list's
// mark.
fn read_members(scan: Value) -> Value {
    json!({
        "id": "read_members", "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": {"layout_id": "layout/cards", "key_feature": "member", "key_column": "id",
                      "columns": [], "fact_key": LIST_KEY, "valid_for_ms": VALID_FOR_MS,
                      "scan": scan}
    })
}

// `close`, a tap from the list to done guarded by the list's end mark.
fn close() -> Value {
    json!({"id": "close", "from": "list", "to": "done",
           "click": {"kind": "point", "x": 30, "y": 30},
           "guard": {"page_id": "list", "target_id": "guard/end",
                     "expected_rect": {"x": 2, "y": 0, "width": 1, "height": 1},
                     "color_probe": "guard/end"},
           "expect_after": {"page_id": "done", "timeout_ms": 1000}})
}

// The path `open`, `read_members` over `scan` and `close`, from home to done.
fn path_task(scan: Value) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 3,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "listed", "effect": "no_designated_effect",
             "terminal_pages": ["done"]}
        ]},
        "candidate_layouts": [card_layout()],
        "operations": [tap("open", "home", "list"), read_members(scan), close()]
    })
}

// The task of the one step `pick`, which selects over `scan` the card the policy wants, from the
// list (the entry) to the detail page, guarded by the list's mark.
fn pick_task(scan: Value) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "list",
        "target_page": "detail",
        "max_steps": 1,
        "candidate_layouts": [card_layout()],
        "operations": [{
            "id": "pick", "from": "list", "to": "detail",
            "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
            "guard": {"page_id": "list", "target_id": "guard/list",
                      "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                      "color_probe": "guard/list"},
            "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                       "scan": scan}
        }]
    })
}

// The policy of `pick`: a card is a member when its identity is `member-g` (golf, card 6, which
// only the list's foot shows), the lowest candidate ID last.
fn pick_policy() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-wanted",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member",
                    "value_type": {"type": "enum_string", "allowed": MEMBERS}}],
        "facts": [],
        "gates": [{"gate_id": "wanted",
                   "predicate": {"kind": "string_in",
                                 "value": {"source": "field", "field": "member"},
                                 "allowed": ["member-g"]},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// The control of a step plan of `max_steps` steps.
fn control(max_steps: usize) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.list",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 32_000,
        "timeout_ms": 300_000,
        "max_steps": max_steps
    })
}

// The page set: home, list, detail and done, each told by pixel (0, 0).
fn pages() -> Value {
    let declared = ["home", "list", "detail", "done"].map(|name| {
        json!({"id": format!("neutral/{name}"), "required": [format!("page/{name}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": declared})
}

// The recognition pack: the pages' colours, the list's start and end marks, the two page checks of
// a paged scan, the guards, the card anchor and name, and the card layout.
fn pack() -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/list", 0, LIST),
            colour("page/detail", 0, DETAIL),
            colour("page/done", 0, DONE),
            colour("check/list_start", 1, GREEN),
            colour("check/list_end", 2, BLUE),
            colour("check/page_0", 1, GREEN),
            colour("check/page_1", 2, BLUE),
            colour("guard/list", 3, GREEN),
            colour("guard/end", 2, BLUE),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [card_layout()]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The step plan of `task`, with `pick`'s policy where the task selects.
fn package(task: &Value) -> Vec<u8> {
    let steps = task["operations"].as_array().map_or(0, Vec::len);
    let mut entries = vec![
        ("control.json", bytes(&control(steps))),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        ("resources/assets/card.png", marker_png()),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&pack()),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages()),
        ),
    ];
    if task["operations"][0].get("select").is_some() {
        entries.push((
            "resources/operations/task/policies/select.json",
            bytes(&pick_policy()),
        ));
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package of `task` as contained task admission prepares it, or its refusal.
fn load(task: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(INSTANCE, &package(task), Arc::new(CardNames))
}

fn admitted(case: &str, task: &Value) -> PreparedContainedTask {
    load(task).unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

fn refused(case: &str, task: &Value) -> ContainedTaskError {
    match load(task) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    }
}

// The instance's fact snapshot: no facts.
fn snapshot() -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: "snapshot:test:empty".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: INSTANCE.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    }
}

// Test 1 (ruling 1, the precedent of a layout pool entry's `other`): the declaration gate and
// admission take `require_stationary: true` beside `check`, in a select step's scan, in a list
// reading's and in a select's scan paged by page-order evidence (ruling 5: the judgement exists
// there). They refuse it written `false` (`InvalidValue`), and written as anything else, `null`
// included (`InvalidType`), at `…/scan/end/require_stationary`. Another member of `end` is refused
// as every unknown member of `end` is refused, by admission's reader of `end`:
// `contained_task_program_invalid` for a select step, `contained_task_resource_reading_invalid`
// `read_members:read_list` for a list reading. Those are the codes a Runtime before `package` 30
// gives a package that writes `require_stationary` itself, whose reader of `end` knows `check` only;
// the revision table counts the member as 30 (the Lab's gate of this slice).
#[test]
fn list_c_the_declaration_takes_require_stationary_only_as_true() {
    admitted("a select", &pick_task(list_scan(Some(json!(true)))));
    admitted("a list reading", &path_task(list_scan(Some(json!(true)))));
    admitted("a paged select", &pick_task(paged_scan()));
    for (written, reason) in [
        (json!(false), ResourceDeclarationReason::InvalidValue),
        (json!("yes"), ResourceDeclarationReason::InvalidType),
        (Value::Null, ResourceDeclarationReason::InvalidType),
    ] {
        for (case, task, pointer) in [
            (
                "a select",
                pick_task(list_scan(Some(written.clone()))),
                "/operations/0/select/scan/end/require_stationary",
            ),
            (
                "a list reading",
                path_task(list_scan(Some(written.clone()))),
                "/operations/1/read_list/scan/end/require_stationary",
            ),
        ] {
            let error = refused(case, &task);
            assert_eq!(
                error.code().as_str(),
                SharedCode::ResourceDeclarationInvalid.as_str(),
                "{case}, {written}: {error}"
            );
            let issue = error
                .declaration_issue()
                .unwrap_or_else(|| panic!("{case}, {written}: the declaration issue"));
            assert_eq!(issue.field_path, pointer, "{case}, {written}");
            assert_eq!(issue.reason, reason, "{case}, {written}");
        }
    }
    let mut other = list_scan(None);
    other["end"]["stationary"] = json!(true);
    let error = refused(
        "another member of a select's end",
        &pick_task(other.clone()),
    );
    assert_eq!(error.code().as_str(), PROGRAM_INVALID, "{error}");
    let error = refused("another member of a list reading's end", &path_task(other));
    assert_eq!(error.code().as_str(), READING_INVALID, "{error}");
    assert_eq!(error.detail(), Some("read_members:read_list"));
}

// A run of the path on a device that shows home, then `list` (the list's viewports, one after each
// of the reading's moves), then done.
fn run_path(task: &PreparedContainedTask, list: &[Screen]) -> (RunResult, ScriptedRuntime) {
    let mut screens = vec![vec![scripted(page(HOME))]];
    screens.extend(list.iter().map(|screen| vec![scripted(*screen)]));
    screens.push(vec![scripted(page(DONE))]);
    let mut device = ScriptedRuntime::new(screens);
    let result = task.run(&mut device);
    (result, device)
}

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// Every `ListReading` record of a run: its observation time and its list.
fn lists(device: &ScriptedRuntime) -> Vec<(SystemTime, Result<ListReadingRows, ListReadingMiss>)> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ListReading {
                declaration,
                captured_at,
                list,
            } => {
                assert_eq!(declaration.operation_id, "read_members");
                assert_eq!(declaration.fact_key, LIST_KEY);
                Some((*captured_at, list.clone()))
            }
            _ => None,
        })
        .collect()
}

// The capture time of every frame a reading's end named (`ListReadingFrame`), in order.
fn end_frames(device: &ScriptedRuntime) -> Vec<SystemTime> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ListReadingFrame { captured_at } => Some(*captured_at),
            _ => None,
        })
        .collect()
}

// The rows of the business IDs `members`, at the identities' confidence (990).
fn rows(members: &[&str]) -> ListReadingRows {
    ListReadingRows {
        rows: members
            .iter()
            .map(|member| {
                BTreeMap::from([("id".to_owned(), FactScalar::String((*member).to_owned()))])
            })
            .collect(),
        confidence_milli: 990,
    }
}

// The run's normal end on done after the path's three steps.
fn completed() -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(DONE_PAGE.to_owned()),
        executed_steps: 3,
        selected_scheduling_outcome: None,
        designated_effect_caps: Vec::new(),
    }
}

// Test 2 (the case it is for) and test 7, the list reader, on a step plan's `read_list` step: the
// list's end mark shows at 24 px, in the middle, and again at its foot, 48 px. Without the member
// the reading's coverage is complete at 24 px after one move and the run records the first six
// cards as the whole list, `golf` missing (today's behaviour). With it the reading moves on to the
// foot and twice more, the device showing the foot each time, and records all seven cards, at 990,
// once, observed on the frame its end named (one `ListReadingFrame`, the second repeat's); the run
// is Completed on done after its three steps, its inputs `open`, the reading's four moves and
// `close`.
#[test]
fn list_c_a_list_reading_step_reads_past_an_end_mark_in_the_middle() {
    let before = admitted("without the member", &path_task(list_scan(None)));
    let (result, device) = run_path(&before, &[list_at(0, false), list_at(24, true)]);
    assert_eq!(
        result.unwrap_or_else(|error| panic!("the run ends normally, got {error:?}")),
        completed()
    );
    let ends = end_frames(&device);
    let [end_frame] = ends.as_slice() else {
        panic!("one end frame: {ends:?}");
    };
    assert_eq!(lists(&device), [(*end_frame, Ok(rows(&MEMBERS[..6])))]);
    assert_eq!(device.inputs().len(), 3, "{:?}", device.inputs());

    let task = admitted("with the member", &path_task(list_scan(Some(json!(true)))));
    let (result, device) = run_path(&task, &viewports(true, 2));
    assert_eq!(
        result.unwrap_or_else(|error| panic!("the run ends normally, got {error:?}")),
        completed()
    );
    let ends = end_frames(&device);
    let [end_frame] = ends.as_slice() else {
        panic!("one end frame: {ends:?}");
    };
    assert_eq!(lists(&device), [(*end_frame, Ok(rows(&MEMBERS)))]);
    assert_eq!(device.inputs().len(), 6, "{:?}", device.inputs());
}

// Test 9 (ruling 9), the list check's entry `dry_run_list_reading`: on the saved frames of a run
// that ends with the two repeats (the list's top, the look-alike, the foot and the foot twice more,
// each settled) it answers what the run records: the seven rows whole, the end check on every
// viewport but the first. On the frames a run without the member saves, which end at the first
// viewport whose end check passes (the top, 24 px and the foot), it answers what a run would have:
// the list had not stopped, and the frames end where the scan's next move needs one,
// `selection_scan_replay_frame_missing`.
#[test]
fn list_c_the_list_check_answers_as_the_run_on_the_same_frames() {
    let task = admitted("with the member", &path_task(list_scan(Some(json!(true)))));
    let answered = dry_run_list_reading(&task, "read_members", settled(&viewports(true, 2)))
        .unwrap_or_else(|error| panic!("the list, got {error}"));
    assert_eq!(answered.list, Ok(rows(&MEMBERS)));
    assert_eq!(
        answered
            .viewports
            .iter()
            .map(|viewport| (viewport.content_offset_px, viewport.end_check))
            .collect::<Vec<_>>(),
        [(0, false), (24, true), (48, true), (48, true), (48, true)]
    );
    let error = dry_run_list_reading(&task, "read_members", settled(&viewports(false, 0)))
        .expect_err("frames that end at the first end check");
    assert_eq!(error.code().as_str(), FRAME_MISSING, "{error}");
}

// Test 9 (ruling 9), the select's entry `dry_run_scan_decide`: on the saved frames that end with
// the two repeats it decides as a run: coverage complete over 0, 24, 48, 48 and 48 px, `selected`
// with `member-g` (`layout/cards#00`, position 6, fixed on viewport 2 at content row 82), nothing
// unknown. On the frames that end at the first end check, at the look-alike (the top and 24 px),
// it answers `selection_scan_replay_frame_missing`, where without the member the same frames give
// complete coverage over 0 and 24 px and the decision `empty`, golf unseen.
#[test]
fn list_c_the_select_dry_run_decides_as_the_run_on_the_same_frames() {
    let dry_run = |task: &PreparedContainedTask, frames: Vec<Frame>| {
        dry_run_scan_decide(
            task,
            "pick",
            frames,
            (&snapshot(), NOW_UNIX_MS),
            &task.default_run_values(),
            None,
        )
    };
    let task = admitted("with the member", &pick_task(list_scan(Some(json!(true)))));
    let decided = dry_run(&task, settled(&viewports(true, 2)))
        .unwrap_or_else(|error| panic!("a decision, got {error}"));
    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    assert_eq!(
        decided
            .scan
            .viewports
            .iter()
            .map(|viewport| viewport.content_offset_px)
            .collect::<Vec<_>>(),
        [0, 24, 48, 48, 48]
    );
    assert!(
        matches!(
            decided.decision.outcome,
            SelectionOutcome::Selected { count: 1 }
        ),
        "{:?}",
        decided.decision.outcome
    );
    assert_eq!(decided.decision.selected, ["layout/cards#00"]);
    assert_eq!(decided.unknown_check.unknown_instances, 0);
    let chosen = decided.chosen.expect("the chosen card");
    assert_eq!(
        (
            chosen.identity.as_str(),
            chosen.position_index,
            chosen.fixed_viewport,
            chosen.content_axis_px
        ),
        ("member-g", 6, 2, 82)
    );
    let look_alike = [list_at(0, false), list_at(24, true)];
    let error =
        dry_run(&task, settled(&look_alike)).expect_err("frames that end at the look-alike");
    assert_eq!(error.code().as_str(), FRAME_MISSING, "{error}");

    let before = admitted("without the member", &pick_task(list_scan(None)));
    let decided = dry_run(&before, settled(&look_alike))
        .unwrap_or_else(|error| panic!("a decision, got {error}"));
    assert_eq!(decided.scan.coverage, ScanCoverageState::Complete);
    assert_eq!(decided.decision.outcome, SelectionOutcome::Empty);
    assert_eq!(decided.chosen, None);
}
