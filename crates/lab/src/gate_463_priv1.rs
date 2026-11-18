// SPDX-License-Identifier: AGPL-3.0-only

// #463 PRIV-1 (contracts/privacy-zones.md; register §1.478; amendment 1, the owner's ruling of
// 11-18 that quantities an account holds are private; fix round 1, after the independent review): a
// repository root's sensitive-zone declaration `privacy-zones.json`, the one list of a package's
// read regions (`RecognitionPack::reads`), which the Lab's scene of a regression case pastes, its
// readings (`RecognitionPack::reading_targets`), and the comparison of the list with the zones
// (`compare_privacy_zones`). Packs are recognition packs as their JSON reads, on a 160x90 base;
// frames are synthetic; no Runtime, no device, no OCR model; the tests read no clock and never
// sleep.
// - `priv1_the_declaration_reads_public_local_and_absent_and_refuses_with_its_pointer` (1): a public
//   declaration with an explicit empty zone list, a zone without `kind` (identity) and a `value`
//   zone, a local one, an absent file; an unknown member, a duplicate id, a zone outside its base,
//   a base twice and an unknown kind, among others, each refused at its pointer.
// - `priv1_the_scene_of_the_s210_fixture_and_of_every_kind_is_byte_for_byte_as_before` (2, fix
//   round 1): for a fixture written after `gate_463_s210`'s and for a pack of every kind of target
//   and layout read, the patches and the scene built from the one list are those the function it
//   replaced builds (`read_regions_before`, kept below as the oracle); the same refusal from both.
// - `priv1_the_1117_case_a_region_of_another_page_in_a_scene_meets_a_zone` (3).
// - `priv1_rectangles_that_only_touch_do_not_meet_and_a_press_box_is_not_read` (4).
// - `priv1_a_whole_frame_an_unbounded_region_and_an_undeclared_base_are_each_a_finding` (5).
// - `priv1_what_a_rectangle_cannot_see_is_counted_by_kind` (6).
// - `priv1_a_reading_and_a_zones_kind_are_reported_as_facts` (7, amendment 1).
// - `priv1_every_place_the_kernel_reads_a_value_of_the_account_marks_its_region` (8, fix round 1):
//   one region per source of a reading marked, a target of no source not.
// - `priv1_a_feature_read_at_an_offset_is_listed_where_the_kernel_reads_it` (9, fix round 1).

