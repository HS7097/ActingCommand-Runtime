// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T4 (exec key 112; register §1.138; brief-T4 Requirement 3): the Lab bundle reads
// the path drag `single_touch_path_drag_v1` as it reads the vertical-brake drag, in the form
// source compilation writes it (`box_rect`, `turns`, `min_segment_px`, `duration_ms`,
// `corner_hold_ms`), refusing another form or numbers outside the kernel's bounds; and the Lab's
// input port keeps refusing to perform it, before any port opens (the online Lab does not perform
// path drags until B-3). Neutral values; no device.
#[test]
fn t4_the_lab_reads_a_path_drag_and_does_not_perform_it() {
    use crate::SemanticInputExecutor;
    use crate::ports::{InputBackendFactory, InputBackendRequest, LabInputPort, PortSemanticInput};
    use actingcommand_contract::{InputAction, InputPathPoint};
    use actingcommand_device::{AdbConfig, DeviceTarget, MaaTouchConfig, TouchBackendConfig};
    use std::cell::Cell;

    let control = LabControl {
        game: "neutral".to_string(),
        server: "test".to_string(),
        ..test_control()
    };
    let click_json = json!({
        "kind": "single_touch_path_drag_v1",
        "box_rect": {"x": 400, "y": 200, "width": 300, "height": 200},
        "turns": 4,
        "min_segment_px": 40,
        "duration_ms": 1500,
        "corner_hold_ms": 30
    });
    let click: OperationClick =
        serde_json::from_value(click_json.clone()).expect("formal path drag");
    click
        .validate_for_schema(&control, "0.9")
        .unwrap_or_else(|error| panic!("the path drag is read: {}", error.message));
    for (case, field, value) in [
        ("an unknown member", "unexpected", json!(true)),
        ("a point member", "x", json!(450)),
        ("no turns", "turns", json!(0)),
        ("a shortest segment past half the box", "min_segment_px", json!(150)),
        ("a duration shorter than its segments", "duration_ms", json!(4)),
        (
            "a box past the frame's edge",
            "box_rect",
            json!({"x": 1200, "y": 200, "width": 300, "height": 200}),
        ),
    ] {
        let mut invalid = click_json.clone();
        invalid[field] = value;
        let invalid: OperationClick =
            serde_json::from_value(invalid).expect("syntactically valid path drag probe");
        invalid
            .validate_for_schema(&control, "0.9")
            .expect_err(case);
    }

    // A factory that counts the ports it is asked to open and opens none.
    #[derive(Default)]
    struct CountingFactory {
        opens: Cell<usize>,
    }

    impl InputBackendFactory for CountingFactory {
        fn open(&self, _request: InputBackendRequest) -> CliOutcome<Box<dyn LabInputPort>> {
            self.opens.set(self.opens.get() + 1);
            Err(CliError::device("no input port opens in this test"))
        }
    }

    let factory = CountingFactory::default();
    let input = PortSemanticInput {
        factory: &factory,
        instance_alias: Some("neutral.instance".to_owned()),
        config: TouchBackendConfig::new(
            AdbConfig::default(),
            DeviceTarget {
                serial: None,
                host: "neutral.host".to_owned(),
                port: 1,
                connect: false,
            },
            MaaTouchConfig::default(),
        ),
        lease: None,
    };
    let error = input
        .execute(InputAction::SingleTouchPathDragV1 {
            box_x: 400,
            box_y: 200,
            box_width: 300,
            box_height: 200,
            points: vec![
                InputPathPoint { x: 450, y: 250 },
                InputPathPoint { x: 650, y: 350 },
            ],
            segment_duration_ms: vec![500],
            corner_hold_ms: 0,
        })
        .expect_err("the port refuses the path drag");
    assert_eq!(error.code, "validation_failed");
    assert_eq!(factory.opens.get(), 0);
}
