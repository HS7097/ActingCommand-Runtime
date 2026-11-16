// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-5ab (`contracts/display-adapter.md` section 11; the coordinator's work order of
//! 11-16 on board #392, test "presses"): on a display that maps to its package's base, a Lab
//! operation takes its coordinates in the base, observes on the frame brought to the base, and
//! sends its input to the device through the run's own exit.
//!
//! - On a 1920x1080 device a 1280x720 package's base point (640, 360) leaves as (960, 540), the base
//!   column 1279 as 1919, and a swipe from (0, 0) to (1279, 719) as one from (0, 0) to
//!   (1919, 1079), each with its duration; the records keep the base inputs, and the operation's
//!   projections are of the base's size.
//! - On a 1600x900 device (s = 5/4, which the capture side admits and the input side does not) the
//!   operation observes its page and fails at its input with `display_map_unresolved`
//!   (`input_scale_unsupported` at `display_input_map`, the frame 1600x900 and the base 1280x720),
//!   and nothing is sent.
//!
//! The device's frames show `page_a` (before any input) or `page_b` (after one) in their 3x3
//! top-left block, so the base frame's first pixel keeps the page's colour. The package is
//! #412 Lab B-3 S2's synthetic and neutral one.

use super::gate_392_da2a::{number, text};
use super::gate_412_b3_s2 as lab;
use super::*;
use actingcommand_contract::{
    ContainedLabOperationRequest, LabOperationSelection, LabOperationStage, LabProjectionHint,
    ObservationFramePlan,
};
use actingcommand_runtime_client::{RuntimeClient, VerifiedLabOperation};
use serde_json::json;

/// A Lab host whose one instance's frames and display are `width`x`height`.
fn device_host(root: &TempDir, (width, height): (u32, u32)) -> (RuntimeHost, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((width, height));
    let row = width as usize;
    *state.pixel_script.lock().expect("pixel script") = Some(Arc::new(
        move |_capture: usize, inputs: usize| -> Vec<(usize, [u8; 3])> {
            let page = if inputs > 0 { lab::PAGE_B } else { lab::PAGE_A };
            (0..3)
                .flat_map(move |line| (0..3).map(move |column| (line * row + column, page)))
                .collect()
        },
    ));
    let host = lab::start(
        root,
        FakeProvider::one(lab::ALIAS, instance_id(), Arc::clone(&state)),
    );
    (host, state)
}

/// The 1280x720 package of #412 Lab B-3 S2 (its declared operation is not run here).
fn package(root: &TempDir) -> PathBuf {
    lab::write_package(
        root,
        (1280, 720),
        json!({
            "id": lab::OPERATION,
            "from": "page_a",
            "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }),
    )
}

/// One Lab operation pressing `action`, given in the package's base, verified by the Lab client.
fn press(client: &RuntimeClient, package: &Path, action: InputAction) -> VerifiedLabOperation {
    client
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            lab::ALIAS,
            ContainedLabOperationRequest {
                package_path: package.to_str().expect("path").to_owned(),
                selection: LabOperationSelection::Coordinates { action },
                projection_hint: LabProjectionHint::default(),
                after: None,
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the client verifies the operation")
}

#[test]
fn da5ab_a_lab_press_on_a_device_three_halves_the_base_leaves_at_its_device_point() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1920, 1080));
    let package = package(&root);
    let client = lab::lab_client(&root);
    let presses = [
        (
            InputAction::Tap { x: 640, y: 360 },
            InputAction::Tap { x: 960, y: 540 },
        ),
        (
            InputAction::Tap { x: 1279, y: 0 },
            InputAction::Tap { x: 1919, y: 0 },
        ),
        (
            InputAction::Swipe {
                x1: 0,
                y1: 0,
                x2: 1279,
                y2: 719,
                duration_ms: 300,
            },
            InputAction::Swipe {
                x1: 0,
                y1: 0,
                x2: 1919,
                y2: 1079,
                duration_ms: 300,
            },
        ),
    ];
    for (base, _) in &presses {
        let verified = press(&client, &package, base.clone());
        let record = &verified.operation().record;
        assert!(record.failure.is_none(), "{base:?}: {:?}", record.failure);
        assert_eq!(
            record.prepared.action.as_ref(),
            Some(base),
            "the base input"
        );
        for (frame, projection) in [
            (
                &record.prepared.before_frame,
                &record.prepared.before_projection,
            ),
            (&record.after_frame, &record.after_projection),
        ] {
            let frame = frame.as_ref().expect("the operation's frame");
            let projection = projection.as_ref().expect("the frame's projection");
            assert_eq!(
                (frame.observation.width(), frame.observation.height()),
                (1920, 1080),
                "the device frame"
            );
            assert_eq!(
                (
                    projection.projection.frame.width,
                    projection.projection.frame.height
                ),
                (1280, 720),
                "the projection on the base"
            );
        }
    }
    drop(client);
    host.close().expect("close host");
    assert_eq!(
        *state.input_actions.lock().expect("fake input actions"),
        presses
            .iter()
            .map(|(_, device)| device.clone())
            .collect::<Vec<_>>(),
        "the device received each base input at its device point"
    );
}

#[test]
fn da5ab_a_lab_press_on_a_device_five_quarters_the_base_sends_nothing() {
    let root = TempDir::new().expect("tempdir");
    let (host, state) = device_host(&root, (1600, 900));
    let package = package(&root);
    let client = lab::lab_client(&root);
    let verified = press(&client, &package, InputAction::Tap { x: 640, y: 360 });
    let record = &verified.operation().record;
    let projection = record
        .prepared
        .before_projection
        .as_ref()
        .expect("the page was observed");
    assert_eq!(projection.projection.page, "neutral/page_a");
    let failure = record.failure.as_ref().expect("the operation failed");
    assert_eq!(failure.code, "display_map_unresolved", "{failure:?}");
    assert_eq!(failure.stage, LabOperationStage::Input, "{failure:?}");
    assert!(!record.input_returned, "the input never reached its input");
    assert_eq!(record.effect, EffectDisposition::NotPerformed);
    let outcome = verified.receipt().outcome();
    assert_eq!(outcome.code().as_str(), "display_map_unresolved");
    assert_eq!(
        text(outcome, "display_problem"),
        Some("input_scale_unsupported")
    );
    assert_eq!(text(outcome, "stage"), Some("display_input_map"));
    assert_eq!(number(outcome, "frame_width"), Some(1600));
    assert_eq!(number(outcome, "frame_height"), Some(900));
    assert_eq!(number(outcome, "base_width"), Some(1280));
    assert_eq!(number(outcome, "base_height"), Some(720));
    drop(client);
    host.close().expect("close host");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0, "no input");
    assert!(
        state
            .input_actions
            .lock()
            .expect("fake input actions")
            .is_empty()
    );
}
