// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-4 (`contracts/resource-readings.md`, "List readings"; register §1.291, the
// slice's named tests), the kernel's halves. A list reading's session runs over a fake source
// that hands out synthetic viewports: a vertical list of 100 x 80 px cards at a row pitch of
// 100 px in a 400 x 600 frame, whose readable band (y 60 to 260) shows two rows, so each
// advance of 100 px brings one new row. Every expected value comes from the contract: the rows
// in ascending byte order of their key, the lowest confidence, the three misses with their
// count, a scan failure at the stage `read_list`, and admission's refusals
// (`contained_task_resource_reading_invalid`, `<operation id>:<member>`). The packages are
// synthetic `neutral` packages; the ids are made up (`member.a` to `member.z`, the keys
// `inventory.list.example_kind_a` and `inventory.list.example_kind_b`).

use super::list_reading::{ListRead, at_read_list, prepare_list_readings};
use super::scan::{
    ScanDeclaration, ScanDrag, ScanDragMotion, ScanReadInstance, ScanReading, ScanSession,
    ScanSource,
};
use super::*;
use crate::{OperationRunCap, RunMaxRunsEnd};
use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateIdentitySource, CandidateRect,
    CandidateUnknownReason, FactScalar, FrameId,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition_pack::CandidateLayout;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Cursor, Write};
use std::time::Duration;
use zip::{ZipWriter, write::FileOptions};

const FRAME: (u32, u32) = (400, 600);
// The one column's left edge and the first row's top edge, in content coordinates.
const CROSS: i32 = 20;
const FIRST_ROW: i32 = 60;
const PITCH: i32 = 100;
const KEY_COLUMN: &str = "id";

// One card of a synthetic list: its row, its identity (Known or not), its signature colour and
// its rank, with the confidence each reading carries.
#[derive(Debug, Clone)]
struct Card {
    row: i32,
    identity: Option<String>,
    identity_confidence: i64,
    color: [u8; 3],
    rank: i64,
    rank_confidence: Option<i64>,
}

fn card(row: i32, identity: &str, color: [u8; 3], rank: i64) -> Card {
    Card {
        row,
        identity: Some(identity.to_owned()),
        identity_confidence: 990,
        color,
        rank,
        rank_confidence: None,
    }
}

fn digest(color: [u8; 3]) -> ColorDigest {
    ColorDigest::from_hex(
        ColorDigestGrid::new(1, 1).expect("grid"),
        &format!("{:02x}{:02x}{:02x}", color[0], color[1], color[2]),
    )
    .expect("digest")
}

fn features(card: &Card) -> CandidateFeatureMap {
    let identity = match &card.identity {
        Some(value) => CandidateFeature::Identity {
            value: value.clone(),
            variant: None,
            source: CandidateIdentitySource::OcrAlias,
            distance: 0,
            confidence: Some(card.identity_confidence),
        },
        // Out of the dictionary: the layout rejects an unknown identity.
        None => CandidateFeature::Unknown {
            reason: CandidateUnknownReason::OutOfDomain,
            source: Some(CandidateIdentitySource::OcrAlias),
        },
    };
    BTreeMap::from([
        ("member".to_owned(), identity),
        (
            "rank".to_owned(),
            CandidateFeature::Integer {
                value: card.rank,
                confidence: card.rank_confidence,
            },
        ),
    ])
}

// One captured frame of the list.
#[derive(Debug, Clone)]
struct View {
    instances: Vec<ScanReadInstance>,
    start: bool,
    end: bool,
}

