// SPDX-License-Identifier: AGPL-3.0-only

// #463 PRIV-1 (contracts/privacy-zones.md; register §1.478; amendment 1, the owner's ruling of
// 11-18 that quantities an account holds are private): a repository root's sensitive-zone
// declaration `privacy-zones.json`, the one list of a package's read regions
// (`RecognitionPack::reads`), which the Lab's scene of a regression case pastes, and the
// comparison of the two (`compare_privacy_zones`). Packs are recognition packs as their JSON reads,
// on a 160x90 base; frames are synthetic; no Runtime, no device, no OCR model; the tests read no
// clock and never sleep.
// - `priv1_the_declaration_reads_public_local_and_absent_and_refuses_with_its_pointer` (1): a public
//   declaration with an explicit empty zone list, a zone without `kind` (identity) and a `value`
//   zone, a local one, an absent file; an unknown member, a duplicate id, a zone outside its base,
//   a base twice and an unknown kind, among others, each refused at its pointer.
// - `priv1_the_scene_of_the_s210_fixture_and_of_every_kind_is_byte_for_byte_as_before` (2): the
//   fixture of `gate_463_s210` (colour marks on one square) and a pack of every kind: the patches the
//   scene pastes and the scene's bytes are what the scene held before the list moved.
// - `priv1_the_1117_case_a_region_of_another_page_in_a_scene_meets_a_zone` (3).
// - `priv1_rectangles_that_only_touch_do_not_meet_and_a_press_box_is_not_read` (4).
// - `priv1_a_whole_frame_an_unbounded_region_and_an_undeclared_base_are_each_a_finding` (5).
// - `priv1_what_a_rectangle_cannot_see_is_counted_by_kind` (6).
// - `priv1_a_reading_and_a_zones_kind_are_reported_as_facts` (7, amendment 1).

use crate::recording::cases::{Patch, read_regions, scene_of};
use crate::recording::frames::decode_frame;
use crate::recording::model::RecordSize;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabErrorClass, ResourceDeclarationIssue, ResourceDeclarationReason, ResourceReadingDeclaration,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::{
    PackCoordinateSpace, PackReadArea, PackReadKind, PackReadRegion, PackRect, RecognitionPack,
};
use actingcommand_resource_tooling::{
    PRIVACY_ZONES_FILE, PrivacyBase, PrivacyCase, PrivacyDeclaration, PrivacyExemption,
    PrivacyFinding, PrivacyPlace, PrivacyRectKind, PrivacySceneRect, PrivacyUnseen,
    PrivacyUnseenKind, PrivacyZone, PrivacyZoneKind, PublicPrivacyZones, compare_privacy_zones,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// The recording's size and the package's base.
const SIZE: RecordSize = RecordSize {
    width: WIDTH,
    height: HEIGHT,
};
const BASE: PackCoordinateSpace = PackCoordinateSpace {
    width: WIDTH,
    height: HEIGHT,
};
const SCHEMA: &str = "actingcommand.privacy-zones.v1";
/// The made-up model of the OCR and NN targets; no test loads it.
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const BLACK: [u8; 3] = [0, 0, 0];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

/// `gate_463_s210`'s page square, every colour mark's region there.
const SQUARE: PackRect = rect(0, 0, 16, 16);

// ---- Packs -----------------------------------------------------------------------------------

// A recognition pack of `targets` and the candidate layouts `layouts`, as its JSON reads.
fn pack(targets: Value, layouts: Value) -> RecognitionPack {
    serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "targets": targets,
        "candidate_layouts": layouts
    }))
    .expect("a recognition pack")
}

fn color(id: &str, area: PackRect) -> Value {
    json!({"type": "color", "id": id, "region": area, "expected": RED})
}

// A colour probe over `area` whose press box is `click`.
fn pressed_color(id: &str, area: PackRect, click: PackRect) -> Value {
    json!({"type": "color", "id": id, "region": area, "expected": RED, "click": click})
}

fn digest(id: &str, area: PackRect) -> Value {
    json!({"type": "color_digest", "id": id, "region": area, "algorithm": "color_digest.v1",
           "columns": 2, "rows": 2, "cells": "00000000", "max_mean_milli": 100})
}

