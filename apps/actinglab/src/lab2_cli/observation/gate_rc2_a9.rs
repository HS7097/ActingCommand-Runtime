// SPDX-License-Identifier: AGPL-3.0-only

// #411 A9 (#411 comment 6252521723, reproduced in comment 6253170525; register §1.101; exec gate
// key 86): `observe` on a frame no page matches answers `unknown` (page-projection.md: "No match
// yields `state=unknown`"). Its near-miss `candidates` are a transport member, not the identity,
// so Min removes them from the end, the lowest score last, until the answer fits its hard limit,
// marked `candidates_truncated` with `candidates_omitted_count`. An answer that fits keeps every
// near miss unmarked, and the Normal view (`--verbose`) keeps the whole list. Every expected
// value comes from the slice's requirement and that contract.

use super::*;
use actingcommand_ledger::{MIN_PROJECTION_HARD_LIMIT_BYTES, MIN_PROJECTION_SOFT_LIMIT_BYTES};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const PAGES: [&str; 5] = ["entry_page", "page_a", "page_b", "page_c", "page_d"];

/// The projection of a frame that no page of the package matches.
fn unmatched_projection() -> PageProjection {
    let catalog = shared::ProjectionCatalog::from_resources(
        &json!({"coordinate_space": {"width": WIDTH, "height": HEIGHT}, "targets": [{"id": "shared_header"}]}),
        &json!({"pages": PAGES.map(|id| json!({"id": id}))}),
        &json!({"navigation": []}),
    )
    .unwrap();
    shared::project(
        ProjectionInput {
            frame: FrameIdentity {
                kind: shared::FrameKind::Rgb8,
                sha256: "3e7a".repeat(16),
                width: WIDTH,
                height: HEIGHT,
            },
            matched_pages: vec![],
            elements: vec![],
            missing: vec![],
            fields: vec![],
        },
        &shared::VerifiedProjectionMetadata::unannotated(catalog),
    )
    .unwrap()
}

/// One near miss as offline `observe` lists it (`lab2_page_candidates`).
fn near_miss(
    id: &str,
    score: f64,
    required_passed: usize,
    required_total: usize,
    message: String,
) -> Value {
    json!({
        "id": id,
        "matched": false,
        "passed": false,
        "score": score,
        "required_passed": required_passed,
        "required_total": required_total,
        "forbidden_passed": 0,
        "message": message
    })
}

/// The reason of a page whose required targets passed and whose any_of group of five failed.
fn any_of_failed(page: &str) -> String {
    let targets = [
        "list_header_title",
        "list_header_subtitle",
        "confirm_button_enabled",
        "confirm_button_disabled",
        "close_button_visible",
    ]
    .map(|target| format!("{page}_{target}"));
    format!("any_of group failed: {}", targets.join(", "))
}

fn required_failed(target: &str) -> String {
    format!("required target failed: {target}")
}

/// Four near misses, sorted by score, most of which name a failed any_of group.
fn long_near_misses() -> Vec<Value> {
    vec![
        near_miss("page_a", 0.75, 3, 3, any_of_failed("page_a")),
        near_miss("page_b", 0.6, 3, 3, any_of_failed("page_b")),
        near_miss(
            "page_c",
            0.5,
            1,
            2,
            required_failed("page_c_list_header_title"),
        ),
        near_miss("page_d", 0.25, 1, 1, any_of_failed("page_d")),
    ]
}

/// Four near misses, sorted by score, each naming one failed required target.
fn short_near_misses() -> Vec<Value> {
    vec![
        near_miss("page_a", 0.75, 3, 4, required_failed("page_a_title")),
        near_miss("page_b", 0.6, 3, 5, required_failed("page_b_title")),
        near_miss("page_c", 0.5, 1, 2, required_failed("page_c_title")),
        near_miss("page_d", 0.25, 1, 4, required_failed("page_d_title")),
    ]
}

