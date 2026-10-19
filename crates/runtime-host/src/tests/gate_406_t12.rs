// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #406 T12 (host gate key 123; register §1.136; plan Q13, Q15, Q16; brief T12
//! requirements 1, 4 and 5): the host hands each step of a run the `touch` keys in effect, and
//! the landing history of single-region presses lives in its memory, per instance and per box,
//! bounded by `touch.history.*`, empty after a restart. Every expected point comes from
//! [`reference_candidates`], "The draw" written again from the brief's text, and the least-recent
//! rule as Alice ruled it (2026-10-14), never from the code.

use super::*;
use crate::touch_history::{TouchHistory, TouchHistoryLimits};
use actingcommand_contract::{
    ConfigSettingChange, InputSamplingEvidence, InputSamplingParameters, InputSamplingRegion,
    RuntimeConfigManifest,
};
use actingcommand_runtime_config::touch::{CANDIDATE_COUNT, SIGMA_RATIO_MILLI};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::json;

const ALIAS: &str = "neutral.instance";

/// "The draw" of brief T12 (normative), written from its text alone: `count` successive
/// centre-hotspot points of the box `(x, y, width, height)` from `action_seed`, in draw order.
/// xorshift64 from the normalized seed; each coordinate's draw sums twelve 24-bit uniforms; a
/// point outside the box is drawn again at most `max_redraws` times, then the centre is taken; all
/// arithmetic in i128, truncating toward zero.
pub(super) fn reference_candidates(
    action_seed: u64,
    (x, y, width, height): (i32, i32, i32, i32),
    sigma_ratio_milli: u32,
    max_redraws: u32,
    count: u32,
) -> Vec<(i32, i32)> {
    let mut state = if action_seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        action_seed
    };
    let mut gaussian_milli = || {
        let mut sum: i128 = 0;
        for _ in 0..12 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            sum += i128::from(state >> 40);
        }
        ((2 * sum - 12 * ((1_i128 << 24) - 1)) * 1000) / (2 * (1_i128 << 24))
    };
    let (left, top) = (i128::from(x), i128::from(y));
    let (width, height) = (i128::from(width), i128::from(height));
    let (cx, cy) = (left + width / 2, top + height / 2);
    let sigma = i128::from(sigma_ratio_milli);
    let point = |px: i128, py: i128| {
        (
            i32::try_from(px).expect("x on the screen"),
            i32::try_from(py).expect("y on the screen"),
        )
    };
    let mut candidates = Vec::new();
    for _ in 0..count {
        let mut landed = point(cx, cy);
        for _ in 0..=max_redraws {
            let zx = gaussian_milli();
            let zy = gaussian_milli();
            let px = cx + zx * sigma * width / 1_000_000;
            let py = cy + zy * sigma * height / 1_000_000;
            if (left..left + width).contains(&px) && (top..top + height).contains(&py) {
                landed = point(px, py);
                break;
            }
        }
        candidates.push(landed);
    }
    candidates
}

fn region(x: i32, y: i32, width: i32, height: i32) -> InputSamplingRegion {
    InputSamplingRegion::new(x, y, width, height).expect("sampling region")
}

fn least_recent(
    sigma_ratio_milli: u32,
    max_redraws: u32,
    candidate_count: u32,
    chosen: u32,
) -> InputSamplingParameters {
    InputSamplingParameters {
        sigma_ratio_milli,
        max_redraws,
        candidate_count: Some(candidate_count),
        chosen_candidate: Some(chosen),
    }
}

/// The five `touch` rows as actingd assembles them from the shipped defaults file.
fn touch_manifest() -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters =
        render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
            .expect("the defaults resolve")
            .into_iter()
            .filter(|parameter| parameter.key.starts_with("touch."))
            .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 5, "{parameters:?}");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn start(root: &TempDir, manifest: Option<RuntimeConfigManifest>) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host_config = match manifest {
        Some(manifest) => config(root).with_config_manifest(manifest),
        None => config(root),
    };
    RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(ALIAS, instance_id(), state)),
    )
    .expect("runtime host")
}