use crate::recording::cases::{self, Patch, read_regions, scene_of};
use crate::recording::frames::decode_frame;
use crate::recording::model::RecordSize;
use crate::recording::store::invalid;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabErrorClass, LabResult, ResourceDeclarationIssue, ResourceDeclarationReason,
    ResourceReadingDeclaration,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition_pack::{
    PackCoordinateSpace, PackReadArea, PackReadKind, PackReadRegion, PackReadingDeclarations,
    PackRect, PackRegion, RecognitionPack, RecognitionTarget,
};
use actingcommand_resource_tooling::{
    PRIVACY_ZONES_FILE, PrivacyBase, PrivacyCase, PrivacyDeclaration, PrivacyExemption,
    PrivacyFinding, PrivacyPlace, PrivacyRectKind, PrivacyUnseen, PrivacyUnseenKind, PrivacyZone,
    PrivacyZoneKind, PublicPrivacyZones, compare_privacy_zones,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
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

// A value test that the number the OCR target `side` reads is at least 1.
fn at_least_one(id: &str, side: &str) -> Value {
    json!({"type": "value_test", "id": id,
           "left": {"target_id": side, "trim": "whitespace_v1",
                    "value": {"type": "unsigned_integer", "min": 0, "max": 999}},
           "op": "ge", "right": {"const": 1}})
}

// A region placed relative to where the template `anchor` was found.
fn relative(anchor: &str) -> Value {
    json!({"mode": "template_relative", "anchor_target_id": anchor, "offset": {"x": 2, "y": 2},
           "width": 4, "height": 4})
}

// A select's identity feature `name` over `entries` (each `{id, aliases}`), read by OCR aliases;
// with `target` and `offset` when it reads its target at an offset.
fn identity_feature(name: &str, entries: Value, placed: Option<(&str, i32, i32)>) -> Value {
    let mut feature = json!({"name": name, "value": "identity", "identity": {
        "entries": entries,
        "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                        "minimum_confidence_milli": 800}}});
    if let Some((target, x, y)) = placed {
        feature["target"] = json!(target);
        feature["offset"] = json!({"x": x, "y": y});
    }
    feature
}

// An integer feature `name` (`ocr_integer`), with `target` and `offset` when it reads its target
// at an offset.
fn integer_feature(name: &str, placed: Option<(&str, i32, i32)>) -> Value {
    let mut feature = json!({"name": name, "value": "ocr_integer",
                             "integer": {"min": 0, "max": 9999, "minimum_confidence_milli": 900}});
    if let Some((target, x, y)) = placed {
        feature["target"] = json!(target);
        feature["offset"] = json!({"x": x, "y": y});
    }
    feature
}

// A feature `name` that reads the verdict of `target` at the offset `(x, y)`.
fn passed_feature(name: &str, target: &str, x: i32, y: i32) -> Value {
    json!({"name": name, "value": "passed", "target": target, "offset": {"x": x, "y": y}})
}

// A `fixed_slots` layout `id` on page `list` of `features`, its slots `(rect, targets)` (each slot
// its own press box), and a shared OCR read over `shared` when given.
fn slots_layout(
    id: &str,
    features: Value,
    slots: &[(PackRect, Value)],
    shared: Option<PackRect>,
) -> Value {
    let slots = slots
        .iter()
        .map(|(area, targets)| json!({"rect": area, "click": area, "targets": targets}))
        .collect::<Vec<_>>();
    let mut layout = json!({"id": id, "page_id": "list", "kind": "fixed_slots",
                            "features": features, "slots": slots});
    if let Some(shared) = shared {
        layout["shared_ocr"] = json!({"region": shared, "languages": ["en"], "timeout_ms": 1000,
                                      "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256,
                                      "blocks": "detect"});
    }
    layout
}

// A `repeated_anchor` layout `id` on page `list`: the template `anchor`'s matches are its
// instances; its `features` read their targets at offsets from each match.
fn anchored_layout(id: &str, anchor: &str, features: Value) -> Value {
    json!({"id": id, "page_id": "list", "kind": "repeated_anchor", "anchor": anchor,
           "max_instances": 4, "order": "top_to_bottom", "instance_rect": rect(0, 0, 60, 12),
           "click": rect(0, 0, 60, 12), "features": features})
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
    slots_layout(
        "select/slots",
        json!([identity_feature("name", entries, None)]),
        &[(slot, json!({"name": name_target}))],
        shared,
    )
}

// The JSON of a resource reading `id` of the OCR target `target`, as a task's `resource_readings`
// declares it.
fn reading_json(id: &str, target: &str) -> Value {
    json!({
        "id": id,
        "fact_key": format!("resource.{id}"),
        "page_id": "home",
        "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 999_999},
        "minimum_confidence_milli": 900,
        "valid_for_ms": 3_600_000
    })
}

fn reading(id: &str, target: &str) -> ResourceReadingDeclaration {
    serde_json::from_value(reading_json(id, target)).expect("a resource reading")
}

// An OCR field `id` of the group `row` read from `target`, of the type `value`.
fn field(id: &str, target: &str, value: Value) -> Value {
    json!({"id": id, "group": "row", "target_id": target, "required": true, "privacy": "public",
           "trim": "whitespace_v1", "value": value})
}

// A region of the one list.
fn read_region(
    target: &str,
    kind: PackReadKind,
    area: PackReadArea,
    reading: bool,
    layout: Option<&str>,
) -> PackReadRegion {
    PackReadRegion {
        target: target.to_owned(),
        kind,
        area,
        reading,
        layout: layout.map(str::to_owned),
    }
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

// The place of the region of `target` a target declares itself, of `rect_kind`, a reading when
// `reading`.
fn region(target: &str, rect_kind: PrivacyRectKind, reading: bool) -> PrivacyPlace {
    PrivacyPlace::Region {
        target: target.to_owned(),
        rect_kind,
        reading,
        layout: None,
    }
}

// The place of the region of `target` the candidate layout `layout` reads.
fn layout_region(
    target: &str,
    rect_kind: PrivacyRectKind,
    reading: bool,
    layout: &str,
) -> PrivacyPlace {
    PrivacyPlace::Region {
        target: target.to_owned(),
        rect_kind,
        reading,
        layout: Some(layout.to_owned()),
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
fn patch_of(area: PackRect) -> Patch {
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
                patch_of(*area),
                readers.iter().map(|reader| (*reader).to_owned()).collect(),
            )
        })
        .collect()
}

// The rectangles of a case's scene built of `regions`, as `record stop` writes them in its
// `case.json` (`scene_construction.rects`: each patch once).
fn scene_rects(regions: &BTreeMap<Patch, Vec<String>>) -> Vec<PackRect> {
    regions
        .keys()
        .map(|patch| {
            rect(
                signed(patch.x),
                signed(patch.y),
                signed(patch.width),
                signed(patch.height),
            )
        })
        .collect()
}

// ---- The function the one list replaced ------------------------------------------------------

// Fix round 1: the scene builder's `read_regions` as it was before the list moved into the
// recognition pack (main `a8d0aa43`, `crates/lab/src/recording/cases.rs`), kept here only as the
// oracle of test 2: every rectangle the recognition targets of `pack` read, each once with the
// targets that read it in the package's order: a template's search region (and its colour check's
// region), a colour or colour digest probe's region, an OCR or NN target's region. A check, a value
// test and a click-only target read nothing of their own. A region that is no fixed rectangle
// inside the recording's frame (`size`) is the check's error.
fn read_regions_before(
    pack: &RecognitionPack,
    size: RecordSize,
) -> LabResult<BTreeMap<Patch, Vec<String>>> {
    let mut regions: BTreeMap<Patch, Vec<String>> = BTreeMap::new();
    for target in &pack.targets {
        let read = match target {
            RecognitionTarget::Template(template) => {
                let mut read = vec![(&template.id, fixed_before(&template.id, &template.region)?)];
                if let Some(check) = &template.color_check {
                    read.push((&template.id, fixed_before(&template.id, &check.region)?));
                }
                read
            }
            RecognitionTarget::Color(color) => vec![(&color.id, color.region)],
            RecognitionTarget::ColorDigest(digest) => vec![(&digest.id, digest.region)],
            RecognitionTarget::Ocr(ocr) => vec![(&ocr.id, fixed_before(&ocr.id, &ocr.region)?)],
            RecognitionTarget::Nn(nn) => vec![(&nn.id, fixed_before(&nn.id, &nn.region)?)],
            RecognitionTarget::ClickOnly(_)
            | RecognitionTarget::Composite(_)
            | RecognitionTarget::ValueTest(_) => Vec::new(),
        };
        for (id, area) in read {
            let readers = regions.entry(cases::patch(id, area, size)?).or_default();
            if !readers.contains(id) {
                readers.push(id.clone());
            }
        }
    }
    Ok(regions)
}

// The old function's fixed rectangle `region` of the target `id`.
fn fixed_before(id: &str, region: &PackRegion) -> LabResult<PackRect> {
    match region {
        PackRegion::Rect(area) => Ok(*area),
        PackRegion::TemplateRelative(_) | PackRegion::Keyword(_) => Err(invalid(
            "validation_failed",
            format!(
                "target '{id}' reads a region that is no fixed rectangle: a case's scene pastes \
                 fixed regions only"
            ),
        )),
    }
}

// That the patches and the scene of the frame `screen` that the Lab builds of `package` from the
// one list are those the old function gives.
fn assert_scene_as_before(
    name: &str,
    package: &RecognitionPack,
    screen: &dyn Fn(u32, u32) -> [u8; 3],
) {
    let listed = read_regions(package, SIZE).expect("the patches of the one list");
    let before = read_regions_before(package, SIZE).expect("the patches of the old function");
    assert_eq!(listed, before, "{name}");
    let frame = decode_frame(png_of(screen, CaptureBackendName::FixtureSimulation), name)
        .expect("the frame");
    let scene = scene_of(&frame, &listed, SIZE).expect("the scene of the one list");
    assert!(
        scene == scene_of(&frame, &before, SIZE).expect("the scene of the old function"),
        "{name}: the scene is not the old function's"
    );
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

// The work order's test 2, fix round 1 (the review): the scene builder takes its regions from the
// one list, and every scene it builds is byte for byte what it was. The oracle is the function the
// list replaced, kept above as `read_regions_before`. Two packs: a fixture written here after the
// fixture of `gate_463_s210` (its four screens' colour marks and its ending's mark, each on the page
// square), not recorded: no scene of a recording is read; and a pack of every kind of target (a
// template's search area and colour check, two colour probes on one rectangle, a digest, an OCR
// target, an NN target, a click-only target, a check, a value test, a select's name and quantity
// columns) and of every read a layout makes itself (a shared read, a feature a `fixed_slots` layout
// reads at an offset, a feature a `repeated_anchor` layout reads at an offset). For each, the
// patches and the scene's bytes built from the one list equal those built from the old function;
// and a region that is no fixed rectangle is the same refusal from both.
#[test]
fn priv1_the_scene_of_the_s210_fixture_and_of_every_kind_is_byte_for_byte_as_before() {
    // The fixture written after gate_463_s210's: on step 1's screen, a red square on gray.
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
    let screen = |x: u32, y: u32| if covers(SQUARE, x, y) { RED } else { GRAY };
    assert_scene_as_before("the s210 fixture", &s210, &screen);
    // Its one patch with its five readers, and its scene: the black canvas with the square cut from
    // the frame, built here pixel by pixel.
    let regions = read_regions(&s210, SIZE).expect("the s210 fixture's regions");
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

    // Every kind of target and of layout read.
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
            at_least_one("check/count", "slot/0/count"),
            ocr(
                "slot/0/name",
                json!(rect(100, 60, 40, 12)),
                &["ALPHA", "BRAVO"]
            ),
            ocr("slot/0/count", json!(rect(100, 72, 20, 10)), &[]),
            template("tpl/badge", json!(rect(0, 0, 6, 6)), None, rect(0, 0, 6, 6)),
            ocr("row/value", json!(rect(0, 0, 12, 6)), &[])
        ]),
        json!([
            slots_layout(
                "select/slots",
                json!([
                    identity_feature(
                        "name",
                        json!([{"id": "alpha", "aliases": ["ALPHA"]},
                               {"id": "bravo", "aliases": ["BRAVO"]}]),
                        None
                    ),
                    integer_feature("count", None),
                    passed_feature("badge", "tpl/badge", 30, 14)
                ]),
                &[(
                    rect(100, 60, 40, 24),
                    json!({"name": "slot/0/name", "count": "slot/0/count"})
                )],
                Some(rect(96, 56, 60, 30))
            ),
            anchored_layout(
                "rows",
                "tpl/search",
                json!([integer_feature("value", Some(("row/value", 2, 2)))])
            )
        ]),
    );
    let textured = |x: u32, y: u32| [u8::try_from(x).expect("x"), u8::try_from(y).expect("y"), 7];
    assert_scene_as_before("every kind", &every, &textured);
    // The list in the pack's order: each target's own regions, then each layout's reads; no press
    // box. The quantity column is a reading wherever it is read, the layouts' reads name their
    // layout, and the scene pastes none of them.
    let at = PackReadArea::Rect;
    assert_eq!(
        every.reads(&PackReadingDeclarations::default()).regions,
        vec![
            read_region(
                "tpl/search",
                PackReadKind::TemplateSearch,
                at(rect(20, 20, 30, 20)),
                false,
                None
            ),
            read_region(
                "tpl/search",
                PackReadKind::ColorCheck,
                at(rect(25, 25, 5, 5)),
                false,
                None
            ),
            read_region(
                "probe/color",
                PackReadKind::Color,
                at(rect(60, 10, 8, 8)),
                false,
                None
            ),
            read_region(
                "probe/twin",
                PackReadKind::Color,
                at(rect(60, 10, 8, 8)),
                false,
                None
            ),
            read_region(
                "probe/digest",
                PackReadKind::ColorDigest,
                at(rect(70, 10, 8, 8)),
                false,
                None
            ),
            read_region(
                "text/label",
                PackReadKind::Ocr,
                at(rect(80, 30, 30, 10)),
                false,
                None
            ),
            read_region(
                "model/state",
                PackReadKind::Nn,
                at(rect(120, 10, 20, 20)),
                false,
                None
            ),
            read_region(
                "slot/0/name",
                PackReadKind::SelectName,
                at(rect(100, 60, 40, 12)),
                false,
                None
            ),
            read_region(
                "slot/0/count",
                PackReadKind::SelectQuantity,
                at(rect(100, 72, 20, 10)),
                true,
                None
            ),
            read_region(
                "tpl/badge",
                PackReadKind::TemplateSearch,
                at(rect(0, 0, 6, 6)),
                false,
                None
            ),
            read_region(
                "row/value",
                PackReadKind::SelectQuantity,
                at(rect(0, 0, 12, 6)),
                true,
                None
            ),
            read_region(
                "select/slots",
                PackReadKind::SharedOcr,
                at(rect(96, 56, 60, 30)),
                false,
                Some("select/slots")
            ),
            read_region(
                "tpl/badge",
                PackReadKind::TemplateSearch,
                at(rect(130, 74, 6, 6)),
                false,
                Some("select/slots")
            ),
            read_region(
                "row/value",
                PackReadKind::SelectQuantity,
                PackReadArea::Unbounded,
                true,
                Some("rows")
            ),
        ]
    );

    // A region that is no fixed rectangle: the same refusal from both.
    let unbounded = pack(
        json!([
            template(
                "tpl/anchor",
                json!(rect(40, 40, 10, 10)),
                None,
                rect(40, 40, 10, 10)
            ),
            ocr("text/relative", relative("tpl/anchor"), &["TEXT"])
        ]),
        json!([]),
    );
    assert_eq!(
        read_regions(&unbounded, SIZE).expect_err("the list refuses a relative region"),
        read_regions_before(&unbounded, SIZE).expect_err("so did the old function")
    );
}

// The work order's test 3, the case of 11-17: page 1 (`step_01`) shows the player's name where page
// 2's title is, which page 2's OCR target reads. The scene of step 1's case, which the Lab builds
// from step 1's frame with every read region of the package, holds that region cut from page 1's
// frame: the comparison finds it, with its case (by its id only), its target, its kind, both
// rectangles and the zone; and finds the same region as a recognition region of the package.
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
    let regions = read_regions(&package, SIZE).expect("the package's scene regions");
    let cases = [PrivacyCase::V1 {
        case: "step_01".to_owned(),
        rects: scene_rects(&regions),
    }];
    let zones = public(&zones_at_base(json!([zone("z_name", name_zone)])));
    let compared = compare_privacy_zones(
        "neutral.test.neutral_task",
        &package.reads(&PackReadingDeclarations::default()),
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
    let reads = package.reads(&PackReadingDeclarations::default());
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
    let reads = package.reads(&PackReadingDeclarations::default());
    let zones = public(&zones_at_base(json!([
        zone("z_name", rect(100, 60, 10, 10)),
        zone("z_server", rect(120, 60, 10, 10))
    ])));
    let cases = [PrivacyCase::V1 {
        case: "final".to_owned(),
        rects: vec![whole],
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
        &small.reads(&PackReadingDeclarations::default()),
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
        &package.reads(&PackReadingDeclarations::default()),
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
    let task = PackReadingDeclarations {
        readings: vec![
            reading("stock", "text/stock"),
            reading("name_count", "text/name_count"),
        ],
        ..PackReadingDeclarations::default()
    };
    let reads = package.reads(&task);
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

// Fix round 1, the review's must-change 2: every place whose recognised value the kernel
// publishes as, or into, a fact of the account marks its target's regions a reading, from
// the members of the entry task as `task.json` writes them: a mark's target (`resource_readings`),
// an adjustment's amount (`resource_adjustments`; an adjustment without amount names no target), the
// integer column of the layout a `read_list` step reads (its `ocr_integer` feature, read at an
// offset from the anchor's match: both its own region and the layout's read), a select's quantity
// column (a `fixed_slots` slot's target of an `ocr_integer` feature, of kind `select_quantity`), and
// a field list's integer column (`field_lists`, the `unsigned_integer` field of its group in
// `post_admission_ocr`). Not marked: a target of no source, a list's key (the identity feature, the
// key field) and a field list's string column.
#[test]
fn priv1_every_place_the_kernel_reads_a_value_of_the_account_marks_its_region() {
    let package = pack(
        json!([
            ocr("text/mark", json!(rect(10, 2, 30, 8)), &[]),
            ocr("text/amount", json!(rect(50, 2, 30, 8)), &[]),
            ocr("text/other", json!(rect(90, 2, 30, 8)), &[]),
            template(
                "list/anchor",
                json!(rect(0, 20, 60, 60)),
                None,
                rect(0, 20, 60, 60)
            ),
            ocr("list/name", json!(rect(0, 0, 30, 10)), &[]),
            ocr("list/count", json!(rect(0, 0, 16, 10)), &[]),
            ocr("slot/0/name", json!(rect(100, 60, 40, 12)), &["ALPHA"]),
            ocr("slot/0/count", json!(rect(100, 72, 20, 10)), &[]),
            ocr("field/name", json!(rect(70, 20, 40, 10)), &[]),
            ocr("field/count", json!(rect(70, 32, 20, 10)), &[]),
            ocr("field/kind", json!(rect(70, 44, 40, 10)), &[])
        ]),
        json!([
            anchored_layout(
                "list/rows",
                "list/anchor",
                json!([
                    identity_feature(
                        "member",
                        json!([{"id": "alpha", "aliases": ["ALPHA"]}]),
                        Some(("list/name", 0, 0))
                    ),
                    integer_feature("count", Some(("list/count", 40, 0)))
                ])
            ),
            slots_layout(
                "select/slots",
                json!([
                    identity_feature("name", json!([{"id": "alpha", "aliases": ["ALPHA"]}]), None),
                    integer_feature("count", None)
                ]),
                &[(
                    rect(100, 60, 40, 24),
                    json!({"name": "slot/0/name", "count": "slot/0/count"})
                )],
                None
            )
        ]),
    );
    let dictionary = |path: &str| json!({"type": "dictionary_entry", "dictionary": {"path": path}});
    let declared = json!({
        "schema_version": "0.9",
        "task_id": "neutral_task",
        "resource_readings": [reading_json("mark", "text/mark")],
        "resource_adjustments": [
            {"id": "spent", "fact_key": "resource.spent", "operation_id": "buy",
             "amount": {"target_id": "text/amount", "direction": "consume"}},
            {"id": "after", "fact_key": "resource.mark", "operation_id": "buy"}
        ],
        "post_admission_ocr": {
            "mode": "fields_v1",
            "page_ids": ["item"],
            "fields": [
                field("name", "field/name", dictionary("dictionaries/names.json")),
                field("amount", "field/count",
                      json!({"type": "unsigned_integer", "min": 0, "max": 9999})),
                field("kind", "field/kind", dictionary("dictionaries/kinds.json"))
            ],
            "limits": {"max_frames": 8, "max_items": 8, "max_string_bytes": 64,
                       "max_total_bytes": 4096, "max_truth_entries": 8},
            "outcome_key": "fields_recorded"
        },
        "field_lists": [{
            "id": "list_a", "group": "row", "key_field": "name", "key_column": "id",
            "columns": [{"column": "count", "field": "amount"}, {"column": "kind", "field": "kind"}],
            "minimum_confidence_milli": 900,
            "fact_key": "inventory.list.example_kind_a", "valid_for_ms": 86_400_000,
            "start": {"forbidden": ["mark/previous"]}, "end": {"page_ids": ["item"]}
        }]
    });
    let task = PackReadingDeclarations::of_task(&declared).expect("the entry task's members");
    assert_eq!(
        package.reading_targets(&task),
        [
            "text/mark",
            "text/amount",
            "list/count",
            "slot/0/count",
            "field/count"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
    );
    let reads = package.reads(&task);
    assert_eq!(
        reads
            .regions
            .iter()
            .map(|region| (
                region.target.as_str(),
                region.kind,
                region.layout.as_deref(),
                region.reading
            ))
            .collect::<Vec<_>>(),
        [
            ("text/mark", PackReadKind::Ocr, None, true),
            ("text/amount", PackReadKind::Ocr, None, true),
            ("text/other", PackReadKind::Ocr, None, false),
            ("list/anchor", PackReadKind::TemplateSearch, None, false),
            ("list/name", PackReadKind::SelectName, None, false),
            ("list/count", PackReadKind::SelectQuantity, None, true),
            ("slot/0/name", PackReadKind::SelectName, None, false),
            ("slot/0/count", PackReadKind::SelectQuantity, None, true),
            ("field/name", PackReadKind::Ocr, None, false),
            ("field/count", PackReadKind::Ocr, None, true),
            ("field/kind", PackReadKind::Ocr, None, false),
            (
                "list/name",
                PackReadKind::SelectName,
                Some("list/rows"),
                false
            ),
            (
                "list/count",
                PackReadKind::SelectQuantity,
                Some("list/rows"),
                true
            ),
        ]
    );
    // A task that declares none of them marks only what the pack's layouts read as an integer.
    assert_eq!(
        package.reading_targets(&PackReadingDeclarations::default()),
        ["list/count", "slot/0/count"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
}

// Fix round 1, the review's must-change 3: a feature with `target` and `offset` is read by the
// kernel at its instance's origin plus the offset, with its target's size. In a
// `fixed_slots` layout the list gives one region per slot at that rectangle (a template's search
// area, a colour probe's region), which the comparison finds in a zone with its layout; in a
// `repeated_anchor` layout the anchor's match places it, known only on a frame: the list gives a
// region with no fixed rectangle, which the comparison reports as unbounded. The target's own
// region stays in the list, and the scene pastes exactly what the old function pasted: no read of
// a layout.
#[test]
fn priv1_a_feature_read_at_an_offset_is_listed_where_the_kernel_reads_it() {
    let package = pack(
        json!([
            template(
                "row/anchor",
                json!(rect(0, 70, 60, 20)),
                None,
                rect(0, 70, 60, 20)
            ),
            template(
                "slot/badge",
                json!(rect(0, 0, 10, 10)),
                None,
                rect(0, 0, 10, 10)
            ),
            color("slot/dot", rect(0, 0, 4, 4)),
            ocr("row/value", json!(rect(0, 0, 20, 8)), &[])
        ]),
        json!([
            slots_layout(
                "select/slots",
                json!([
                    passed_feature("badge", "slot/badge", 2, 3),
                    passed_feature("dot", "slot/dot", 30, 1)
                ]),
                &[
                    (rect(100, 10, 50, 20), json!({})),
                    (rect(100, 40, 50, 20), json!({}))
                ],
                None
            ),
            anchored_layout(
                "rows",
                "row/anchor",
                json!([integer_feature("value", Some(("row/value", 5, 2)))])
            )
        ]),
    );
    let at = PackReadArea::Rect;
    let reads = package.reads(&PackReadingDeclarations::default());
    assert_eq!(
        reads.regions,
        vec![
            read_region(
                "row/anchor",
                PackReadKind::TemplateSearch,
                at(rect(0, 70, 60, 20)),
                false,
                None
            ),
            read_region(
                "slot/badge",
                PackReadKind::TemplateSearch,
                at(rect(0, 0, 10, 10)),
                false,
                None
            ),
            read_region(
                "slot/dot",
                PackReadKind::Color,
                at(rect(0, 0, 4, 4)),
                false,
                None
            ),
            read_region(
                "row/value",
                PackReadKind::SelectQuantity,
                at(rect(0, 0, 20, 8)),
                true,
                None
            ),
            read_region(
                "slot/badge",
                PackReadKind::TemplateSearch,
                at(rect(102, 13, 10, 10)),
                false,
                Some("select/slots")
            ),
            read_region(
                "slot/badge",
                PackReadKind::TemplateSearch,
                at(rect(102, 43, 10, 10)),
                false,
                Some("select/slots")
            ),
            read_region(
                "slot/dot",
                PackReadKind::Color,
                at(rect(130, 11, 4, 4)),
                false,
                Some("select/slots")
            ),
            read_region(
                "slot/dot",
                PackReadKind::Color,
                at(rect(130, 41, 4, 4)),
                false,
                Some("select/slots")
            ),
            read_region(
                "row/value",
                PackReadKind::SelectQuantity,
                PackReadArea::Unbounded,
                true,
                Some("rows")
            ),
        ]
    );
    let zone_rect = rect(100, 40, 20, 20);
    let zones = public(&zones_at_base(json!([zone("z_name", zone_rect)])));
    let compared = compare_privacy_zones("neutral.test.neutral_task", &reads, &[], BASE, &zones);
    assert_eq!(
        compared.findings,
        vec![
            met(
                layout_region(
                    "slot/badge",
                    PrivacyRectKind::TemplateSearch,
                    false,
                    "select/slots"
                ),
                rect(102, 43, 10, 10),
                "z_name",
                PrivacyZoneKind::Identity,
                zone_rect
            ),
            PrivacyFinding::RegionUnbounded {
                place: layout_region("row/value", PrivacyRectKind::SelectQuantity, true, "rows"),
            },
        ]
    );
    // The scene pastes the targets' own regions, as the old function did, and no read of a layout.
    let pasted: [(PackRect, &[&str]); 4] = [
        (rect(0, 70, 60, 20), &["row/anchor"]),
        (rect(0, 0, 10, 10), &["slot/badge"]),
        (rect(0, 0, 4, 4), &["slot/dot"]),
        (rect(0, 0, 20, 8), &["row/value"]),
    ];
    assert_eq!(
        read_regions(&package, SIZE).expect("the scene's patches"),
        patches(&pasted)
    );
    assert_eq!(
        read_regions_before(&package, SIZE).expect("the old function's patches"),
        patches(&pasted)
    );
}
