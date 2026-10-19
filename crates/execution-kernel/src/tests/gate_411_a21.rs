// SPDX-License-Identifier: AGPL-3.0-only

// #411 A21 (register §1.123; exec gate key 101): an online observation admits the rows of each
// fact collection by tier. The matched page's candidate evidence (and, with target consensus,
// the recognition samples) comes first, then the rows of the explicitly requested targets and
// the page target evaluations they reuse, then every other row, each tier in production order,
// while each row still fits; admitted rows keep production order and omitted rows are counted.
// So requested targets never change the observation's status, its page or its candidate sets,
// and the observation fails only when the required evidence alone exceeds the 256-row / 64-KiB
// collection. Packages and frames are synthetic and neutral; every size is measured here.

use crate::{EvaluatedPageObservation, OnlineObservationError, PreparedPageObservation};
use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_contract::{ObservationFacts, PageObservationStatus};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const BACKGROUND: [u8; 3] = [60, 60, 60];
const MARK: [u8; 3] = [255, 0, 0];
// A colour the frame never shows: every target expecting it is evaluated and does not pass.
const ABSENT: [u8; 3] = [0, 255, 0];
const OTHER_MARK: [u8; 3] = [0, 0, 255];
// The matched page's one target. Its long name makes its evaluation row the largest page row.
const PAGE_A_MARK: &str = "page_a/the_mark_that_tells_this_page_from_every_other_page";
// A target no page and no slot uses.
const TARGET_X: &str = "target_x";
// Pages that do not match, evaluated before `page_a`, and the targets each evaluates.
const OTHER_PAGES: u32 = 16;
const OTHER_PAGE_TARGETS: u32 = 12;
// The kinds of the rows a recognition requires.
const REQUIRED_KINDS: [&str; 5] = [
    "candidate_sample",
    "candidate_projection",
    "candidate_projection_failure",
    "recognition_coverage",
    "recognition_sample",
];

fn color(id: &str, x: u32, y: u32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id,
           "region": {"x": x, "y": y, "width": 1, "height": 1}, "expected": expected})
}

// A `fixed_slots` layout on `page_a`, its `slots` slots each reading its own target for each of
// its `features` `passed` features, and those targets. Every slot target reads the same
// background pixel, so their evaluations differ only in the target's name.
fn layout(slots: u32, features: u32) -> (Value, Vec<Value>) {
    let mut targets = Vec::new();
    let mut declared = Vec::new();
    for slot in 0..slots {
        let mut read = serde_json::Map::new();
        for feature in 0..features {
            let id = format!("slot/{slot:02}/f{feature}");
            targets.push(color(&id, 40, 40, ABSENT));
            read.insert(format!("f{feature}"), json!(id));
        }
        let rect = json!({"x": (slot % 8) * 8, "y": (slot / 8) * 6, "width": 6, "height": 4});
        declared.push(json!({"rect": rect, "click": rect, "targets": read}));
    }
    let features = (0..features)
        .map(|feature| json!({"name": format!("f{feature}"), "value": "passed"}))
        .collect::<Vec<_>>();
    (
        json!({"id": "layout/slots", "page_id": "page_a", "kind": "fixed_slots",
               "features": features, "slots": declared}),
        targets,
    )
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn json_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON")
}