// A template target searched for in `region`, with a colour check over `check` when given and the
// press box `click`.
fn template(id: &str, region: Value, check: Option<Value>, click: PackRect) -> Value {
    let mut target = json!({"type": "template", "id": id, "template_path": "templates/neutral.png",
                            "region": region, "click": click});
    if let Some(check) = check {
        target["color_check"] = json!({"region": check, "expected": RED});
    }
    target
}

fn ocr(id: &str, region: Value, expected: &[&str]) -> Value {
    json!({"type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
           "match_mode": "contains", "expected": expected, "case_sensitive": false,
           "minimum_confidence": 0.8, "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256})
}

fn nn(id: &str, area: PackRect) -> Value {
    json!({"type": "nn", "id": id, "region": area, "model_ref": "neutral-nn",
           "model_sha256": MODEL_SHA256, "candidate_labels": ["on", "off"], "minimum_score": 0.5,
           "selection": "best", "timeout_ms": 1000})
}

fn click_only(id: &str, click: PackRect) -> Value {
    json!({"type": "click_only", "id": id, "click": click})
}

fn all_of(id: &str, members: &[&str]) -> Value {
    json!({"type": "composite", "id": id, "mode": "all_of", "members": members})
}

// A region placed relative to where the template `anchor` was found.
fn relative(anchor: &str) -> Value {
    json!({"mode": "template_relative", "anchor_target_id": anchor, "offset": {"x": 2, "y": 2},
           "width": 4, "height": 4})
}

// A select's `fixed_slots` layout on page `list`: one slot `slot` (its press box too) whose name the
// OCR target `name_target` reads, its identity entries `entries` (each `{id, aliases}`), and a
// shared OCR read over `shared` when given.
fn select_layout(
    name_target: &str,
    slot: PackRect,
    entries: Value,
    shared: Option<PackRect>,
) -> Value {
    let mut layout = json!({
        "id": "select/slots",
        "page_id": "list",
        "kind": "fixed_slots",
        "features": [{"name": "name", "value": "identity", "identity": {
            "entries": entries,
            "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                            "minimum_confidence_milli": 800}}}],
        "slots": [{"rect": slot, "click": slot, "targets": {"name": name_target}}]
    });
    if let Some(shared) = shared {
        layout["shared_ocr"] = json!({"region": shared, "languages": ["en"], "timeout_ms": 1000,
                                      "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256,
                                      "blocks": "detect"});
    }
    layout
}

// A resource reading `id` of the OCR target `target`, as a task's `resource_readings` declares it.
fn reading(id: &str, target: &str) -> ResourceReadingDeclaration {
    serde_json::from_value(json!({
        "id": id,
        "fact_key": format!("resource.{id}"),
        "page_id": "home",
        "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 999_999},
        "minimum_confidence_milli": 900,
        "valid_for_ms": 3_600_000
    }))
    .expect("a resource reading")
}

// ---- Declarations ----------------------------------------------------------------------------

// The public declaration `document`, which must read as one.
fn public(document: &Value) -> PublicPrivacyZones {
    let bytes = serde_json::to_vec(document).expect("declaration JSON");
    match PrivacyDeclaration::parse(&bytes) {
        Ok(PrivacyDeclaration::Public(zones)) => zones,
        Ok(other) => panic!("not a public declaration: {other:?}"),
        Err(error) => panic!("{}: {} {:?}", error.code, error.message, error.details),
    }
}

// A public declaration of one base, 160x90, with `zones`.
fn zones_at_base(zones: Value) -> Value {
    json!({"schema_version": SCHEMA, "repository": "public",
           "bases": [{"width": WIDTH, "height": HEIGHT, "zones": zones}]})
}

fn zone(id: &str, area: PackRect) -> Value {
    json!({"id": id, "rect": area})
}

fn zone_of(id: &str, kind: &str, area: PackRect) -> Value {
    json!({"id": id, "kind": kind, "rect": area})
}

// The place of the region of `target`, of `rect_kind`, a reading when `reading`.
fn region(target: &str, rect_kind: PrivacyRectKind, reading: bool) -> PrivacyPlace {
    PrivacyPlace::Region {
        target: target.to_owned(),
        rect_kind,
        reading,
    }
}

// The finding that `place`, at `area`, meets the zone `id` of `kind` at `zone_rect`.
fn met(
    place: PrivacyPlace,
    area: PackRect,
    id: &str,
    kind: PrivacyZoneKind,
    zone_rect: PackRect,
) -> PrivacyFinding {
    PrivacyFinding::ZoneMet {
        place,
        rect: area,
        zone_id: id.to_owned(),
        zone_kind: kind,
        zone_rect,
    }
}

// ---- Frames and scenes -----------------------------------------------------------------------

fn signed(value: u32) -> i32 {
    i32::try_from(value).expect("a pixel coordinate")
}

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: PackRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a 160x90 frame whose pixel at `(x, y)` is `color(x, y)`, encoded as `backend` does.
fn png_of(color: &dyn Fn(u32, u32) -> [u8; 3], backend: CaptureBackendName) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(color(x, y));
        }
    }
    Frame::from_pixels(WIDTH, HEIGHT, pixels, PixelFormat::Rgb8, backend)
        .expect("a frame")
        .encode_png_fast()
        .expect("a PNG")
}

// The scene a case holds of the frame `color`: a black canvas with the frame's pixels inside the
// rectangles `pasted` and nothing else, encoded as the Lab encodes a scene, built here pixel by
// pixel, apart from the Lab's scene builder.
fn expected_scene(color: &dyn Fn(u32, u32) -> [u8; 3], pasted: &[PackRect]) -> Vec<u8> {
    png_of(
        &|x, y| {
            if pasted.iter().any(|area| covers(*area, x, y)) {
                color(x, y)
            } else {
                BLACK
            }
        },
        CaptureBackendName::AdbScreencap,
    )
}

// The patch of `area`, as the scene builder keys its rectangles.
fn patch(area: PackRect) -> Patch {
    let unsigned = |value: i32| u32::try_from(value).expect("a patch inside the frame");
    Patch {
        y: unsigned(area.y),
        x: unsigned(area.x),
        height: unsigned(area.height),
        width: unsigned(area.width),
    }
}

// The patches of `expected`, each with the targets that read it in order.
fn patches(expected: &[(PackRect, &[&str])]) -> BTreeMap<Patch, Vec<String>> {
    expected
        .iter()
        .map(|(area, readers)| {
            (
                patch(*area),
                readers.iter().map(|reader| (*reader).to_owned()).collect(),
            )
        })
        .collect()
}

// The rectangles of a case's scene built of `regions`, as `record stop` writes them in its
// `case.json` (`scene_construction.rects`: each patch once, its `content` the targets that read it).
fn scene_rects(regions: &BTreeMap<Patch, Vec<String>>) -> Vec<PrivacySceneRect> {
    regions
        .iter()
        .map(|(patch, readers)| PrivacySceneRect {
            rect: rect(
                signed(patch.x),
                signed(patch.y),
                signed(patch.width),
                signed(patch.height),
            ),
            content: Some(readers.join(", ")),
        })
        .collect()
}

// ---- Tests -----------------------------------------------------------------------------------

// The work order's test 1, with amendment 1's: a public declaration (two bases, the second's zone
// list explicitly empty; a zone without `kind`, which is `identity`, and a `value` zone; the
// transition list), a local one and an absent file read as their three states; every other
// document is refused with its JSON pointer and reason, as the root's other declarations are.
#[test]
fn priv1_the_declaration_reads_public_local_and_absent_and_refuses_with_its_pointer() {
    let document = json!({
        "schema_version": SCHEMA,
        "repository": "public",
        "bases": [
            {"width": WIDTH, "height": HEIGHT, "zones": [
                zone("z_name", rect(10, 10, 40, 10)),
                zone_of("z_stock", "value", rect(100, 5, 50, 10)),
                zone_of("z_server", "identity", rect(0, 80, 160, 10))
            ]},
            {"width": 320, "height": 180, "zones": []}
        ],
        "exempt": [{"package_id": "neutral.test.earlier_task", "content": "0123456789ab"}]
    });
    let expected = PrivacyDeclaration::Public(PublicPrivacyZones {
        bases: vec![
            PrivacyBase {
                width: WIDTH,
                height: HEIGHT,
                zones: vec![
                    PrivacyZone {
                        id: "z_name".to_owned(),
                        kind: PrivacyZoneKind::Identity,
                        rect: rect(10, 10, 40, 10),
                    },
                    PrivacyZone {
                        id: "z_stock".to_owned(),
                        kind: PrivacyZoneKind::Value,
                        rect: rect(100, 5, 50, 10),
                    },
                    PrivacyZone {
                        id: "z_server".to_owned(),
                        kind: PrivacyZoneKind::Identity,
                        rect: rect(0, 80, 160, 10),
                    },
                ],
            },
            PrivacyBase {
                width: 320,
                height: 180,
                zones: Vec::new(),
            },
        ],
        exempt: vec![PrivacyExemption {
            package_id: "neutral.test.earlier_task".to_owned(),
            content: "0123456789ab".to_owned(),
        }],
    });
    let bytes = serde_json::to_vec(&document).expect("declaration JSON");
    assert_eq!(
        PrivacyDeclaration::parse(&bytes).expect("the public declaration"),
        expected
    );
    assert_eq!(
        PrivacyDeclaration::parse(
            serde_json::to_string(&json!({"schema_version": SCHEMA, "repository": "local"}))
                .expect("declaration JSON")
                .as_bytes()
        )
        .expect("the local declaration"),
        PrivacyDeclaration::Local
    );

    // A root without the file is absent, not an error; with it, the file as it reads.
    let root = TempDir::new().expect("a repository root");
    assert_eq!(
        PrivacyDeclaration::read(root.path()).expect("an absent declaration"),
        PrivacyDeclaration::Absent
    );
    fs::write(root.path().join(PRIVACY_ZONES_FILE), &bytes).expect("the declaration file");
    assert_eq!(
        PrivacyDeclaration::read(root.path()).expect("the declaration read from its root"),
        expected
    );

    let refused = [
        (
            json!({"schema_version": SCHEMA, "repository": "public", "bases": [], "note": ""}),
            "/note",
            ResourceDeclarationReason::UnknownField,
        ),
        (
            zones_at_base(json!([{"id": "z_name", "rect": rect(10, 10, 40, 10), "label": "x"}])),
            "/bases/0/zones/0/label",
            ResourceDeclarationReason::UnknownField,
        ),
        (
            zones_at_base(json!([
                zone("z_name", rect(10, 10, 40, 10)),
                zone("z_name", rect(60, 10, 40, 10))
            ])),
            "/bases/0/zones/1/id",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            zones_at_base(json!([zone("z_edge", rect(150, 10, 20, 10))])),
            "/bases/0/zones/0/rect",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            json!({"schema_version": SCHEMA, "repository": "public", "bases": [
                {"width": WIDTH, "height": HEIGHT, "zones": []},
                {"width": WIDTH, "height": HEIGHT, "zones": []}
            ]}),
            "/bases/1",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            zones_at_base(json!([zone_of("z_name", "secret", rect(10, 10, 40, 10))])),
            "/bases/0/zones/0/kind",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            zones_at_base(json!([{"id": "z_name", "kind": 1, "rect": rect(10, 10, 40, 10)}])),
            "/bases/0/zones/0/kind",
            ResourceDeclarationReason::InvalidType,
        ),
        (
            json!({"schema_version": SCHEMA, "repository": "local", "bases": []}),
            "/bases",
            ResourceDeclarationReason::UnconsumedField,
        ),
        (
            json!({"schema_version": SCHEMA, "repository": "private"}),
            "/repository",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            json!({"schema_version": SCHEMA}),
            "/repository",
            ResourceDeclarationReason::MissingField,
        ),
        (
            json!({"schema_version": "actingcommand.privacy-zones.v2", "repository": "local"}),
            "/schema_version",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            json!({"schema_version": SCHEMA, "repository": "public", "bases": [
                {"width": WIDTH, "height": HEIGHT, "zones": []}
            ], "exempt": [{"package_id": "neutral.test.earlier_task", "content": "0123"}]}),
            "/exempt/0/content",
            ResourceDeclarationReason::InvalidValue,
        ),
    ];
    for (document, pointer, reason) in refused {
        let bytes = serde_json::to_vec(&document).expect("declaration JSON");
        let error = PrivacyDeclaration::parse(&bytes).expect_err("a refused declaration");
        assert_eq!(
            error.code,
            SharedCode::ResourceDeclarationInvalid.as_str(),
            "{document}"
        );
        assert_eq!(error.class, LabErrorClass::UsageValidation, "{document}");
        let issue: ResourceDeclarationIssue =
            serde_json::from_value(error.details.clone().expect("the issue"))
                .expect("the issue's form");
        assert_eq!(issue.declaration_file, PRIVACY_ZONES_FILE, "{document}");
        assert_eq!(issue.field_path, pointer, "{document}");
        assert_eq!(issue.reason, reason, "{document}");
    }
}

// The work order's test 2: the scene builder now takes its regions from the one list, and every
// scene it builds is byte for byte what it was. Two packs: the fixture of `gate_463_s210` (its four
// screens' colour marks and its ending's mark, each on the page square), and a pack of every kind
// the list holds, with press boxes, a check, a click-only target, a select's name and a layout's
// shared read. The patches (each rectangle once, with the targets that read it in the package's
// order) are those the scene pasted before the list moved, written out here; the scene's bytes are
// those of a black canvas with exactly those patches of the frame, built here pixel by pixel.
#[test]
fn priv1_the_scene_of_the_s210_fixture_and_of_every_kind_is_byte_for_byte_as_before() {
    // The fixture of gate_463_s210: on step 1's screen, a red square on gray.
    let s210 = pack(
        json!([
            color("square_1", SQUARE),
            color("square_2", SQUARE),
            color("square_3", SQUARE),
            color("square_4", SQUARE),
            color("ending/alpha", SQUARE)
        ]),
        json!([]),
    );
    let regions = read_regions(&s210, &[], SIZE).expect("the s210 fixture's regions");
    let square: [(PackRect, &[&str]); 1] = [(
        SQUARE,
        &[
            "square_1",
            "square_2",
            "square_3",
            "square_4",
            "ending/alpha",
        ],
    )];
    assert_eq!(regions, patches(&square));
    let screen = |x: u32, y: u32| if covers(SQUARE, x, y) { RED } else { GRAY };
    let frame = decode_frame(
        png_of(&screen, CaptureBackendName::FixtureSimulation),
        "step 1's screen",
    )
    .expect("the frame");
    let scene = scene_of(&frame, &regions, SIZE).expect("the scene");
    assert!(
        scene == expected_scene(&screen, &[SQUARE]),
        "the s210 fixture's scene is not the black canvas with its square"
    );

    // Every kind the list holds.
    let every = pack(
        json!([
            template(
                "tpl/search",
                json!(rect(20, 20, 30, 20)),
                Some(json!(rect(25, 25, 5, 5))),
                rect(0, 80, 10, 10)
            ),
            pressed_color("probe/color", rect(60, 10, 8, 8), rect(140, 80, 8, 8)),
            color("probe/twin", rect(60, 10, 8, 8)),
            digest("probe/digest", rect(70, 10, 8, 8)),
            ocr("text/label", json!(rect(80, 30, 30, 10)), &["LABEL"]),
            nn("model/state", rect(120, 10, 20, 20)),
            click_only("press/only", rect(140, 70, 10, 10)),
            all_of("check/both", &["probe/color", "probe/digest"]),
            ocr(
                "slot/0/name",
                json!(rect(100, 60, 40, 12)),
                &["ALPHA", "BRAVO"]
            )
        ]),
        json!([select_layout(
            "slot/0/name",
            rect(100, 60, 40, 12),
            json!([{"id": "alpha", "aliases": ["ALPHA"]}, {"id": "bravo", "aliases": ["BRAVO"]}]),
            Some(rect(96, 56, 60, 30))
        )]),
    );
    let list = |target: &str, kind: PackReadKind, area: PackRect| PackReadRegion {
        target: target.to_owned(),
        kind,
        area: PackReadArea::Rect(area),
        reading: false,
    };
    assert_eq!(
        every.reads(&[]).regions,
        vec![
            list(
                "tpl/search",
                PackReadKind::TemplateSearch,
                rect(20, 20, 30, 20)
            ),
            list("tpl/search", PackReadKind::ColorCheck, rect(25, 25, 5, 5)),
            list("probe/color", PackReadKind::Color, rect(60, 10, 8, 8)),
            list("probe/twin", PackReadKind::Color, rect(60, 10, 8, 8)),
            list(
                "probe/digest",
                PackReadKind::ColorDigest,
                rect(70, 10, 8, 8)
            ),
            list("text/label", PackReadKind::Ocr, rect(80, 30, 30, 10)),
            list("model/state", PackReadKind::Nn, rect(120, 10, 20, 20)),
            list(
                "slot/0/name",
                PackReadKind::SelectName,
                rect(100, 60, 40, 12)
            ),
            list(
                "select/slots",
                PackReadKind::SharedOcr,
                rect(96, 56, 60, 30)
            ),
        ]
    );
    let pasted: [(PackRect, &[&str]); 7] = [
        (rect(20, 20, 30, 20), &["tpl/search"]),
        (rect(25, 25, 5, 5), &["tpl/search"]),
        (rect(60, 10, 8, 8), &["probe/color", "probe/twin"]),
        (rect(70, 10, 8, 8), &["probe/digest"]),
        (rect(80, 30, 30, 10), &["text/label"]),
        (rect(120, 10, 20, 20), &["model/state"]),
        (rect(100, 60, 40, 12), &["slot/0/name"]),
    ];
    let regions = read_regions(&every, &[], SIZE).expect("every kind's regions");
    assert_eq!(regions, patches(&pasted));
    let textured = |x: u32, y: u32| [u8::try_from(x).expect("x"), u8::try_from(y).expect("y"), 7];
    let frame = decode_frame(
        png_of(&textured, CaptureBackendName::FixtureSimulation),
        "a textured screen",
    )
    .expect("the frame");
    let scene = scene_of(&frame, &regions, SIZE).expect("the scene");
    let areas = pasted.map(|(area, _)| area);
    assert!(
        scene == expected_scene(&textured, &areas),
        "the scene of every kind is not the black canvas with exactly its patches"
    );
}

// The work order's test 3, the case of 11-17: page 1 (`step_01`) shows the player's name where page
// 2's title is, which page 2's OCR target reads. The scene of step 1's case, which the Lab builds
// from step 1's frame with every read region of the package, holds that region cut from page 1's
// frame: the comparison finds it, with its case, its target, its kind, both rectangles and the zone;
// and finds the same region as a recognition region of the package.
#[test]
fn priv1_the_1117_case_a_region_of_another_page_in_a_scene_meets_a_zone() {
    let title = rect(100, 5, 50, 10);
    let name_zone = rect(98, 4, 56, 14);
    let package = pack(
        json!([
            color("step_01/square", SQUARE),
            ocr("step_02/title", json!(title), &["TITLE"])
        ]),
        json!([]),
    );
    let regions = read_regions(&package, &[], SIZE).expect("the package's scene regions");
    let cases = [PrivacyCase::V1 {
        case: "step_01".to_owned(),
        rects: scene_rects(&regions),
    }];
    let zones = public(&zones_at_base(json!([zone("z_name", name_zone)])));
    let compared = compare_privacy_zones(
        "neutral.test.neutral_task",
        &package.reads(&[]),
        &cases,
        BASE,
        &zones,
    );
    assert_eq!(compared.package_id, "neutral.test.neutral_task");
    assert_eq!(
        compared.findings,
        vec![
            met(
                region("step_02/title", PrivacyRectKind::Ocr, false),
                title,
                "z_name",
                PrivacyZoneKind::Identity,
                name_zone
            ),
            met(
                PrivacyPlace::Scene {
                    case: "step_01".to_owned(),
                    index: 1,
                    content: Some("step_02/title".to_owned()),
                },
                title,
                "z_name",
                PrivacyZoneKind::Identity,
                name_zone
            ),
        ]
    );
    let PrivacyFinding::ZoneMet { place, .. } = &compared.findings[1] else {
        panic!("not a zone met: {:?}", compared.findings[1]);
    };
    assert_eq!(place.rect_kind(), PrivacyRectKind::SceneRect);
    assert!(!place.reading());
}

// The work order's test 4: a rectangle that only touches a zone at an edge or a corner does not
// meet it, and one that overlaps it by a single pixel does; a press box over a zone (a colour
// probe's `click`, a template's, a click-only target) is in no list and is not found.
#[test]
fn priv1_rectangles_that_only_touch_do_not_meet_and_a_press_box_is_not_read() {
    let zone_rect = rect(10, 10, 20, 20);
    let press = rect(12, 12, 16, 16);
    let package = pack(
        json!([
            color("probe/left", rect(0, 10, 10, 10)),
            color("probe/above", rect(10, 0, 20, 10)),
            color("probe/corner", rect(30, 30, 5, 5)),
            color("probe/inside", rect(29, 29, 2, 2)),
            pressed_color("probe/pressed", rect(50, 50, 5, 5), press),
            template("tpl/pressed", json!(rect(60, 50, 10, 10)), None, press),
            click_only("press/only", press)
        ]),
        json!([]),
    );
    let reads = package.reads(&[]);
    assert_eq!(
        reads
            .regions
            .iter()
            .map(|region| region.target.as_str())
            .collect::<Vec<_>>(),
        [
            "probe/left",
            "probe/above",
            "probe/corner",
            "probe/inside",
            "probe/pressed",
            "tpl/pressed"
        ]
    );
    assert!(
        reads
            .regions
            .iter()
            .all(|region| region.area != PackReadArea::Rect(press)),
        "{reads:?}"
    );
    let zones = public(&zones_at_base(json!([zone("z_name", zone_rect)])));
    let compared = compare_privacy_zones("neutral.test.neutral_task", &reads, &[], BASE, &zones);
    assert_eq!(
        compared.findings,
        vec![met(
            region("probe/inside", PrivacyRectKind::Color, false),
            rect(29, 29, 2, 2),
            "z_name",
            PrivacyZoneKind::Identity,
            zone_rect
        )]
    );
}

// The work order's test 5: a region over the whole base and a region `full_frame` are each a
// whole-frame finding, named once and not once per zone; a region placed relative to a template's
// match is an unbounded one; a scene's rectangle over the whole canvas is a whole-frame finding of
// its case; and a base size the declaration does not list is a finding of its own, before any
// other, with no zone compared.
#[test]
fn priv1_a_whole_frame_an_unbounded_region_and_an_undeclared_base_are_each_a_finding() {
    let whole = rect(0, 0, 160, 90);
    let package = pack(
        json!([
            color("probe/whole", whole),
            template(
                "tpl/anchor",
                json!(rect(40, 40, 10, 10)),
                Some(relative("tpl/anchor")),
                rect(40, 40, 10, 10)
            ),
            ocr("text/full", json!("full_frame"), &["TEXT"]),
            ocr("text/relative", relative("tpl/anchor"), &["TEXT"])
        ]),
        json!([]),
    );
    let reads = package.reads(&[]);
    let zones = public(&zones_at_base(json!([
        zone("z_name", rect(100, 60, 10, 10)),
        zone("z_server", rect(120, 60, 10, 10))
    ])));
    let cases = [PrivacyCase::V1 {
        case: "final".to_owned(),
        rects: vec![PrivacySceneRect {
            rect: whole,
            content: None,
        }],
    }];
    let compared = compare_privacy_zones("neutral.test.neutral_task", &reads, &cases, BASE, &zones);
    assert_eq!(
        compared.findings,
        vec![
            PrivacyFinding::WholeFrame {
                place: region("probe/whole", PrivacyRectKind::Color, false),
            },
            PrivacyFinding::RegionUnbounded {
                place: region("tpl/anchor", PrivacyRectKind::ColorCheck, false),
            },
            PrivacyFinding::WholeFrame {
                place: region("text/full", PrivacyRectKind::Ocr, false),
            },
            PrivacyFinding::RegionUnbounded {
                place: region("text/relative", PrivacyRectKind::Ocr, false),
            },
            PrivacyFinding::WholeFrame {
                place: PrivacyPlace::Scene {
                    case: "final".to_owned(),
                    index: 0,
                    content: None,
                },
            },
        ]
    );

    // A package at a base the declaration does not list.
    let small = pack(json!([color("probe/square", SQUARE)]), json!([]));
    let undeclared = PackCoordinateSpace {
        width: 320,
        height: 180,
    };
    let compared = compare_privacy_zones(
        "neutral.test.neutral_task",
        &small.reads(&[]),
        &[],
        undeclared,
        &zones,
    );
    assert_eq!(
        compared.findings,
        vec![PrivacyFinding::BaseUndeclared {
            width: 320,
            height: 180,
        }]
    );
}

// The work order's test 6: what a rectangle check cannot see is counted by kind: the texts the
// package's OCR targets expect (a select's name target's too), the aliases its layout's identity
// entries are read as, and its cases of another schema, whose scenes are not compared.
#[test]
fn priv1_what_a_rectangle_cannot_see_is_counted_by_kind() {
    let package = pack(
        json!([
            ocr("text/a", json!(rect(10, 10, 30, 10)), &["ONE", "TWO"]),
            ocr("text/b", json!(rect(10, 30, 30, 10)), &["THREE"]),
            ocr("slot/0/name", json!(rect(100, 60, 40, 12)), &["ALPHA"])
        ]),
        json!([select_layout(
            "slot/0/name",
            rect(100, 60, 40, 12),
            json!([
                {"id": "alpha", "aliases": ["ALPHA", "ALFA"]},
                {"id": "bravo", "aliases": ["BRAVO"]}
            ]),
            None
        )]),
    );
    let cases = [
        PrivacyCase::V1 {
            case: "step_01".to_owned(),
            rects: Vec::new(),
        },
        PrivacyCase::OtherSchema {
            case: "legacy_a".to_owned(),
        },
        PrivacyCase::OtherSchema {
            case: "legacy_b".to_owned(),
        },
    ];
    let zones = public(&zones_at_base(json!([])));
    let compared = compare_privacy_zones(
        "neutral.test.neutral_task",
        &package.reads(&[]),
        &cases,
        BASE,
        &zones,
    );
    assert_eq!(compared.findings, Vec::new());
    assert_eq!(
        compared.unseen,
        PrivacyUnseen {
            expected_texts: 4,
            aliases: 3,
            cases_other_schema: 2,
        }
    );
    assert_eq!(
        compared.unseen.by_kind(),
        [
            (PrivacyUnseenKind::ExpectedText, 4),
            (PrivacyUnseenKind::Alias, 3),
            (PrivacyUnseenKind::CaseOtherSchema, 2),
        ]
    );
}

// Amendment 1's tests: a region whose target a resource reading of the package names is marked a
// reading in the one list. In one `value` zone, a reading's region and a region that is no reading
// are both reported, each with its facts; a reading's region in an `identity` zone (a zone without
// `kind`) is reported too. The comparison decides nothing of them.
#[test]
fn priv1_a_reading_and_a_zones_kind_are_reported_as_facts() {
    let stock_zone = rect(100, 5, 50, 10);
    let name_zone = rect(10, 60, 50, 10);
    let stock = rect(102, 6, 40, 8);
    let label = rect(100, 5, 20, 10);
    let count = rect(12, 62, 30, 6);
    let package = pack(
        json!([
            ocr("text/stock", json!(stock), &[]),
            ocr("text/stock_label", json!(label), &["STOCK"]),
            ocr("text/name_count", json!(count), &[])
        ]),
        json!([]),
    );
    let readings = [
        reading("stock", "text/stock"),
        reading("name_count", "text/name_count"),
    ];
    let reads = package.reads(&readings);
    assert_eq!(
        reads
            .regions
            .iter()
            .map(|region| (region.target.as_str(), region.reading))
            .collect::<Vec<_>>(),
        [
            ("text/stock", true),
            ("text/stock_label", false),
            ("text/name_count", true)
        ]
    );
    let zones = public(&zones_at_base(json!([
        zone_of("z_stock", "value", stock_zone),
        zone("z_name", name_zone)
    ])));
    assert_eq!(zones.bases[0].zones[1].kind, PrivacyZoneKind::Identity);
    let compared = compare_privacy_zones("neutral.test.neutral_task", &reads, &[], BASE, &zones);
    assert_eq!(
        compared.findings,
        vec![
            met(
                region("text/stock", PrivacyRectKind::Ocr, true),
                stock,
                "z_stock",
                PrivacyZoneKind::Value,
                stock_zone
            ),
            met(
                region("text/stock_label", PrivacyRectKind::Ocr, false),
                label,
                "z_stock",
                PrivacyZoneKind::Value,
                stock_zone
            ),
            met(
                region("text/name_count", PrivacyRectKind::Ocr, true),
                count,
                "z_name",
                PrivacyZoneKind::Identity,
                name_zone
            ),
        ]
    );
    let PrivacyFinding::ZoneMet { place, .. } = &compared.findings[0] else {
        panic!("not a zone met: {:?}", compared.findings[0]);
    };
    assert!(place.reading());
    assert_eq!(place.rect_kind(), PrivacyRectKind::Ocr);
}
