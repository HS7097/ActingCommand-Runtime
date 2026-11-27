// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-A3 (register §1.534; contracts/resource-readings.md, "List readings", "A list
// of the wanted keys only" and "Offline"; the coordinator's rulings 1, 2 and 5 in the work order of
// board #503): the dry run of a list reading answers every position its scan read: where it
// stands, the reads of its name with what the identity mapping says of each (the text, its
// confidence, the mark, why it names no key, the nearest wanted key and alias, the distance and
// the limit it had to exceed), and what it gives the list (its known key; passed over; unread with
// a cause of the dry run's own, which tells a column that changed, `contradicted`, from a key on
// one viewport passed over on another, `partly_passed_over`); and, for the list, the wanted keys a
// card showed and those none showed, the counts and whether it is whole.
//
// Every expected value comes from those rulings and the package values below, never from the code.
// The fixture is LIST-A's (`gate_503_list_a`), defined here again: a synthetic `neutral` step plan
// (`open`, `read_members`, `close`) admitted by the kernel's whole admission, its frames 64 x 60,
// cards in one column at content y 10 + 12 k, each a noise marker (the anchor) at x 4, a signature
// patch, a name patch a scripted OCR provider reads the name (its red level) and the confidence
// (its green level, in hundredths) from, and here a flag patch below the signature and the name
// (x 12 to 20, rows 6 and 7 of the card) that a colour target reads green or not (the column `flag`
// of the package that declares it). The wanted keys are `member-a`,
// `member-b` and `member-c`, whose aliases are `alpha`, `bravo` and `charlie`, read within 1 at a
// margin of 1 and a floor of 800, so the limit is 2. `zulu` is 5 from `alpha` and from `bravo`
// (the nearest is the first entry), `cxxxxxe` (a misreading of `charlie`, five letters changed)
// 5 from `charlie` and 7 from the others, `alpzz` 2 from `alpha`. The dry run takes the settled
// frames v0, v0, v1, v1.

use super::*;
use actingcommand_contract::{CandidateUnknownReason, FactScalar};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    CandidateNameMark, CandidateNameReading, CandidateNearestName, CandidateUnreadCause,
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 60;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const VALID_FOR_MS: u64 = 86_400_000;
// The names the provider reads, by index; the first three are the wanted keys' aliases.
const NAMES: [&str; 7] = [
    "alpha", "bravo", "charlie", "zulu", "yankee", "alpzz", "cxxxxxe",
];
const MEMBERS: [&str; 3] = ["member-a", "member-b", "member-c"];
const ALPHA: usize = 0;
const BRAVO: usize = 1;
const CHARLIE: usize = 2;
const ZULU: usize = 3;
const YANKEE: usize = 4;
const NEAR_ALPHA: usize = 5;
const MISREAD_CHARLIE: usize = 6;
// The confidence a card's name is read at, in hundredths, above the floor of 800; as milli.
const SURE: u8 = 99;
const SURE_MILLI: u16 = 990;
// `max_distance + minimum_margin`.
const LIMIT: u16 = 2;

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];

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

// Card k's signature patch: a colour of its own.
fn signature(slot: u32) -> [u8; 3] {
    let red = u8::try_from(30 + 36 * slot).expect("level");
    [red, 220 - red / 2, 90]
}

// The red level the name `name` (an index of `NAMES`) is read from.
fn name_level(name: usize) -> u8 {
    u8::try_from(40 + 30 * name).expect("name level")
}

// One card: its place in the list, the name its patch reads, the read's confidence in hundredths,
// and whether its flag patch is green.
#[derive(Debug, Clone, Copy)]
struct Card {
    slot: u32,
    name: usize,
    confidence: u8,
    flag: bool,
}

// Card `slot` reading `name` with confidence, its flag green.
fn card(slot: u32, name: usize) -> Card {
    Card {
        slot,
        name,
        confidence: SURE,
        flag: true,
    }
}

// One screen: its page's colour; on the list, whether its start and end marks show, the content
// moved up by `offset` pixels, and its cards.
#[derive(Debug, Clone)]
struct Screen {
    page: [u8; 3],
    start: bool,
    end: bool,
    offset: u32,
    cards: Vec<Card>,
}

// The list at content offset `offset` showing `cards`: its start mark at offset 0, its end mark
// where `end`.
fn list_at(offset: u32, end: bool, cards: &[Card]) -> Screen {
    Screen {
        page: LIST,
        start: offset == 0,
        end,
        offset,
        cards: cards.to_vec(),
    }
}