// An observation package (the Lab resource layout): `other_pages` pages that do not match, each
// evaluating `OTHER_PAGE_TARGETS` targets, then `page_a`, which matches the frame, with a layout
// of `slots` x `features` when `slots` is not 0, and `TARGET_X`.
fn package(other_pages: u32, slots: u32, features: u32) -> Vec<u8> {
    let mut targets = vec![
        color(PAGE_A_MARK, 0, 0, MARK),
        color(TARGET_X, 50, 30, ABSENT),
    ];
    let mut pages = Vec::new();
    for page in 0..other_pages {
        let mark = format!("page_b{page:02}/mark");
        targets.push(color(&mark, 1, 0, OTHER_MARK));
        let optional = (1..OTHER_PAGE_TARGETS)
            .map(|target| {
                let id = format!("page_b{page:02}/t{target:02}");
                targets.push(color(&id, target + 1, 1, ABSENT));
                id
            })
            .collect::<Vec<_>>();
        pages.push(json!({"id": format!("page_b{page:02}"), "required": [mark],
                          "optional": optional}));
    }
    pages.push(json!({"id": "page_a", "required": [PAGE_A_MARK]}));
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": targets
    });
    if slots > 0 {
        let (layout, slot_targets) = layout(slots, features);
        pack["targets"]
            .as_array_mut()
            .expect("pack targets")
            .extend(slot_targets);
        pack["candidate_layouts"] = json!([layout]);
    }
    zip_entries(&[
        (
            "control.json",
            json_bytes(json!({
                "game": "neutral",
                "server": "test",
                "package_id": "neutral.observation",
                "entry_task_id": "task"
            })),
        ),
        (
            "resources/manifest.json",
            json_bytes(json!({"entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            json_bytes(json!({"task_id": "task"})),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json_bytes(pack),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json_bytes(json!({"schema_version": "0.6", "pages": pages})),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json_bytes(json!({"navigation": []})),
        ),
    ])
}

// The frame: the background, with `page_a`'s mark at (0, 0).
fn frame_png() -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for index in 0..WIDTH * HEIGHT {
        pixels.extend_from_slice(if index == 0 { &MARK } else { &BACKGROUND });
    }
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

fn observe(
    package: &[u8],
    targets: &[&str],
) -> Result<EvaluatedPageObservation, OnlineObservationError> {
    let targets = targets
        .iter()
        .map(|target| (*target).to_string())
        .collect::<Vec<_>>();
    let observation = PreparedPageObservation::load("node.a", package, &targets, None)
        .unwrap_or_else(|error| {
            panic!(
                "the package is admitted for online observation: {error} ({})",
                error.cause()
            )
        });
    observation.evaluate(
        &frame_png(),
        FrameIdentity {
            kind: FrameKind::Rgb8,
            sha256: "0".repeat(64),
            width: WIDTH,
            height: HEIGHT,
        },
    )
}

fn observed(package: &[u8], targets: &[&str]) -> EvaluatedPageObservation {
    observe(package, targets)
        .unwrap_or_else(|error| panic!("observation: {error} ({})", error.cause()))
}

fn kinds(facts: &ObservationFacts) -> Vec<&str> {
    facts
        .rows
        .iter()
        .map(|row| row["kind"].as_str().expect("row kind"))
        .collect()
}

fn required_kinds(facts: &ObservationFacts) -> Vec<&str> {
    kinds(facts)
        .into_iter()
        .filter(|kind| REQUIRED_KINDS.contains(kind))
        .collect()
}

fn rows_of<'a>(facts: &'a ObservationFacts, kind: &str) -> Vec<&'a Value> {
    facts
        .rows
        .iter()
        .filter(|row| row["kind"] == kind)
        .collect()
}

// The candidate sets of an observation: its `candidate_projection` rows' projections.
fn candidate_sets(facts: &ObservationFacts) -> Vec<Value> {
    rows_of(facts, "candidate_projection")
        .into_iter()
        .map(|row| row["projection"].clone())
        .collect()
}

// With no explicit target the page rows alone exceed both collections, and still the matched
// page's candidate evidence is kept: the observation is recognized with its candidate projection
// in both collections, and the omitted page rows are counted.
#[test]
fn a21_required_candidate_evidence_is_kept_when_page_rows_overflow() {
    // The fixture, measured: without a layout the page rows alone do not fit either collection.
    let pages_only = observed(&package(OTHER_PAGES, 0, 0), &[]);
    assert!(
        pages_only.private_facts.truncated && pages_only.facts.truncated,
        "the page rows alone must exceed both collections"
    );

    let evaluated = observed(&package(OTHER_PAGES, 2, 2), &[]);
    assert_eq!(evaluated.status, PageObservationStatus::Recognized);
    assert_eq!(evaluated.projection.page, "page_a");
    for facts in [&evaluated.private_facts, &evaluated.facts] {
        assert_eq!(
            rows_of(facts, "candidate_projection").len(),
            1,
            "{:?}",
            kinds(facts)
        );
        assert!(rows_of(facts, "candidate_projection_failure").is_empty());
        assert!(facts.truncated);
        assert!(facts.omitted_count > 0);
        assert_eq!(facts.rows.len() + facts.omitted_count, facts.item_count);
        facts
            .validate()
            .expect("the collection stays within its budget");
    }
}

// The same frame with the explicit targets `TARGET_X` (no page uses it) and `PAGE_A_MARK` (the
// matched page's target) gives the same status, page, elements and candidate sets; the requested
// rows are added: `explicit_target` for `TARGET_X`, `explicit_target_uses` for `PAGE_A_MARK` and
// the page target evaluation it names.
#[test]
fn a21_explicit_targets_do_not_change_the_outcome() {
    let package = package(OTHER_PAGES, 2, 2);
    let without = observed(&package, &[]);
    let with = observe(&package, &[TARGET_X, PAGE_A_MARK]).unwrap_or_else(|error| {
        panic!(
            "explicit targets must not fail the observation: {error} ({})",
            error.cause()
        )
    });
    assert_eq!(with.status, PageObservationStatus::Recognized);
    assert_eq!(with.status, without.status);
    assert_eq!(with.projection.page, without.projection.page);
    assert_eq!(with.projection.elements, without.projection.elements);
    assert_eq!(candidate_sets(&with.facts), candidate_sets(&without.facts));
    assert_eq!(
        candidate_sets(&with.private_facts),
        candidate_sets(&without.private_facts)
    );
    assert_eq!(candidate_sets(&with.facts).len(), 1);
    for facts in [&with.private_facts, &with.facts] {
        assert!(
            rows_of(facts, "explicit_target")
                .iter()
                .any(|row| row["target"]["target_id"] == TARGET_X),
            "{:?}",
            kinds(facts)
        );
        let uses = rows_of(facts, "explicit_target_uses");
        let uses = uses
            .iter()
            .find(|row| row["target_id"] == PAGE_A_MARK)
            .unwrap_or_else(|| panic!("no explicit_target_uses row in {:?}", kinds(facts)));
        let named = uses["evaluation_uses"].as_array().expect("evaluation_uses");
        assert!(!named.is_empty());
        for index in named {
            assert!(
                rows_of(facts, "target_evaluation")
                    .iter()
                    .any(|row| row["evaluation_index"] == *index
                        && row["target"]["target_id"] == PAGE_A_MARK),
                "the page target evaluation {index} the requested target reuses is kept"
            );
        }
        assert!(facts.truncated);
    }
}