/// One direct run of `package` on the instance: its one press and the press's sampling evidence.
fn press_once(
    client: &mut TestClient,
    root: &TempDir,
    package: &[u8],
) -> (InputAction, InputSamplingEvidence) {
    let package_path = root.path().join("touch-task.zip");
    fs::write(&package_path, package).expect("write package");
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
                .expect("contained task request"),
        ),
    );
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let events = projected_events(
        client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let effect = events
        .iter()
        .find(|event| event.event_type == EventType::TaskEffectIntent)
        .expect("the run's effect intent");
    let ProjectionPayload::Full(payload) = &effect.payload else {
        panic!("the effect intent keeps its full payload")
    };
    let EventPayload::Task(TaskPayload::Semantic(payload)) = payload.as_ref() else {
        panic!("the effect intent is a typed task payload")
    };
    let TaskSemanticFact::EffectIntent { action, .. } = payload.fact() else {
        panic!("the effect intent holds its action")
    };
    let sampling = payload.sampling().expect("the press's sampling evidence");
    (action.clone(), sampling.clone())
}

fn configure(client: &mut TestClient, changes: &[(&str, serde_json::Value)]) {
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes: changes
            .iter()
            .map(|(key, value)| ConfigSettingChange {
                key: (*key).to_owned(),
                value: Some(value.clone()),
            })
            .collect(),
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// The neutral package whose one operation presses the 1 × 1 box at (1, 0).
fn one_point_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        true,
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "click":{"kind":"rect","x":1,"y":0,"width":1,"height":1},
                 "unguarded_trusted_coordinate":true,
                 "retryable":false
             }]
        }"#,
    )
}

// Requirement 4 (plan Q13): the host reads the `touch` keys at each step. A run presses the box
// with the shipped defaults (250 ‰, 8 redraws, 4 candidates); after one instruction sets
// `sigma_ratio_milli` 500 and `candidate_count` 2, the next run's press is drawn and recorded
// with them, and its choice reads the first run's point from the instance's history.
#[test]
fn t12_the_host_hands_the_kernel_the_touch_keys_in_effect() {
    let root = TempDir::new().expect("tempdir");
    let host = start(&root, Some(touch_manifest()));
    let mut client = TestClient::connect(&host);
    let package = neutral_region_contained_task_package(true);
    let rect = (0, 0, 2, 1);

    let (first, sampling) = press_once(&mut client, &root, &package);
    let candidates = reference_candidates(sampling.action_seed(), rect, 250, 8, 4);
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::CenterHotspotLeastRecentV1
    );
    assert_eq!(sampling.parameters(), Some(&least_recent(250, 8, 4, 0)));
    assert_eq!(
        first,
        InputAction::Tap {
            x: candidates[0].0,
            y: candidates[0].1
        }
    );

    configure(
        &mut client,
        &[(SIGMA_RATIO_MILLI, json!(500)), (CANDIDATE_COUNT, json!(2))],
    );
    let (second, sampling) = press_once(&mut client, &root, &package);
    let candidates = reference_candidates(sampling.action_seed(), rect, 500, 8, 2);
    // The history holds the first run's point only: the first candidate elsewhere lands, else
    // (both on that point, used alike) the first.
    let used = match first {
        InputAction::Tap { x, y } => (x, y),
        other => panic!("a tap, got {other:?}"),
    };
    let chosen = candidates
        .iter()
        .position(|candidate| *candidate != used)
        .unwrap_or(0);
    assert_eq!(
        sampling.parameters(),
        Some(&least_recent(
            500,
            8,
            2,
            u32::try_from(chosen).expect("index")
        ))
    );
    assert_eq!(
        second,
        InputAction::Tap {
            x: candidates[chosen].0,
            y: candidates[chosen].1
        }
    );
    drop(client);
    host.close().expect("close runtime host");
}