fn pixel(screen: &Screen, x: u32, y: u32) -> [u8; 3] {
    if y == 0 && x < 4 {
        return match x {
            0 => screen.page,
            1 if screen.start => GREEN,
            2 if screen.end => BLUE,
            3 if screen.page == LIST => GREEN,
            _ => BACKGROUND,
        };
    }
    let content = y + screen.offset;
    for shown in &screen.cards {
        let top = 10 + 12 * shown.slot;
        if (top..top + 6).contains(&content) && (4..12).contains(&x) {
            return noise(x - 4, content - top);
        }
        if (top..top + 4).contains(&content) && (12..16).contains(&x) {
            return signature(shown.slot);
        }
        if (top..top + 4).contains(&content) && (16..20).contains(&x) {
            return [name_level(shown.name), shown.confidence, 90];
        }
        if (top + 6..top + 8).contains(&content) && (12..20).contains(&x) {
            return if shown.flag { GREEN } else { BLUE };
        }
    }
    BACKGROUND
}

fn pixels(screen: &Screen) -> Vec<[u8; 3]> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| pixel(screen, x, y)))
        .collect()
}

// The screen as a saved frame.
fn saved(screen: &Screen) -> Frame {
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels(screen).concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("saved frame")
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    let marker = (0..6)
        .flat_map(|y| (0..8).flat_map(move |x| noise(x, y)))
        .collect::<Vec<u8>>();
    Frame::from_pixels(
        8,
        6,
        marker,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("marker frame")
    .png_for_artifact()
    .expect("marker PNG")
}

// Reads a card's name and the read's confidence from its name patch: the name region starts at the
// card's anchor, and the name patch lies 12 px to its right.
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
        let x = usize::try_from(request.region.x + 13).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let at = (y * width + x) * 3;
        let red = request.frame.rgb8_pixels[at];
        let hundredths = request.frame.rgb8_pixels[at + 1];
        let text = (0..NAMES.len())
            .position(|name| name_level(name) == red)
            .map_or("", |name| NAMES[name]);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: Vec::new(),
            confidence: Some(f32::from(hundredths) / 100.0),
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

// The cards as a `repeated_anchor` layout on the list page, each naming one of the wanted keys or
// none; with `flag`, each also reads its flag patch as a `passed` feature.
fn card_layout(flag: bool) -> Value {
    let entries = MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    let mut features = vec![json!({
        "name": "member", "value": "identity", "target": "card/name",
        "offset": {"x": 0, "y": 0},
        "identity": {
            "entries": entries,
            "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                            "minimum_confidence_milli": 800}}
    })];
    if flag {
        features.push(
            json!({"name": "flag", "value": "passed", "target": "card/flag",
                             "offset": {"x": 12, "y": 6}}),
        );
    }
    json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
        "unknown_identity": "reject",
        "features": features
    })
}

// The scan of the list, LIST-A's: a vertical drag forward of 12 to 48 px, no rewind.
fn scan() -> Value {
    json!({
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
    })
}

// A trusted tap `id` from `from` to `to`.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 1, "y": 1},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": 1000}})
}

// `read_members`, with `others` when it is given and, with `flag`, the column `flag`.
fn read_members(others: Option<Value>, flag: bool) -> Value {
    let columns = if flag {
        json!([{"column": "flag", "feature": "flag"}])
    } else {
        json!([])
    };
    let mut read = json!({"layout_id": "layout/cards", "key_feature": "member",
                          "key_column": "id", "columns": columns, "fact_key": LIST_KEY,
                          "valid_for_ms": VALID_FOR_MS, "scan": scan()});
    if let Some(others) = others {
        read["others"] = others;
    }
    json!({
        "id": "read_members", "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": read
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

// The path: `open`, `read_members` and `close`, from home to done.
fn path_task(others: Option<Value>, flag: bool) -> Value {
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
        "candidate_layouts": [card_layout(flag)],
        "operations": [tap("open", "home", "list"), read_members(others, flag), close()]
    })
}

// The control of the path's step plan.
fn control() -> Value {
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
        "max_steps": 3
    })
}

// The page set: home, list and done, each told by pixel (0, 0).
fn pages() -> Value {
    let pages = ["home", "list", "done"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": pages})
}

// The recognition pack: the pages' colours, the list's marks, the guards, the card anchor, name
// and flag, and the card layout.
fn pack(flag: bool) -> Value {
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
            colour("page/done", 0, DONE),
            colour("check/list_start", 1, GREEN),
            colour("check/list_end", 2, BLUE),
            colour("guard/list", 3, GREEN),
            colour("guard/end", 2, BLUE),
            colour("card/flag", 0, GREEN),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 16, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.1,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [card_layout(flag)]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of the path with `others` (and, with `flag`, the column), as contained task
// admission prepares it.
fn load(others: Option<Value>, flag: bool) -> PreparedContainedTask {
    let entries = [
        ("control.json", bytes(&control())),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            bytes(&path_task(others, flag)),
        ),
        ("resources/assets/card.png", marker_png()),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&pack(flag)),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages()),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    let package = zip.finish().expect("finish zip").into_inner();
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package,
        Arc::new(CardNames),
    )
    .unwrap_or_else(|error| panic!("the package is refused: {error}"))
}