// An observation whose rows all fit is the collection filled in production order, with and
// without explicit targets: the kind sequence is the production order and nothing is omitted.
#[test]
fn a21_an_observation_that_fits_is_unchanged() {
    let package = package(0, 2, 2);
    let samples = ["candidate_sample"; 4];
    let mut without = vec!["page_evaluation", "target_evaluation"];
    without.extend(samples);
    without.push("candidate_projection");
    let mut with = vec![
        "page_evaluation",
        "target_evaluation",
        "explicit_target",
        "explicit_target_uses",
    ];
    with.extend(samples);
    with.push("candidate_projection");
    for (targets, expected) in [(Vec::new(), without), (vec![TARGET_X, PAGE_A_MARK], with)] {
        let evaluated = observed(&package, &targets);
        assert_eq!(evaluated.status, PageObservationStatus::Recognized);
        for facts in [&evaluated.private_facts, &evaluated.facts] {
            assert_eq!(kinds(facts), expected, "explicit targets {targets:?}");
            assert_eq!(facts.omitted_count, 0);
            assert!(!facts.truncated);
            assert_eq!(facts.item_count, expected.len());
        }
    }
}

// A layout whose samples alone exceed the collection ends the projection with
// `candidate_sample_record_failed`: the observation is partial and records that failure in both
// collections. The samples took the collection before any page row: the required rows kept, with
// one more sample at least as long as any of them, do not fit.
#[test]
fn a21_required_evidence_alone_over_budget_still_fails() {
    let evaluated = observe(&package(OTHER_PAGES, 64, 4), &[]).unwrap_or_else(|error| {
        panic!(
            "candidate samples over the budget make the observation partial: {error} ({})",
            error.cause()
        )
    });
    assert_eq!(evaluated.status, PageObservationStatus::Partial);
    for facts in [&evaluated.private_facts, &evaluated.facts] {
        let failures = rows_of(facts, "candidate_projection_failure");
        assert_eq!(failures.len(), 1, "{:?}", kinds(facts));
        assert_eq!(failures[0]["code"], "candidate_sample_record_failed");
        assert_eq!(failures[0]["item"], "recognition_evidence");
        assert!(rows_of(facts, "candidate_projection").is_empty());
        assert!(facts.truncated);
    }

    let facts = &evaluated.private_facts;
    let samples = rows_of(facts, "candidate_sample");
    assert!(!samples.is_empty(), "no candidate sample was kept");
    let longest: &Value = samples
        .iter()
        .copied()
        .max_by_key(|row| serde_json::to_vec(row).expect("sample row").len())
        .expect("a kept sample");
    let mut longest = longest.clone();
    // Longer than any elapsed-time member a sample row can carry.
    longest["elapsed_us"] = json!("x".repeat(64));
    let mut alone = ObservationFacts::default();
    for row in facts
        .rows
        .iter()
        .filter(|row| REQUIRED_KINDS.contains(&row["kind"].as_str().expect("row kind")))
    {
        let target_count = usize::from(row["kind"] == "candidate_sample");
        alone.push(row.clone(), target_count).expect("required row");
    }
    alone.push(longest, 1).expect("one more sample");
    assert!(
        alone.truncated,
        "the required rows alone filled the collection: one more sample does not fit"
    );
}

// The public and the controlled collection keep the same required rows.
#[test]
fn a21_public_and_controlled_collections_keep_the_same_required_rows() {
    let evaluated = observed(&package(OTHER_PAGES, 2, 2), &[]);
    let private = required_kinds(&evaluated.private_facts);
    assert_eq!(
        private,
        [
            "candidate_sample",
            "candidate_sample",
            "candidate_sample",
            "candidate_sample",
            "candidate_projection"
        ]
    );
    assert_eq!(required_kinds(&evaluated.facts), private);
}