// Requirement 4 and plan Q16: the history is per instance and per box. Consecutive presses on
// one box choose no point twice while an unused candidate is drawn, and then the point used
// longest ago; another instance, and another box of the same instance holding the same pixel,
// start unused; `max_boxes` forgets the box of an instance unused longest, without touching
// another instance's boxes.
#[test]
fn t12_the_landing_history_is_per_instance_and_per_box() {
    let limits = TouchHistoryLimits {
        points_per_box: 16,
        max_boxes: 1_024,
    };
    let mut history = TouchHistory::default();
    let small = region(10, 10, 20, 20);
    let candidates = [(15, 15), (16, 15), (15, 16), (20, 20)];
    let chosen = (0..4)
        .map(|_| history.choose("instance_a", &small, &candidates, limits))
        .collect::<Vec<_>>();
    assert_eq!(chosen, [0, 1, 2, 3], "an unused candidate lands first");
    assert_eq!(
        history.choose("instance_a", &small, &candidates, limits),
        0,
        "every point used: the one used longest ago"
    );
    assert_eq!(
        history.choose("instance_b", &small, &candidates, limits),
        0,
        "another instance starts unused"
    );
    let large = region(10, 10, 30, 30);
    assert_eq!(
        history.choose("instance_a", &large, &[(15, 15), (30, 30)], limits),
        0,
        "another box holding the same pixel starts unused"
    );
    assert_eq!(
        history.points("instance_a", &small),
        [(16, 15), (15, 16), (20, 20), (15, 15)]
    );
    assert_eq!(history.points("instance_b", &small), [(15, 15)]);

    let two_boxes = TouchHistoryLimits {
        points_per_box: 16,
        max_boxes: 2,
    };
    let boxes = [region(0, 0, 4, 4), region(10, 0, 4, 4), region(20, 0, 4, 4)];
    for (index, kept) in boxes.iter().enumerate() {
        let x = i32::try_from(index).expect("index") * 10;
        assert_eq!(
            history.choose("instance_c", kept, &[(x, 0), (x + 1, 0)], two_boxes),
            0
        );
    }
    assert!(
        history.points("instance_c", &boxes[0]).is_empty(),
        "the box unused longest is forgotten"
    );
    assert_eq!(history.points("instance_c", &boxes[1]), [(10, 0)]);
    assert_eq!(history.points("instance_c", &boxes[2]), [(20, 0)]);
    assert_eq!(
        history.choose("instance_c", &boxes[0], &[(0, 0), (1, 0)], two_boxes),
        0,
        "a forgotten box's point counts as unused"
    );
    assert_eq!(
        history.points("instance_a", &small).len(),
        4,
        "another instance keeps its boxes"
    );
}

// Requirement 4: with `points_per_box` 2 a box keeps its two newest points; the oldest is
// forgotten and counts as unused again.
#[test]
fn t12_a_box_keeps_at_most_points_per_box_and_the_oldest_is_forgotten() {
    let limits = TouchHistoryLimits {
        points_per_box: 2,
        max_boxes: 1_024,
    };
    let mut history = TouchHistory::default();
    let kept = region(0, 0, 10, 10);
    let (p1, p2, p3, p4) = ((1, 1), (2, 2), (3, 3), (4, 4));
    assert_eq!(history.choose("instance_a", &kept, &[p1, p2], limits), 0);
    assert_eq!(history.choose("instance_a", &kept, &[p2, p1], limits), 0);
    assert_eq!(history.choose("instance_a", &kept, &[p3, p1], limits), 0);
    assert_eq!(history.points("instance_a", &kept), [p2, p3]);
    assert_eq!(
        history.choose("instance_a", &kept, &[p1, p4], limits),
        0,
        "the forgotten point is unused again"
    );
    assert_eq!(history.points("instance_a", &kept), [p3, p1]);
}

// Requirement 4: the history is memory only. A host closed and started again on the same root
// starts with an empty history, so the first press of the 1 × 1 box takes its first candidate
// although its point was used before the restart.
#[test]
fn t12_a_restart_starts_with_an_empty_history() {
    let root = TempDir::new().expect("tempdir");
    let package = one_point_package();
    let point = region(1, 0, 1, 1);

    let host = start(&root, None);
    let mut client = TestClient::connect(&host);
    let (action, sampling) = press_once(&mut client, &root, &package);
    assert_eq!(action, InputAction::Tap { x: 1, y: 0 });
    assert_eq!(sampling.parameters(), Some(&least_recent(250, 8, 4, 0)));
    assert_eq!(
        host.touch_history_points_for_test(ALIAS, &point)
            .expect("the history"),
        [(1, 0)]
    );
    drop(client);
    host.close().expect("close runtime host");

    let host = start(&root, None);
    assert!(
        host.touch_history_points_for_test(ALIAS, &point)
            .expect("the history")
            .is_empty(),
        "a restart starts with an empty history"
    );
    let mut client = TestClient::connect(&host);
    let (action, sampling) = press_once(&mut client, &root, &package);
    assert_eq!(action, InputAction::Tap { x: 1, y: 0 });
    assert_eq!(sampling.parameters(), Some(&least_recent(250, 8, 4, 0)));
    assert_eq!(
        host.touch_history_points_for_test(ALIAS, &point)
            .expect("the history"),
        [(1, 0)]
    );
    drop(client);
    host.close().expect("close runtime host");
}