// The list at content offset `offset` in the band of `band_end`: every card that lies in the
// frame, actionable inside the readable band (y 60 to `band_end`), with its features only then.
fn view_in(list: &[Card], offset: i32, band_end: i32, (start, end): (bool, bool)) -> View {
    let instances = list
        .iter()
        .filter_map(|card| {
            let y = FIRST_ROW + PITCH * card.row - offset;
            (y >= 0 && y + 80 <= 600).then(|| {
                let actionable = y >= 60 && y + 80 <= band_end;
                ScanReadInstance {
                    rect: CandidateRect {
                        x: CROSS,
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
                }
            })
        })
        .collect();
    View {
        instances,
        start,
        end,
    }
}

// The list at `offset` in the two-row band.
fn view(list: &[Card], offset: i32, start: bool, end: bool) -> View {
    view_in(list, offset, 260, (start, end))
}

// What the fake hands out: the dispatched frame, then its captures in order.
struct Script {
    dispatched: Vec<CandidateRect>,
    captures: VecDeque<View>,
}

impl Script {
    // `Pin`: the dispatched frame, and one equal capture, so it settles at once.
    fn starting_with(first: View) -> Self {
        Self {
            dispatched: first
                .instances
                .iter()
                .map(|instance| instance.rect)
                .collect(),
            captures: VecDeque::from([first]),
        }
    }

    // After a move: the frame captured twice, so it settles at the second.
    fn then(mut self, moved: View) -> Self {
        self.captures.push_back(moved.clone());
        self.captures.push_back(moved);
        self
    }
}

fn frame_id(capture: u32) -> FrameId {
    serde_json::from_value(json!(format!("frame_{capture:032x}"))).expect("frame id")
}

// A scan source that replays a script; every capture costs the settle interval and is the next
// frame. It keeps every move and the capture on which the end check was reported passed, the
// frame a list's observation names.
struct Fake {
    script: Script,
    current: Option<View>,
    captures: u32,
    clock: Duration,
    drags: Vec<ScanDragMotion>,
    end_passed_on: Vec<u32>,
}

impl Fake {
    fn new(script: Script) -> Self {
        Self {
            script,
            current: None,
            captures: 0,
            clock: Duration::ZERO,
            drags: Vec::new(),
            end_passed_on: Vec::new(),
        }
    }
}

impl ScanSource for Fake {
    type Error = String;

    fn elapsed(&mut self) -> Duration {
        self.clock
    }

    fn dispatched(&mut self) -> Result<Vec<CandidateRect>, ContainedTaskRunError<String>> {
        Ok(self.script.dispatched.clone())
    }

    fn capture(
        &mut self,
        interval: Duration,
    ) -> Result<Vec<CandidateRect>, ContainedTaskRunError<String>> {
        self.clock += interval;
        let view = self
            .script
            .captures
            .pop_front()
            .ok_or_else(|| ContainedTaskRunError::Boundary("no further capture".to_owned()))?;
        self.captures += 1;
        let anchors = view
            .instances
            .iter()
            .map(|instance| instance.rect)
            .collect();
        self.current = Some(view);
        Ok(anchors)
    }

    fn viewport(&mut self) -> Result<ScanReading, ContainedTaskRunError<String>> {
        let view = self
            .current
            .clone()
            .ok_or_else(|| ContainedTaskRunError::Boundary("no capture yet".to_owned()))?;
        Ok(ScanReading {
            instances: view.instances,
            start_check: view.start,
            end_check: view.end,
            frame_id: Some(frame_id(self.captures)),
        })
    }

    fn drag(&mut self, drag: ScanDrag) -> Result<(), ContainedTaskRunError<String>> {
        self.drags.push(drag.motion);
        Ok(())
    }

    fn end_passed(&mut self) -> Result<(), ContainedTaskRunError<String>> {
        self.end_passed_on.push(self.captures);
        Ok(())
    }
}

// The scan of the two-row band: one column, an advance of 100 to 120 px, a reposition of at most
// 180 px, the tracking area y 40 to 280; `patch` changes it.
fn declaration(patch: impl FnOnce(&mut Value)) -> ScanDeclaration {
    let mut value = json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 150, "y": 200, "width": 100, "height": 10},
                    "to": {"x": 150, "y": 90, "width": 100, "height": 10},
                    "duration_ms": 500, "min_content_step_px": 100, "max_content_step_px": 120},
        "reposition": {"kind": "drag",
                       "from": {"x": 150, "y": 220, "width": 100, "height": 10},
                       "to": {"x": 150, "y": 70, "width": 100, "height": 10},
                       "duration_ms": 400, "max_content_step_px": 180},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 40, "width": 400, "height": 240},
                  "edge_margin_px": 12,
                  "signature": {"rect": {"x": 0, "y": 0, "width": 20, "height": 20},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 4,
                  "min_overlap_instances": 1,
                  "spacing": {"max_axis_gap_px": 120, "columns": 1, "max_trailing_gap_px": 200,
                              "max_cross_lead_px": 40, "max_cross_gap_px": 140}},
        "start": {"check": "check/list_start", "rewind": false},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 32, "max_reposition_viewports": 8, "max_ms": 240000}
    });
    patch(&mut value);
    ScanDeclaration::for_test(value)
}

