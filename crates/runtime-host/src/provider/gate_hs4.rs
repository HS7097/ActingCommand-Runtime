// SPDX-License-Identifier: AGPL-3.0-only

//! HS-4 (#402; design `rd-1031/backend-switch/model-backend-switch.md` §4.4; register §1.268):
//! the capability table of an instance whose input is `nemu_ipc` says what the Nemu input
//! performs, as the input declares it (`NemuInputSupport`, which the host's input gate and the
//! kernel follow): the tap, the long tap, the swipe, the segmented drags and the reset, and
//! neither `key` nor `text`. Nothing is opened: the table is built from the configured choices.

use super::*;
use actingcommand_contract::{InputAction, InputPathPoint};
use actingcommand_device::NemuInputSupport;

/// One input action of every kind an input capability covers; none for another capability.
fn actions_of(capability: EmulatorCapability) -> Vec<InputAction> {
    match capability {
        EmulatorCapability::InputTap => vec![InputAction::Tap { x: 10, y: 10 }],
        EmulatorCapability::InputLongTap => vec![InputAction::LongTap {
            x: 10,
            y: 10,
            duration_ms: 500,
        }],
        EmulatorCapability::InputSwipe => vec![InputAction::Swipe {
            x1: 10,
            y1: 10,
            x2: 90,
            y2: 10,
            duration_ms: 200,
        }],
        EmulatorCapability::InputSegmentedSwipe => vec![
            InputAction::SingleTouchDragWithVerticalBrakeV1 {
                x1: 300,
                y1: 400,
                x2: 100,
                y2: 400,
                x3: 100,
                y3: 300,
                horizontal_duration_ms: 200,
                corner_hold_ms: 150,
                brake_distance_px: 100,
                brake_duration_ms: 200,
                slope_in: 2,
                slope_out: 0,
            },
            InputAction::SingleTouchDragWithPauseAndBrakeV1 {
                x1: 300,
                y1: 400,
                x2: 100,
                y2: 400,
                drag_duration_ms: 200,
                pause_ms: 100,
                x3: 100,
                y3: 400,
                brake_duration_ms: 0,
            },
            InputAction::SingleTouchPathDragV1 {
                box_x: 0,
                box_y: 0,
                box_width: 400,
                box_height: 400,
                points: vec![
                    InputPathPoint { x: 10, y: 10 },
                    InputPathPoint { x: 200, y: 200 },
                ],
                segment_duration_ms: vec![200],
                corner_hold_ms: 0,
            },
        ],
        EmulatorCapability::InputKey => vec![InputAction::Key {
            key: "neutral".to_owned(),
        }],
        EmulatorCapability::InputText => vec![InputAction::Text {
            text: "neutral".to_owned(),
        }],
        EmulatorCapability::InputReset => vec![InputAction::Reset],
        _ => Vec::new(),
    }
}

// Each input capability the table claims for a Nemu instance is one the Nemu input performs for
// every action of its kind, and each it refuses is one the input does not perform; the design's
// list (§4.4) is the result: the long tap and the reset are now performed, and the segmented
// drags, which the input performed already, are no longer left unclaimed.
#[test]
fn hs4_capability_profile_matches_view() {
    let profile = capability_profile(TouchBackendChoice::NemuIpc, CaptureBackendChoice::NemuIpc)
        .expect("the capability table of a Nemu instance");
    let mut claimed = BTreeMap::new();
    for capability in EmulatorCapability::ALL {
        let actions = actions_of(capability);
        if actions.is_empty() {
            continue;
        }
        let evidence = profile.evidence(capability);
        let supported =
            evidence.implementation() == Some(EmulatorCapabilityImplementation::Supported);
        for action in &actions {
            let performed = NemuInputSupport::of(action) != NemuInputSupport::Unsupported;
            assert_eq!(
                supported, performed,
                "{capability:?}: the table claims {supported}, the input performs {performed}"
            );
        }
        assert_eq!(evidence.evidence_ref(), "nemu_ipc", "{capability:?}");
        claimed.insert(capability, supported);
    }
    assert_eq!(
        claimed,
        BTreeMap::from([
            (EmulatorCapability::InputTap, true),
            (EmulatorCapability::InputLongTap, true),
            (EmulatorCapability::InputSwipe, true),
            (EmulatorCapability::InputSegmentedSwipe, true),
            (EmulatorCapability::InputKey, false),
            (EmulatorCapability::InputText, false),
            (EmulatorCapability::InputReset, true),
        ])
    );
    assert_eq!(
        NemuInputSupport::of(&InputAction::Reset),
        NemuInputSupport::WithoutFrame,
        "the reset needs no frame"
    );
}
