// SPDX-License-Identifier: AGPL-3.0-only

// HS-4 (#402; design `rd-1031/backend-switch/model-backend-switch.md` §5; register §1.268): every
// input a package's run makes names the frame its step decided on, so an instance whose input is
// `nemu_ipc` never refuses a package's input with `nemu_input_frame_required`. That refusal is
// left for inputs from outside packages (direct CLI or MCP inputs, the Lab's own presses).
//
// The device is the scripted-frame runtime (EXEC-I4) behind a runtime that commits every frame it
// captures, as the host does (`committed_input_frame`): each capture gets its own frame reference.
// It keeps every input with the frame reference the kernel handed it and the number of frames
// captured by then. Two packages run on it, every press of them blind
// (`unguarded_trusted_coordinate`, the pack taps §5 calls blind):
//
// - a page graph (`navigable_route`) through six pages, one operation of each click kind a page
//   graph runs: a box tap, a box long press, a drag, the drag with a vertical brake and the path
//   drag;
// - a step plan (`linear_steps`) through four pages: a point tap, a box long press and a drag.
//
// A 200 x 200 frame's first pixel names its page; the device shows page `n` after `n` inputs.
// The runs play on the scripted clock; nothing here sleeps or reads a clock.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{IdentifierIssuer, InputFrameReference};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const SIDE: u32 = 200;
const PAGES: [(&str, [u8; 3]); 6] = [
    ("p0", [255, 0, 0]),
    ("p1", [0, 255, 0]),
    ("p2", [0, 0, 255]),
    ("p3", [255, 255, 0]),
    ("p4", [255, 0, 255]),
    ("p5", [0, 255, 255]),
];

/// The scripted device, every frame of which the runtime commits as the host does.
struct FramedDevice {
    script: ScriptedRuntime,
    ids: IdentifierIssuer,
    captured: Vec<InputFrameReference>,
    // Each input, the frame reference it named and the number of frames captured before it.
    inputs: Vec<(InputAction, Option<InputFrameReference>, usize)>,
}

impl FramedDevice {
    fn new() -> Self {
        let screens = PAGES
            .iter()
            .map(|(_, colour)| vec![page_frame(*colour)])
            .collect();
        Self {
            script: ScriptedRuntime::new(screens),
            ids: IdentifierIssuer::new().expect("identifier issuer"),
            captured: Vec::new(),
            inputs: Vec::new(),
        }
    }

    /// Every input named a frame the runtime committed before it.
    fn assert_every_input_names_its_frame(&self, case: &str) {
        for (index, (action, reference, captured)) in self.inputs.iter().enumerate() {
            let reference = reference
                .unwrap_or_else(|| panic!("{case}: input {index} ({action:?}) names no frame"));
            assert!(
                self.captured[..*captured].contains(&reference),
                "{case}: input {index} ({action:?}) names a frame not captured before it"
            );
        }
    }

    fn actions(&self) -> Vec<&InputAction> {
        self.inputs.iter().map(|(action, _, _)| action).collect()
    }
}

impl ContainedTaskRuntime for FramedDevice {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let mut observed = self.script.capture()?;
        let frame_id = *self
            .ids
            .mint_frame_id()
            .map_err(|_| "a frame id")?
            .transport();
        let reference = InputFrameReference {
            frame_id,
            width: observed.frame.width,
            height: observed.frame.height,
        };
        observed.input_reference = Some(reference);
        self.captured.push(reference);
        Ok(observed)
    }

    fn committed_input_frame(
        &mut self,
        reference: InputFrameReference,
    ) -> Result<Option<InputFrameContext>, Self::Error> {
        if self.captured.contains(&reference) {
            Ok(Some(InputFrameContext::for_test(reference)))
        } else {
            Ok(None)
        }
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push((
            action.clone(),
            frame.as_ref().map(InputFrameContext::reference),
            self.captured.len(),
        ));
        self.script.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.script.record(trace)
    }

    fn sampling_supported(&self) -> bool {
        self.script.sampling_supported()
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.script.step_pacing()
    }

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }
}

// A frame of the page of colour `colour`: its first pixel, black elsewhere.
fn page_frame(colour: [u8; 3]) -> ScriptedFrame {
    let mut pixels = vec![[0_u8; 3]; (SIDE * SIDE) as usize];
    pixels[0] = colour;
    ScriptedFrame::rgb(SIDE, SIDE, &pixels)
}

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