// The list's layout: the identity `member` and the integer `rank` of each card, over the
// readable band y 60 to `band_end`.
fn layout_in(identities: &[String], band_end: i32) -> CandidateLayout {
    serde_json::from_value(json!({
        "id": "layout/members", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 100, "height": 80},
        "click": {"x": 10, "y": 10, "width": 80, "height": 40},
        "readable_band": {"x": 0, "y": 60, "width": 400, "height": band_end - 60},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 50},
             "identity": {
                 "entries": identities.iter().map(|id| json!({"id": id, "aliases": [id]}))
                     .collect::<Vec<_>>(),
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "rank", "value": "ocr_integer", "target": "card/rank",
             "offset": {"x": 60, "y": 0}, "integer": {"min": 0, "max": 999}}
        ]
    }))
    .expect("layout")
}

fn layout(identities: &[String]) -> CandidateLayout {
    layout_in(identities, 260)
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

// The one declared column: `rank`, the reading of the feature `rank`.
fn rank_column() -> Vec<(String, String)> {
    vec![("rank".to_owned(), "rank".to_owned())]
}

struct Read {
    list: Result<ListRead, ContainedTaskRunError<String>>,
    fake: Fake,
}

// One list reading of `layout` with `scan` over `script`.
fn read(scan: &ScanDeclaration, layout: &CandidateLayout, script: Script) -> Read {
    let columns = rank_column();
    let mut session = ScanSession::for_list(
        scan,
        layout,
        columns.iter().map(|(_, feature)| feature.clone()).collect(),
        FRAME,
        Duration::from_secs(300),
    )
    .expect("a list reading's session");
    let mut fake = Fake::new(script);
    let list = session.read_list(&mut fake, KEY_COLUMN, &columns);
    Read { list, fake }
}

fn row(id: &str, rank: i64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        (KEY_COLUMN.to_owned(), FactScalar::String(id.to_owned())),
        ("rank".to_owned(), FactScalar::Integer(rank)),
    ])
}

// The three cards `member.c`, `member.a` and `member.b`, in that order down the list, each in a
// colour far from the others'.
fn three_cards() -> Vec<Card> {
    let mut cards = vec![
        card(0, "member.c", [200, 10, 10], 5),
        card(1, "member.a", [10, 200, 10], 7),
        card(2, "member.b", [10, 10, 200], 9),
    ];
    cards[0].identity_confidence = 980;
    cards[1].identity_confidence = 990;
    cards[2].identity_confidence = 985;
    cards[0].rank_confidence = Some(960);
    cards[2].rank_confidence = Some(955);
    cards
}

// The three viewports of `list`: offsets 0, 100 and 200, the end check passing on the last.
fn three_viewports(list: &[Card]) -> Script {
    Script::starting_with(view(list, 0, true, false))
        .then(view(list, 100, false, false))
        .then(view(list, 200, false, true))
}

// S168-4 test 1, the session: a scan over three viewports reads `member.c`, `member.a` and
// `member.b` with a `rank` column. The list is whole: rows `member.a`, `member.b`, `member.c` in
// that order (ascending byte order of the key, not the order on the screen), each with its
// `rank`, at the lowest confidence of the features the rows read (`member.b`'s rank, 955). The
// end check passed on the third viewport, the fifth capture, which the session reported once:
// the frame the list is observed on. No capture follows it.
#[test]
fn list_reading_publishes_whole_list() {
    let list = three_cards();
    let layout = layout(&ids(&["member.a", "member.b", "member.c"]));
    let scan = declaration(|_| {});
    let read = read(&scan, &layout, three_viewports(&list));
    match read.list {
        Ok(ListRead::Whole(whole)) => {
            assert_eq!(
                whole.rows,
                vec![row("member.a", 7), row("member.b", 9), row("member.c", 5)]
            );
            assert_eq!(whole.confidence_milli, 955);
        }
        other => panic!("a whole list, got {other:?}"),
    }
    assert_eq!(
        read.fake.drags,
        [ScanDragMotion::Advance, ScanDragMotion::Advance]
    );
    assert_eq!(read.fake.captures, 5);
    assert_eq!(read.fake.end_passed_on, [5]);
    assert!(read.fake.script.captures.is_empty());
}

// S168-4 test 2, the session: no anchor matches and both checks pass on the first viewport. The
// list is known and empty: no row, confidence 1000 (its evidence is the two checks), and no move.
#[test]
fn list_reading_empty_list_is_known() {
    let layout = layout(&ids(&["member.a"]));
    let scan = declaration(|_| {});
    let read = read(
        &scan,
        &layout,
        Script::starting_with(view(&[], 0, true, true)),
    );
    match read.list {
        Ok(ListRead::Whole(whole)) => {
            assert!(whole.rows.is_empty());
            assert_eq!(whole.confidence_milli, 1000);
        }
        other => panic!("an empty list, got {other:?}"),
    }
    assert!(read.fake.drags.is_empty(), "no input");
    assert_eq!(read.fake.end_passed_on, [1]);
}