// The list of `screens` as the dry run reads it for `task`: each viewport settles on two equal
// captures.
fn offline(task: &PreparedContainedTask, screens: &[Screen]) -> ListReadingDryRun {
    let frames = screens
        .iter()
        .flat_map(|screen| [saved(screen), saved(screen)])
        .collect();
    dry_run_list_reading(task, "read_members", frames)
        .unwrap_or_else(|error| panic!("the dry run answers: {error}"))
}

// The mapping's reading of a name read at `SURE`: its mark, why it names no key and its nearest
// wanted key, alias and distance.
fn reading(
    name: usize,
    mark: CandidateNameMark,
    unread_cause: Option<CandidateUnreadCause>,
    nearest: (&str, &str, u16),
) -> CandidateNameReading {
    let (key, alias, distance) = nearest;
    CandidateNameReading {
        text: NAMES[name].to_owned(),
        confidence_milli: Some(SURE_MILLI),
        mark,
        unread_cause,
        nearest: Some(CandidateNearestName {
            key: key.to_owned(),
            variant: None,
            alias: alias.to_owned(),
            distance,
            limit: LIMIT,
        }),
    }
}

// The read of `position` on the coverage viewport `viewport`: its mark, and every sample the
// mapping read as `expected`.
#[track_caller]
fn assert_read(
    position: &ListDryRunPosition,
    viewport: u32,
    mark: CandidateNameMark,
    expected: &CandidateNameReading,
) {
    let read = position
        .reads
        .iter()
        .find(|read| read.viewport == viewport)
        .unwrap_or_else(|| {
            panic!(
                "position {} has no read on {viewport}",
                position.position_index
            )
        });
    assert_eq!(read.mark, mark, "{position:?}");
    assert!(!read.samples.is_empty(), "{position:?}");
    assert!(
        read.samples.iter().all(|sample| sample == expected),
        "{position:?}"
    );
}

// The row of the business ID `member`.
fn row(member: &str) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([("id".to_owned(), FactScalar::String(member.to_owned()))])
}

// The answer of an unread position: its cause and the identity it kept, and, when it kept none,
// why its identity read Unknown.
fn unread(
    cause: ListUnreadCause,
    identity: Option<&str>,
    identity_unknown: Option<CandidateUnknownReason>,
) -> ListPositionAnswer {
    ListPositionAnswer::Unread {
        cause,
        identity: identity.map(str::to_owned),
        identity_unknown,
    }
}

fn known(member: &str) -> ListPositionAnswer {
    ListPositionAnswer::Known {
        key: member.to_owned(),
    }
}

fn keys(members: &[&str]) -> Vec<String> {
    members.iter().map(|member| (*member).to_owned()).collect()
}