/// The members offline `observe` sends for a frame no page matches (`run_observe`): the near
/// misses, which the suspicion repeats (`lab2_observation_suspicion`), and the wake hint.
fn unknown_payload(near_misses: &[Value]) -> Value {
    json!({
        "req_id": "req-18dd54d1a2b3c4d5-1",
        "state": "unknown",
        "instance": "node.a",
        "page": "unknown",
        "matched": false,
        "standby": true,
        "frame_age_ms": 0,
        "backend": "scene_file",
        "frame_source": {"kind": "scene_file"},
        "targets": [],
        "actions": [],
        "arbitration": {"authority": "isolated_offline", "decision": "not_applicable", "persistent_state": false},
        "candidates": near_misses,
        "recovery_hint": {"action": "wake_safe_point", "point": {"x": 300, "y": 2}, "note": "CLI does not click automatically"},
        "suspicion": {"reason": "low_page_margin", "candidates": near_misses},
        "frame_coordinates": {"device_pixels": [WIDTH, HEIGHT], "orientation": "landscape", "applies_to": ["tap", "tap_rect", "swipe"]}
    })
}

fn encoded_len(value: &Value) -> usize {
    serde_json::to_vec(value).unwrap().len()
}

#[test]
fn a9_min_unknown_page_over_the_hard_limit_answers_unknown_with_its_near_misses_cut() {
    let near_misses = long_near_misses();

    let output = project(
        unknown_payload(&near_misses),
        unmatched_projection(),
        &FlagArgs::default(),
        false,
    )
    .expect("Min answers an unknown page instead of failing");

    // The scene: with every near miss kept, the answer is over the hard limit.
    let mut with_every_near_miss = output.clone();
    with_every_near_miss["candidates"] = json!(near_misses);
    assert!(encoded_len(&with_every_near_miss) > MIN_PROJECTION_HARD_LIMIT_BYTES);

    assert!(encoded_len(&output) <= MIN_PROJECTION_HARD_LIMIT_BYTES);
    assert_eq!(output["observation"]["state"], "unknown");
    let kept = output["candidates"]
        .as_array()
        .expect("the near misses stay an array");
    assert_eq!(kept[0]["id"], "page_a");
    assert_eq!(output["candidates_truncated"], true);
    let omitted = output["candidates_omitted_count"]
        .as_u64()
        .expect("the number of near misses removed");
    assert_eq!(kept.len() as u64 + omitted, 4);
}

#[test]
fn a9_min_unknown_page_within_the_limit_keeps_every_near_miss_unmarked() {
    let near_misses = short_near_misses();

    let output = project(
        unknown_payload(&near_misses),
        unmatched_projection(),
        &FlagArgs::default(),
        false,
    )
    .expect("Min answers an unknown page");

    // The scene: the answer is over Min's 1 KiB target, so Min reduces it, and fits the hard
    // limit with every near miss.
    let length = encoded_len(&output);
    assert!(length > MIN_PROJECTION_SOFT_LIMIT_BYTES);
    assert!(length <= MIN_PROJECTION_HARD_LIMIT_BYTES);
    assert_eq!(output["observation"]["state"], "unknown");
    assert_eq!(output["candidates"], json!(near_misses));
    assert!(output.get("candidates_truncated").is_none());
    assert!(output.get("candidates_omitted_count").is_none());
}

#[test]
fn a9_verbose_unknown_page_keeps_every_near_miss() {
    let near_misses = long_near_misses();

    let output = project(
        unknown_payload(&near_misses),
        unmatched_projection(),
        &FlagArgs::default(),
        true,
    )
    .expect("the Normal view answers an unknown page");

    assert_eq!(output["observation"]["state"], "unknown");
    assert_eq!(output["candidates"], json!(near_misses));
    assert!(output.get("candidates_truncated").is_none());
    assert!(output.get("candidates_omitted_count").is_none());
}