fn missed(read: &Read) -> ListReadingMiss {
    match &read.list {
        Ok(ListRead::Missed(miss)) => *miss,
        other => panic!("a miss, got {other:?}"),
    }
}

// The grid of the 257th position: three columns at a row pitch of 100 px over the full band
// (y 60 to 540, five rows), `rows` rows of distinct identities, each column's colour of its own.
fn grid(rows: usize) -> Vec<(i32, Card)> {
    const CROSSES: [i32; 3] = [20, 140, 260];
    const COLORS: [[u8; 3]; 3] = [[200, 10, 10], [10, 200, 10], [10, 10, 200]];
    (0..rows * 3)
        .map(|index| {
            let mut card = card(
                i32::try_from(index / 3).expect("row"),
                &format!("member.{index:03}"),
                COLORS[index % 3],
                1,
            );
            card.color[2] = card.color[2].wrapping_add(u8::try_from(index % 7).expect("shade"));
            (CROSSES[index % 3], card)
        })
        .collect()
}

fn grid_view(grid: &[(i32, Card)], offset: i32) -> View {
    let instances = grid
        .iter()
        .filter_map(|(cross, card)| {
            let y = FIRST_ROW + PITCH * card.row - offset;
            (y >= 0 && y + 80 <= 600).then(|| {
                let actionable = y >= 60 && y + 80 <= 540;
                ScanReadInstance {
                    rect: CandidateRect {
                        x: *cross,
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
                }
            })
        })
        .collect();
    View {
        instances,
        start: offset == 0,
        end: false,
    }
}

// S168-4 test 3, the session: a list that cannot be read whole gives no reading, never a part of
// one. A position whose identity is out of the dictionary on every read is
// `list_position_unread` (count 1); two positions that read one business ID are
// `list_duplicate_key` (count 1, the repeated key); and a list that shows a 257th position is
// `list_rows_exceeded` (count 257), the scan stopping on the viewport that read it: 15 positions
// on the first viewport and 9 more on each advance of 300 px reach 258 on the 28th viewport, after
// 27 advances, and no capture follows.
#[test]
fn list_reading_miss_publishes_nothing() {
    let scan = declaration(|_| {});
    let layout = layout(&ids(&["member.a", "member.b", "member.c"]));

    let mut unread = three_cards();
    unread[1].identity = None;
    let read_unread = read(&scan, &layout, three_viewports(&unread));
    let miss = missed(&read_unread);
    assert_eq!(miss.problem, ResourceReadingProblem::ListPositionUnread);
    assert_eq!(miss.count, 1);

    let mut repeated = three_cards();
    repeated[2].identity = Some("member.a".to_owned());
    let read_repeated = read(&scan, &layout, three_viewports(&repeated));
    let miss = missed(&read_repeated);
    assert_eq!(miss.problem, ResourceReadingProblem::ListDuplicateKey);
    assert_eq!(miss.count, 1);

    // The full band of the SEL-L3 grid: three columns, five rows a viewport, 300 px a move.
    let long = grid(90);
    let identities = long
        .iter()
        .filter_map(|(_, card)| card.identity.clone())
        .collect::<Vec<_>>();
    let wide = declaration(|value| {
        value["advance"] = json!({"kind": "drag",
            "from": {"x": 150, "y": 450, "width": 100, "height": 20},
            "to": {"x": 150, "y": 150, "width": 100, "height": 20},
            "duration_ms": 500, "min_content_step_px": 180, "max_content_step_px": 400});
        value["reposition"] = json!({"kind": "drag",
            "from": {"x": 150, "y": 500, "width": 100, "height": 20},
            "to": {"x": 150, "y": 80, "width": 100, "height": 20},
            "duration_ms": 400, "max_content_step_px": 480});
        value["track"]["area"] = json!({"x": 0, "y": 40, "width": 400, "height": 520});
        value["track"]["min_overlap_instances"] = json!(2);
        value["track"]["spacing"]["columns"] = json!(3);
    });
    let wide_layout = layout_in(&identities, 540);
    let mut script = Script::starting_with(grid_view(&long, 0));
    for advance in 1..=27 {
        script = script.then(grid_view(&long, 300 * advance));
    }
    let read_long = read(&wide, &wide_layout, script);
    let miss = missed(&read_long);
    assert_eq!(miss.problem, ResourceReadingProblem::ListRowsExceeded);
    assert_eq!(miss.count, 257);
    assert_eq!(read_long.fake.drags.len(), 27);
    assert_eq!(read_long.fake.captures, 1 + 2 * 27);
    assert!(read_long.fake.script.captures.is_empty());
    assert!(read_long.fake.end_passed_on.is_empty());

    // Each miss is the warning `contained_task_resource_reading_unresolved` with the step's id,
    // the list's key, the problem and its count.
    let warning = miss.outcome(&ListReadingDeclaration {
        operation_id: "read_members".to_owned(),
        fact_key: "inventory.list.example_kind_a".to_owned(),
        valid_for_ms: 86_400_000,
    });
    assert_eq!(
        warning.code().as_str(),
        ExecutionKernelCode::ContainedTaskResourceReadingUnresolved.as_str()
    );
    assert_eq!(
        warning.category(),
        actingcommand_contract::outcome::Category::Warning
    );
    assert_eq!(warning.value("reading_id"), Some(&json!("read_members")));
    assert_eq!(
        warning.value("fact_key"),
        Some(&json!("inventory.list.example_kind_a"))
    );
    assert_eq!(
        warning.value("reading_problem"),
        Some(&json!("list_rows_exceeded"))
    );
    assert_eq!(warning.value("count"), Some(&json!(257)));
}

// S168-4 test 4, the session and the step's stage: the second viewport lies 30 px on, outside
// the advance's window (100 to 120 px) and not where it stood: the scan fails
// `selection_scan_alignment_rejected` (`unexpected_displacement`), with no list. The step ends
// the run with that code at the stage `read_list`.
#[test]
fn list_reading_failed_scan_fails_run() {
    let list = three_cards();
    let layout = layout(&ids(&["member.a", "member.b", "member.c"]));
    let scan = declaration(|_| {});
    let script =
        Script::starting_with(view(&list, 0, true, false)).then(view(&list, 30, false, false));
    let read = read(&scan, &layout, script);
    let error = match read.list {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("a scan failure, got {other:?}"),
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionScanAlignmentRejected.as_str()
    );
    assert_eq!(
        error.outcome().value("scan_alignment_failure"),
        Some(&json!("unexpected_displacement"))
    );
    assert!(read.fake.end_passed_on.is_empty());
    let staged = match at_read_list(ContainedTaskRunError::<String>::Task(error)) {
        ContainedTaskRunError::Task(error) => error,
        other => panic!("the task's failure, got {other:?}"),
    };
    assert_eq!(
        staged.code().as_str(),
        ExecutionKernelCode::SelectionScanAlignmentRejected.as_str()
    );
    assert_eq!(staged.outcome().value("stage"), Some(&json!("read_list")));
    // The runtime's own failures keep their form.
    assert!(matches!(
        at_read_list(ContainedTaskRunError::Boundary("device".to_owned())),
        ContainedTaskRunError::Boundary(_)
    ));
}

// ---- Admission: a synthetic package whose list page shows cards, read by one `read_list` step
// `read_members`, and left by the tap `leave` to the detail page.

const WIDTH: u32 = 400;
const HEIGHT: u32 = 600;
const TASK_TIMEOUT_MS: u64 = 300_000;

fn package_scan() -> Value {
    json!({
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
    })
}

fn card_layout() -> Value {
    let entries = ["member.a", "member.b", "member.c"]
        .map(|id| json!({"id": id, "aliases": [id.replace('.', " ")]}));
    json!({
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
                 "entries": entries,
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 80, "y": 0}}
        ]
    })
}