// Test 1 (rulings 2 and 1; the case this slice exists for): wanted and other cards mixed on one
// screen, both marks shown: `alpha`, `zulu`, `cxxxxxe` (a `charlie` card read wrong, far from every
// alias, with confidence) and `yankee`. With `others: pass_over` the list is whole and holds
// `member-a` alone: every position is answered in first-seen order where it stands (index k, content
// y 10 + 12 k, x 4, first seen and read on viewport 0): `alpha` known as `member-a`, its read
// known at 0 from `alpha`; `zulu` passed over, its read none of the keys at 990, beyond the
// distance, nearest `member-a` by `alpha` at 5 over the limit 2; the misread card passed over with
// its text `cxxxxxe` and its nearest name `member-c` by `charlie` at 5; `yankee` passed over. The
// summary: whole, 4 positions, 3 passed over, none unread, `member-a` shown, `member-b` and
// `member-c` not. The same package without the member answers the same positions and reads, the
// three other cards unread with `identity_unread` (their identity read Unknown `low_confidence`),
// the list `list_position_unread`, nothing passed over.
#[test]
fn list_a3_each_position_of_a_mixed_screen_is_answered_with_its_reads_and_the_wanted_keys() {
    let screen = list_at(
        0,
        true,
        &[
            card(0, ALPHA),
            card(1, ZULU),
            card(2, MISREAD_CHARLIE),
            card(3, YANKEE),
        ],
    );
    let alpha = reading(
        ALPHA,
        CandidateNameMark::Known,
        None,
        (MEMBERS[0], "alpha", 0),
    );
    let zulu = reading(
        ZULU,
        CandidateNameMark::NoneOfTheKeys,
        Some(CandidateUnreadCause::BeyondDistance),
        (MEMBERS[0], "alpha", 5),
    );
    let misread = reading(
        MISREAD_CHARLIE,
        CandidateNameMark::NoneOfTheKeys,
        Some(CandidateUnreadCause::BeyondDistance),
        (MEMBERS[2], "charlie", 5),
    );

    let read = offline(
        &load(Some(json!("pass_over")), false),
        std::slice::from_ref(&screen),
    );
    assert_eq!(
        read.list,
        Ok(ListReadingRows {
            rows: vec![row(MEMBERS[0])],
            confidence_milli: SURE_MILLI,
        })
    );
    assert_eq!(read.declaration.passed_over, Some(3));
    assert!(read.unknown.is_empty(), "{:?}", read.unknown);
    let answers = read
        .positions
        .iter()
        .map(|position| {
            (
                position.position_index,
                position.content_axis_px,
                position.cross_px,
                position.first_seen_viewport,
                position.first_read_viewport,
                position.answer.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        answers,
        vec![
            (0, 10, 4, 0, 0, known(MEMBERS[0])),
            (1, 22, 4, 0, 0, ListPositionAnswer::PassedOver),
            (2, 34, 4, 0, 0, ListPositionAnswer::PassedOver),
            (3, 46, 4, 0, 0, ListPositionAnswer::PassedOver),
        ]
    );
    assert_read(&read.positions[0], 0, CandidateNameMark::Known, &alpha);
    assert_read(
        &read.positions[1],
        0,
        CandidateNameMark::NoneOfTheKeys,
        &zulu,
    );
    assert_read(
        &read.positions[2],
        0,
        CandidateNameMark::NoneOfTheKeys,
        &misread,
    );
    let yankee = &read.positions[3].reads[0];
    assert_eq!(yankee.mark, CandidateNameMark::NoneOfTheKeys);
    assert_eq!(yankee.samples[0].text, NAMES[YANKEE]);
    assert_eq!(
        read.summary,
        ListDryRunSummary {
            whole: true,
            positions: 4,
            passed_over: 3,
            unread: 0,
            wanted_shown: keys(&[MEMBERS[0]]),
            wanted_not_shown: keys(&[MEMBERS[1], MEMBERS[2]]),
        }
    );

    let before = offline(&load(None, false), &[screen]);
    assert_eq!(
        before.list,
        Err(ListReadingMiss {
            problem: ResourceReadingProblem::ListPositionUnread,
            count: 3,
        })
    );
    assert_eq!(before.declaration.passed_over, None);
    let other = || {
        unread(
            ListUnreadCause::IdentityUnread,
            None,
            Some(CandidateUnknownReason::LowConfidence),
        )
    };
    assert_eq!(
        before
            .positions
            .iter()
            .map(|position| position.answer.clone())
            .collect::<Vec<_>>(),
        vec![known(MEMBERS[0]), other(), other(), other()]
    );
    assert_read(
        &before.positions[2],
        0,
        CandidateNameMark::NoneOfTheKeys,
        &misread,
    );
    assert_eq!(
        (
            before.summary.whole,
            before.summary.passed_over,
            before.summary.unread
        ),
        (false, 0, 3)
    );
}

// Test 2 (ruling 2): a near miss. `alpzz`, read with confidence 2 from `alpha` (within the limit 2,
// beyond the match distance 1), beside `alpha` and `zulu`: the list is `list_position_unread`,
// count 1; the near miss is unread, `identity_unread` (its identity read Unknown
// `low_confidence`), and its read shows its text, its mark unread, the cause `beyond_distance` and
// its distance 2 to `alpha` beside the limit 2; `zulu` is passed over.
#[test]
fn list_a3_a_near_miss_is_unread_with_its_text_and_its_distance() {
    let screen = list_at(
        0,
        true,
        &[card(0, ALPHA), card(1, NEAR_ALPHA), card(2, ZULU)],
    );
    let read = offline(&load(Some(json!("pass_over")), false), &[screen]);
    assert_eq!(
        read.list,
        Err(ListReadingMiss {
            problem: ResourceReadingProblem::ListPositionUnread,
            count: 1,
        })
    );
    let near = &read.positions[1];
    assert_eq!(
        (near.position_index, near.answer.clone()),
        (
            1,
            unread(
                ListUnreadCause::IdentityUnread,
                None,
                Some(CandidateUnknownReason::LowConfidence)
            )
        )
    );
    assert_read(
        near,
        0,
        CandidateNameMark::Unread,
        &reading(
            NEAR_ALPHA,
            CandidateNameMark::Unread,
            Some(CandidateUnreadCause::BeyondDistance),
            (MEMBERS[0], "alpha", 2),
        ),
    );
    assert_eq!(
        read.positions[2].answer,
        ListPositionAnswer::PassedOver,
        "zulu"
    );
    assert_eq!(
        (
            read.summary.whole,
            read.summary.passed_over,
            read.summary.unread
        ),
        (false, 1, 1)
    );
}

// Test 3 (ruling 5): the two causes LIST-A's offline entry called `contradicted`, told apart. The
// package with the column `flag`, over two viewports (offsets 0 and 24, the end mark on the
// second): card 2 reads `charlie` on the first and `zulu` on the second, its flag green on both;
// card 3 reads `bravo` on both, its flag green on the first and not on the second; card 0
// (`alpha`) shows on the first only, cards 1, 4 and 5 (`yankee`, `zulu`, `yankee`) are passed
// over. The list is `list_position_unread`, count 2: card 2 is unread `partly_passed_over`,
// keeping `member-c`, its read known on viewport 0 and none of the keys on viewport 1; card 3 is
// unread `contradicted`, keeping `member-b`, its reads known on both; `unknown` lists the two with
// the same causes, each first read on viewport 0. The summary: not whole, 6 positions, 3 passed
// over, 2 unread, every wanted key shown.
#[test]
fn list_a3_partly_passed_over_and_a_changed_column_are_two_causes() {
    let cards = |second: usize, flag: bool| {
        [
            card(0, ALPHA),
            card(1, YANKEE),
            card(2, second),
            Card {
                flag,
                ..card(3, BRAVO)
            },
            card(4, ZULU),
            card(5, YANKEE),
        ]
    };
    let viewports = [
        list_at(0, false, &cards(CHARLIE, true)),
        list_at(24, true, &cards(ZULU, false)),
    ];
    let read = offline(&load(Some(json!("pass_over")), true), &viewports);
    assert_eq!(
        read.list,
        Err(ListReadingMiss {
            problem: ResourceReadingProblem::ListPositionUnread,
            count: 2,
        })
    );
    assert_eq!(
        read.positions
            .iter()
            .map(|position| (position.position_index, position.answer.clone()))
            .collect::<Vec<_>>(),
        vec![
            (0, known(MEMBERS[0])),
            (1, ListPositionAnswer::PassedOver),
            (
                2,
                unread(ListUnreadCause::PartlyPassedOver, Some(MEMBERS[2]), None)
            ),
            (
                3,
                unread(ListUnreadCause::Contradicted, Some(MEMBERS[1]), None)
            ),
            (4, ListPositionAnswer::PassedOver),
            (5, ListPositionAnswer::PassedOver),
        ]
    );
    let partly = &read.positions[2];
    assert_eq!(
        partly
            .reads
            .iter()
            .map(|read| (read.viewport, read.mark))
            .collect::<Vec<_>>(),
        vec![
            (0, CandidateNameMark::Known),
            (1, CandidateNameMark::NoneOfTheKeys)
        ]
    );
    assert_read(
        partly,
        1,
        CandidateNameMark::NoneOfTheKeys,
        &reading(
            ZULU,
            CandidateNameMark::NoneOfTheKeys,
            Some(CandidateUnreadCause::BeyondDistance),
            (MEMBERS[0], "alpha", 5),
        ),
    );
    assert_eq!(
        read.positions[3]
            .reads
            .iter()
            .map(|read| (read.viewport, read.mark))
            .collect::<Vec<_>>(),
        vec![(0, CandidateNameMark::Known), (1, CandidateNameMark::Known)]
    );
    assert_eq!(
        read.unknown
            .iter()
            .map(|position| (
                position.position_index,
                position.first_read_viewport,
                position.cause,
                position.identity.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            (2, 0, ListUnreadCause::PartlyPassedOver, Some(MEMBERS[2])),
            (3, 0, ListUnreadCause::Contradicted, Some(MEMBERS[1])),
        ]
    );
    assert_eq!(
        read.summary,
        ListDryRunSummary {
            whole: false,
            positions: 6,
            passed_over: 3,
            unread: 2,
            wanted_shown: keys(&MEMBERS),
            wanted_not_shown: Vec::new(),
        }
    );
}
