// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #387 D1c-F, the kernel rows of the two codes re-coded by their source.
//
// A pack's single-touch drag with a vertical brake is a declaration, decided without the screen:
// a corner box that leaves no room for the 100 px brake above it is refused at admission with
// `contained_task_operation_invalid` (`declaration_error`), and the same declaration handed to a
// run past admission is refused with that code when its action is built, before any input.
//
// The stability sample tells its two causes apart: frame data that does not add up is the
// capture's `contained_task_frame_invalid` (`environment_fault`); a region that does not fit the
// frame, which has the task resolution, is the control declaration's
// `contained_task_control_invalid` (`declaration_error`), the code admission gives the same
// region.

use super::*;
use actingcommand_contract::outcome::LadderClass;
use actingcommand_device::CaptureBackendName;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

// The task resolution: tall enough for a corner box one brake distance below the top edge.
const WIDTH: u32 = 4;
const HEIGHT: u32 = 128;
const BRAKE_DISTANCE_PX: i32 = 100;
// A frame's top-left pixel names its page.
const PAGES: [(&str, [u8; 3]); 2] = [("home", [255, 0, 0]), ("result", [255, 0, 255])];

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    zip.finish().expect("finish zip").into_inner()
}

// A navigable package (control v2, operation schema 0.9) with one operation from `home` to
// `result`: a drag from a box near the bottom through a corner box whose top edge is `corner_y`.
fn swipe_package(corner_y: i32) -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.swipe.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 4
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": ["home"],
        "target_page": "result",
        "operations": [{
            "id": "drag_list",
            "from": "home",
            "to": "result",
            "click": {
                "kind": "single_touch_drag_with_vertical_brake_v1",
                "from_rect": {"x": 3, "y": 110, "width": 1, "height": 1},
                "corner_rect": {"x": 0, "y": corner_y, "width": 1, "height": 1},
                "horizontal_duration_ms": 200,
                "corner_hold_ms": 150,
                "brake_distance_px": BRAKE_DISTANCE_PX,
                "brake_duration_ms": 200
            },
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    let targets = PAGES
        .iter()
        .map(|(page, color)| {
            json!({
                "type": "color",
                "id": format!("page/{page}"),
                "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                "expected": color
            })
        })
        .collect::<Vec<_>>();
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({
                "id": format!("neutral/{page}"),
                "required": [format!("page/{page}")],
                "optional": [],
                "forbidden": []
            })
        })
        .collect::<Vec<_>>();
    zip_entries(&[
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": WIDTH, "height": HEIGHT},
                "defaults": {"color_max_distance": 0.0},
                "targets": targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": pages}),
        ),
    ])
}

// Admission takes the package alone (Workflow #336 PKG-S5).
fn admit(corner_y: i32) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &swipe_package(corner_y))
}

// The runtime of a run that never leaves `home`: it counts the inputs that reach it.
#[derive(Default)]
struct CountingRuntime {
    inputs: usize,
}

impl ContainedTaskRuntime for CountingRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let mut pixels = vec![0; (WIDTH * HEIGHT * 3) as usize];
        pixels[..3].copy_from_slice(&PAGES[0].1);
        Ok(Frame::from_pixels(
            WIDTH,
            HEIGHT,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("home frame")
        .into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }
}

// The declaration alone decides the brake: a corner box whose top edge lies one brake distance
// below the top of the frame is admitted, one a pixel higher is refused at admission with the
// class-3 declaration code.
#[test]
fn d1cf_admission_refuses_a_pack_swipe_whose_corner_leaves_no_room_for_the_brake() {
    if let Err(error) = admit(BRAKE_DISTANCE_PX) {
        panic!("a corner one brake distance below the top is admitted: {error}");
    }
    let Err(error) = admit(BRAKE_DISTANCE_PX - 1) else {
        panic!("a corner within the brake distance of the top is refused at admission");
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskOperationInvalid.as_str()
    );
    assert_eq!(
        ExecutionKernelCode::ContainedTaskOperationInvalid.ladder_class(),
        LadderClass::DeclarationError
    );
}

// The same declaration in a task that passed no admission: the run refuses it when the action is
// built, with the same class-3 code, and no input reaches the runtime.
#[test]
fn d1cf_a_pack_swipe_past_admission_is_refused_before_any_input() {
    let mut task = admit(BRAKE_DISTANCE_PX).expect("the admitted package");
    let click = task.program.operations[0]
        .click
        .as_mut()
        .expect("the drag operation's click");
    click.corner_rect = Some(ClickRect {
        x: 0,
        y: BRAKE_DISTANCE_PX - 1,
        width: 1,
        height: 1,
    });
    let mut runtime = CountingRuntime::default();
    let error = match task.run(&mut runtime) {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("expected the declaration's refusal, got {other:?}"),
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskOperationInvalid.as_str()
    );
    assert_eq!(
        ExecutionKernelCode::ContainedTaskOperationInvalid.ladder_class(),
        LadderClass::DeclarationError
    );
    assert_eq!(runtime.inputs, 0);
}

// A stability sample of a 3x1 frame: frame data one byte short is the capture's failure; a
// region past the frame's right edge, or an empty one, is the declaration's.
#[test]
fn d1cf_a_stability_sample_tells_frame_data_from_a_region_that_does_not_fit() {
    let declaration = |x: u32, width: u32| -> StabilityTerminationDeclaration {
        serde_json::from_value(json!({
            "region": {"x": x, "y": 0, "width": width, "height": 1},
            "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
            "consecutive_unchanged_threshold": 2,
            "max_steps": 4
        }))
        .expect("stability declaration")
    };
    let frame = || {
        Frame::from_pixels(
            3,
            1,
            [[1, 2, 3], [10, 11, 12], [4, 5, 6]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("stability frame")
    };
    stability_sample(&frame(), &declaration(1, 2)).expect("a region inside the frame");

    let mut short = frame();
    short.pixels.pop();
    let error =
        stability_sample(&short, &declaration(1, 2)).expect_err("frame data that does not add up");
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskFrameInvalid.as_str()
    );
    assert_eq!(
        ExecutionKernelCode::ContainedTaskFrameInvalid.ladder_class(),
        LadderClass::EnvironmentFault
    );

    for (x, width) in [(2, 2), (1, 0)] {
        let error = stability_sample(&frame(), &declaration(x, width))
            .expect_err("a region that does not fit the frame");
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::ContainedTaskControlInvalid.as_str(),
            "region x={x} width={width}"
        );
    }
    assert_eq!(
        ExecutionKernelCode::ContainedTaskControlInvalid.ladder_class(),
        LadderClass::DeclarationError
    );
}