fn read_step() -> Value {
    json!({
        "id": "read_members", "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": {"layout_id": "layout/cards", "key_feature": "member", "key_column": "id",
                      "columns": [{"column": "open", "feature": "open"}],
                      "fact_key": "inventory.list.example_kind_a", "valid_for_ms": 86_400_000,
                      "scan": package_scan()}
    })
}

fn leave_step() -> Value {
    json!({"id": "leave", "from": "list", "to": "detail",
           "click": {"kind": "point", "x": 200, "y": 300},
           "unguarded_trusted_coordinate": true, "retryable": false})
}

// The admitted task: `read_members` on the list page, then `leave` to the detail page, the
// terminal its one mapping names.
fn task() -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "detail",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "listed", "effect": "no_designated_effect",
             "terminal_pages": ["detail"]}
        ]},
        "candidate_layouts": [card_layout()],
        "operations": [read_step(), leave_step()]
    })
}

fn color(id: &str, x: i32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
           "expected": expected})
}

// A textured template image.
fn template_png() -> Vec<u8> {
    let pixels = (0..16_u32 * 12)
        .flat_map(|index| {
            let [red, green, blue, _] = index.wrapping_mul(2_654_435_761).to_le_bytes();
            [red, green, blue]
        })
        .collect::<Vec<u8>>();
    Frame::from_pixels(
        16,
        12,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("template frame")
    .png_for_artifact()
    .expect("template PNG")
}

// The package of `task` with `control_patch` applied to its control: the recognition pack holds
// the task's candidate layouts and their targets.
fn package_with(task: &Value, control_patch: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.list.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 10_000,
        "timeout_ms": TASK_TIMEOUT_MS
    });
    control_patch(&mut control);
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            color("page/home", 0, [255, 255, 0]),
            color("page/list", 0, [255, 0, 0]),
            color("page/detail", 0, [0, 0, 255]),
            color("check/list_start", 1, [0, 255, 0]),
            color("check/list_end", 2, [0, 0, 255]),
            color("guard/list", 3, [0, 255, 0]),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.9},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 100, "height": 20},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["member a"], "case_sensitive": false, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)},
            {"type": "color", "id": "card/open",
             "region": {"x": 0, "y": 0, "width": 20, "height": 20}, "expected": [0, 255, 0]}
        ],
        "candidate_layouts": task["candidate_layouts"].clone()
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": ["home", "list", "detail"].map(|page| json!({
            "id": format!("neutral/{page}"), "required": [format!("page/{page}")],
            "optional": [], "forbidden": []
        }))
    });
    let bytes = |value: &Value| serde_json::to_vec(value).expect("JSON bytes");
    let entries = [
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        ("resources/assets/card.png", template_png()),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn admit(task: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package_with(task, |_| {}))
}