fn rect(x: u32, y: u32, width: u32, height: u32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

// A blind press `id` from `from` to `to`.
fn press(id: &str, from: &str, to: &str, click: Value) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": click,
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

// A package (control v2, operation schema 0.9) from `p0` to `target` through `operations`, a step
// plan (`linear_steps`, its `max_steps` the operation count in the task and the control) when
// `linear` says so, else a page graph (`navigable_route`); one colour target and one page per page
// of `PAGES`. It is admitted as a package's admission admits it.
fn admit(target: &str, operations: Vec<Value>, linear: bool) -> PreparedContainedTask {
    let steps = operations.len();
    let (mode, entry_page, max_steps) = if linear {
        ("linear_steps", json!("p0"), steps)
    } else {
        ("navigable_route", json!(["p0"]), steps + 2)
    };
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.input.frames",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": SIDE, "height": SIDE},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5_000,
        "timeout_ms": 60_000,
        "max_steps": max_steps
    });
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": SIDE, "height": SIDE},
        "entry_page": entry_page,
        "target_page": target,
        "operations": operations
    });
    if linear {
        task["max_steps"] = json!(steps);
    }
    let targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({
                "type": "color",
                "id": format!("page/{page}"),
                "region": rect(0, 0, 1, 1),
                "expected": colour
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
    let package = zip_entries(&[
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
                "coordinate_space": {"width": SIDE, "height": SIDE},
                "defaults": {"color_max_distance": 0.0},
                "targets": targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version": "0.3", "pages": pages}),
        ),
    ]);
    PreparedContainedTask::load("neutral.instance", &package)
        .unwrap_or_else(|error| panic!("{mode}: the package is admitted: {error}"))
}

fn long_press() -> Value {
    json!({"kind": "long_press", "x": 60, "y": 60, "width": 10, "height": 10, "duration_ms": 500})
}

fn drag() -> Value {
    json!({
        "kind": "drag",
        "from_rect": rect(20, 100, 10, 10),
        "to_rect": rect(150, 100, 10, 10),
        "duration_ms": 300
    })
}

// Every input of a page graph's run and of a step plan's run names a frame the runtime committed
// before it, whatever the click: a box tap, a box long press, a drag, the drag with a vertical
// brake and the path drag, every one of them blind.
#[test]
fn hs4_pack_inputs_always_carry_frame() {
    let graph = admit(
        "p5",
        vec![
            press(
                "tap",
                "p0",
                "p1",
                json!({"kind": "rect", "x": 50, "y": 50, "width": 10, "height": 10}),
            ),
            press("long_press", "p1", "p2", long_press()),
            press("drag", "p2", "p3", drag()),
            press(
                "brake_drag",
                "p3",
                "p4",
                json!({
                    "kind": "single_touch_drag_with_vertical_brake_v1",
                    "from_rect": rect(170, 180, 5, 5),
                    "corner_rect": rect(20, 150, 5, 5),
                    "horizontal_duration_ms": 200,
                    "corner_hold_ms": 150,
                    "brake_distance_px": 100,
                    "brake_duration_ms": 200
                }),
            ),
            press(
                "path_drag",
                "p4",
                "p5",
                json!({
                    "kind": "single_touch_path_drag_v1",
                    "box_rect": rect(20, 20, 160, 160),
                    "turns": 1,
                    "min_segment_px": 20,
                    "duration_ms": 300,
                    "corner_hold_ms": 50
                }),
            ),
        ],
        false,
    );
    let mut device = FramedDevice::new();
    let result = graph.run(&mut device);
    assert!(result.is_ok(), "the page graph reaches p5: {result:?}");
    let actions = device.actions();
    assert!(
        matches!(
            actions.as_slice(),
            [
                InputAction::Tap { .. },
                InputAction::LongTap { .. },
                InputAction::Swipe { .. },
                InputAction::SingleTouchDragWithVerticalBrakeV1 { .. },
                InputAction::SingleTouchPathDragV1 { .. },
            ]
        ),
        "one input of each click, in order: {actions:?}"
    );
    device.assert_every_input_names_its_frame("page graph");

    let plan = admit(
        "p3",
        vec![
            press(
                "tap",
                "p0",
                "p1",
                json!({"kind": "point", "x": 10, "y": 10}),
            ),
            press("long_press", "p1", "p2", long_press()),
            press("drag", "p2", "p3", drag()),
        ],
        true,
    );
    let mut device = FramedDevice::new();
    let result = plan.run(&mut device);
    assert!(result.is_ok(), "the step plan reaches p3: {result:?}");
    let actions = device.actions();
    assert!(
        matches!(
            actions.as_slice(),
            [
                InputAction::Tap { .. },
                InputAction::LongTap { .. },
                InputAction::Swipe { .. },
            ]
        ),
        "one input of each step, in order: {actions:?}"
    );
    device.assert_every_input_names_its_frame("step plan");
}