// Admission's own checks of a program, as `prepare_bundle` runs them after the declaration gate:
// the members of every `read_list` step, then its references and its scan, against the admitted
// base package's control and recognition pack.
fn admission_refusal(base: &PreparedContainedTask, task: &Value) -> ContainedTaskError {
    let mut program: TaskProgram = serde_json::from_value(task.clone()).expect("task program");
    match program.validate_list_reading_members() {
        Err(error) => error,
        Ok(()) => prepare_list_readings(&mut program, &base.control, &base.evaluator)
            .expect_err("admission refuses the step"),
    }
}

fn assert_reading_invalid(case: &str, error: &ContainedTaskError, detail: &str) {
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskResourceReadingInvalid.as_str(),
        "{case}: {error}"
    );
    assert_eq!(error.detail(), Some(detail), "{case}");
}

// `task` with the read step changed by `patch`.
fn with_read(patch: impl FnOnce(&mut Value)) -> Value {
    let mut task = task();
    patch(&mut task["operations"][0]);
    task
}

// S168-4 test 5, admission's half (the declaration gate's half is the pack-containment test of
// the same name): the base package is admitted, and the step is prepared with its layout, its
// scan's time limit and the once-per-run cap: one dispatch, then the page's next candidate, as
// the static walk follows `leave` after it; a page whose only operation is the list reading
// leaves the run no way on and is refused. Each rule of the declaration table is refused by
// admission's own checks with `contained_task_resource_reading_invalid` and
// `<operation id>:<member>`; a scan whose numbers break a select step's static check is refused
// that way through the whole admission, after the gate; and a `linear_steps` task refuses the
// step with `contained_task_linear_invalid`, `reason=read_list`.
#[test]
fn list_reading_declaration_refusals() {
    let base = admit(&task()).expect("the base package is admitted");
    let [read_members, _] = base.program.operations.as_slice() else {
        panic!("two operations");
    };
    assert!(read_members.prepared_list.is_some(), "the step is prepared");
    assert_eq!(read_members.scan_max_ms(), Some(240_000));
    assert!(base.has_list_readings());
    assert_eq!(
        read_members.run_cap(),
        Some(OperationRunCap {
            operation_id: "read_members".to_owned(),
            max_runs: 1,
            on_max_runs: RunMaxRunsEnd::NextCandidate,
        })
    );
    assert!(read_members.may_be_run_capped());
    let mut stranded = task();
    stranded["operations"] = json!([read_step()]);
    let error = match admit(&stranded) {
        Ok(_) => panic!("a page left with no operation after the list is refused"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskOutcomeDeclarationIncomplete.as_str(),
        "{error}"
    );

    let mut older = task();
    older["schema_version"] = json!("0.8");
    let mut unscheduled = task();
    unscheduled
        .as_object_mut()
        .expect("task")
        .remove("scheduling_outcome");
    let mut seventeen = task();
    let mut operations = (1..=17)
        .map(|index| {
            let mut step = read_step();
            step["id"] = json!(format!("read_{index:02}"));
            step["read_list"]["fact_key"] = json!(format!("inventory.list.example_{index:02}"));
            step
        })
        .collect::<Vec<_>>();
    operations.push(leave_step());
    seventeen["operations"] = json!(operations);
    let mut second_list = task();
    let mut again = read_step();
    again["id"] = json!("read_again");
    second_list["operations"] = json!([read_step(), again, leave_step()]);

    let member_cases = [
        ("an older schema", older, "read_members:read_list"),
        (
            "no scheduling outcome",
            unscheduled,
            "read_members:scheduling_outcome",
        ),
        ("a 17th read_list step", seventeen, "read_17:operations"),
        (
            "another effect",
            with_read(|step| step["click"] = json!({"kind": "point", "x": 1, "y": 1})),
            "read_members:click",
        ),
        (
            "a retry count",
            with_read(|step| step["max_attempts"] = json!(2)),
            "read_members:max_attempts",
        ),
        (
            "a retry interval",
            with_read(|step| step["retry_interval_ms"] = json!(100)),
            "read_members:retry_interval_ms",
        ),
        (
            "a recovery",
            with_read(|step| step["on_error"] = json!("return_home")),
            "read_members:on_error",
        ),
        (
            "a trusted coordinate",
            with_read(|step| step["unguarded_trusted_coordinate"] = json!(true)),
            "read_members:unguarded_trusted_coordinate",
        ),
        (
            "a confirmation",
            with_read(|step| step["expect_after"] = json!({"page_id": "list"})),
            "read_members:expect_after",
        ),
        (
            "a post delay",
            with_read(|step| step["post_delay_ms"] = json!(100)),
            "read_members:post_delay_ms",
        ),
        (
            "an input recheck",
            with_read(|step| step["input_recheck"] = json!({"kind": "fresh_frame"})),
            "read_members:input_recheck",
        ),
        (
            "an observation window",
            with_read(|step| {
                step["observation_window"] = json!({"until": "window_end", "window_ms": 1000});
            }),
            "read_members:observation_window",
        ),
        (
            "a guard refusal",
            with_read(|step| step["on_guard_refused"] = json!("next_candidate")),
            "read_members:on_guard_refused",
        ),
        (
            "a cap of its own",
            with_read(|step| {
                step["max_runs"] = json!(1);
                step["on_max_runs"] = json!("next_candidate");
            }),
            "read_members:max_runs",
        ),
        (
            "an intermediate state",
            with_read(|step| {
                step["transition"] = json!({"kind": "window", "min_ms": 1, "max_ms": 2});
            }),
            "read_members:transition",
        ),
        (
            "an optional page",
            with_read(|step| step["optional"] = json!({"settle_ms": 10})),
            "read_members:optional",
        ),
        (
            "a retry",
            with_read(|step| step["retryable"] = json!(true)),
            "read_members:retryable",
        ),
        (
            "no guard",
            with_read(|step| {
                step.as_object_mut().expect("step").remove("guard");
            }),
            "read_members:guard",
        ),
        (
            "another destination",
            with_read(|step| step["to"] = json!("detail")),
            "read_members:to",
        ),
        (
            "no scan",
            with_read(|step| {
                step["read_list"]
                    .as_object_mut()
                    .expect("read_list")
                    .remove("scan");
            }),
            "read_members:read_list",
        ),
        (
            "another layout",
            with_read(|step| step["read_list"]["layout_id"] = json!("layout/other")),
            "read_members:read_list/layout_id",
        ),
        (
            "a key feature that is no identity",
            with_read(|step| step["read_list"]["key_feature"] = json!("open")),
            "read_members:read_list/key_feature",
        ),
        (
            "a key column that is no token",
            with_read(|step| step["read_list"]["key_column"] = json!("Id")),
            "read_members:read_list/key_column",
        ),
        (
            "a column named as the key column",
            with_read(|step| step["read_list"]["columns"][0]["column"] = json!("id")),
            "read_members:read_list/columns/0/column",
        ),
        (
            "a column of the key feature",
            with_read(|step| step["read_list"]["columns"][0]["feature"] = json!("member")),
            "read_members:read_list/columns/0/feature",
        ),
        (
            "64 columns",
            with_read(|step| {
                step["read_list"]["columns"] = json!(
                    (0..64)
                        .map(|index| json!({"column": format!("c{index}"), "feature": "open"}))
                        .collect::<Vec<_>>()
                );
            }),
            "read_members:read_list/columns",
        ),
        (
            "a key outside the prefix",
            with_read(|step| step["read_list"]["fact_key"] = json!("inventory.example_members")),
            "read_members:read_list/fact_key",
        ),
        (
            "a key another list writes",
            second_list,
            "read_again:read_list/fact_key",
        ),
        (
            "no lifetime",
            with_read(|step| step["read_list"]["valid_for_ms"] = json!(0)),
            "read_members:read_list/valid_for_ms",
        ),
        (
            "an early stop",
            with_read(|step| step["read_list"]["scan"]["early_stop"] = json!("upper_bound")),
            "read_members:read_list/scan/early_stop",
        ),
        (
            "a select step's static check",
            with_read(|step| step["read_list"]["scan"]["settle"]["timeout_ms"] = json!(100)),
            "read_members:read_list/scan settle=invalid",
        ),
    ];
    for (case, broken, detail) in member_cases {
        assert_reading_invalid(case, &admission_refusal(&base, &broken), detail);
    }

    // Through the whole admission: the gate passes the scan's numbers, admission refuses them.
    let refused = match admit(&with_read(|step| {
        step["read_list"]["scan"]["settle"]["timeout_ms"] = json!(100);
    })) {
        Ok(_) => panic!("a scan that breaks a static check is refused"),
        Err(error) => error,
    };
    assert_reading_invalid(
        "the whole admission",
        &refused,
        "read_members:read_list/scan settle=invalid",
    );

    // A step plan has no list reading.
    let mut linear = task();
    linear["entry_page"] = json!("list");
    linear["max_steps"] = json!(2);
    let refused = match PreparedContainedTask::load(
        "neutral.instance",
        &package_with(&linear, |control| {
            control["execution_mode"] = json!("linear_steps");
            control["max_steps"] = json!(2);
        }),
    ) {
        Ok(_) => panic!("a step plan with a list reading is refused"),
        Err(error) => error,
    };
    assert_eq!(
        refused.code().as_str(),
        ExecutionKernelCode::ContainedTaskLinearInvalid.as_str(),
        "{refused}"
    );
    assert_eq!(
        refused.detail(),
        Some("reason=read_list operation=read_members")
    );
}

fn mark(id: &str, fact_key: &str, capacity: Option<&str>) -> Value {
    let mut mark = json!({
        "id": id, "fact_key": fact_key, "page_id": "list", "target_id": "card/name",
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 999,
                  "format": "current_capacity"},
        "minimum_confidence_milli": 900, "valid_for_ms": 60_000
    });
    if let Some(capacity) = capacity {
        mark["capacity_fact_key"] = json!(capacity);
    }
    mark
}

// S168-4 test 10, admission's half (the gate's half is the pack-containment test of the same
// name): a mark whose `fact_key`, and a `current_capacity` mark whose `capacity_fact_key`, lies
// under `inventory.list.` is refused by admission's own mark rules with
// `contained_task_resource_reading_invalid` and `<id>:<reason>`
// (`resource_reading_fact_key_invalid`, `resource_reading_capacity_fact_key_invalid`); a mark
// under `inventory.` outside the prefix is still admitted.
#[test]
fn list_reading_mark_key_reserved() {
    let program = |marks: Vec<Value>| {
        let mut task = task();
        task["resource_readings"] = json!(marks);
        serde_json::from_value::<TaskProgram>(task).expect("task program")
    };
    let refused = program(vec![mark("k1", "inventory.list.example_kind_a", None)])
        .validate_reading_marks()
        .expect_err("a mark under the prefix is refused");
    assert_reading_invalid("fact_key", &refused, "k1:resource_reading_fact_key_invalid");
    let refused = program(vec![mark(
        "k2",
        "inventory.example_count",
        Some("inventory.list.example_kind_b"),
    )])
    .validate_reading_marks()
    .expect_err("a capacity under the prefix is refused");
    assert_reading_invalid(
        "capacity_fact_key",
        &refused,
        "k2:resource_reading_capacity_fact_key_invalid",
    );
    program(vec![mark(
        "k3",
        "inventory.example_count",
        Some("inventory.example_capacity"),
    )])
    .validate_reading_marks()
    .expect("a mark under inventory. outside the prefix is admitted");
    // The prefix is its own: a key that only begins like it is no list's.
    program(vec![mark("k4", "inventory.listing_count", None)])
        .validate_reading_marks()
        .expect("inventory.listing_count is not under inventory.list.");
}
