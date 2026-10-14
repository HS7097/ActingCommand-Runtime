// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-2 (model-398 v1.1 §4.1, §4.2 and §12 row RD-2-T1): the decoder of
// `adb track-devices` output. Several length-prefixed frames in one read, the empty list and a
// frame cut anywhere by reads give the right device lists; an output of unknown shape (bad
// hexadecimal, a line that is not a serial, a tab and a state, a payload that is not text) is
// refused, never guessed. Every frame is hand-written neutral text and no process runs; the
// serials are neutral fakes, distinct from the other device tests' so their labels stay their
// own, and a list never shows one.

use crate::{
    DeviceSerial, TrackDevicesDecoder, TrackDevicesProtocolInvalid, link_state_of_adb_state,
};
use actingcommand_contract::DeviceLinkState;

const SERIAL_A: &str = "TESTSERIAL0021";
const SERIAL_B: &str = "TESTSERIAL0022";

// One frame as adb writes it: four hexadecimal digits of the payload's length, then the payload.
fn frame(payload: &str) -> Vec<u8> {
    format!("{:04x}{payload}", payload.len()).into_bytes()
}

#[test]
fn rd2_t1_frames_decode_into_whole_lists_in_one_read_or_across_reads() {
    let handset_a = DeviceSerial::handset(SERIAL_A, "rd2-decoder-a");
    let handset_b = DeviceSerial::handset(SERIAL_B, "rd2-decoder-b");
    let mut decoder = TrackDevicesDecoder::new();
    let mut bytes = frame(&format!("{SERIAL_A}\tdevice\n{SERIAL_B}\tunauthorized\n"));
    bytes.extend(frame(&format!("{SERIAL_A}\toffline\n")));
    bytes.extend(frame(""));
    let lists = decoder.push(&bytes).expect("three whole frames");
    let [both, one, none] = lists.as_slice() else {
        panic!("three lists: {lists:?}");
    };
    assert_eq!(both.len(), 2);
    assert_eq!(both.state_of(&handset_a), DeviceLinkState::Device);
    assert_eq!(both.state_of(&handset_b), DeviceLinkState::Unauthorized);
    assert_eq!(one.state_of(&handset_a), DeviceLinkState::Offline);
    // A bound serial the list does not name is absent; the empty list names none.
    assert_eq!(one.state_of(&handset_b), DeviceLinkState::Absent);
    assert!(none.is_empty());
    assert_eq!(none.state_of(&handset_a), DeviceLinkState::Absent);
    // A list shows its states, never a serial.
    let shown = format!("{lists:?}");
    assert!(
        !shown.contains(SERIAL_A) && !shown.contains(SERIAL_B),
        "{shown}"
    );

    // A frame cut by the reads anywhere waits for its rest.
    let whole = frame(&format!("{SERIAL_B}\tdevice\n"));
    for cut in 1..whole.len() {
        let mut decoder = TrackDevicesDecoder::new();
        assert!(
            decoder
                .push(&whole[..cut])
                .expect("a part frame")
                .is_empty(),
            "cut {cut}"
        );
        let lists = decoder.push(&whole[cut..]).expect("the rest of the frame");
        let [list] = lists.as_slice() else {
            panic!("one list after cut {cut}: {lists:?}");
        };
        assert_eq!(
            list.state_of(&handset_b),
            DeviceLinkState::Device,
            "cut {cut}"
        );
    }
    // A stand-in serial without a raw value is never listed.
    assert_eq!(
        both.state_of(&DeviceSerial::unbound_handset("rd2-decoder-unbound")),
        DeviceLinkState::Absent
    );
}

#[test]
fn rd2_t1_adb_state_words_give_their_link_states() {
    for (word, state) in [
        ("device", DeviceLinkState::Device),
        ("offline", DeviceLinkState::Offline),
        ("unauthorized", DeviceLinkState::Unauthorized),
        (
            "no permissions (missing udev rules? user is in the plugdev group)",
            DeviceLinkState::NoPermissions,
        ),
        ("authorizing", DeviceLinkState::Other),
        ("connecting", DeviceLinkState::Other),
        ("recovery", DeviceLinkState::Other),
        ("sideload", DeviceLinkState::Other),
        ("bootloader", DeviceLinkState::Other),
    ] {
        assert_eq!(link_state_of_adb_state(word), state, "{word}");
        let handset = DeviceSerial::handset(SERIAL_A, "rd2-decoder-a");
        let lists = TrackDevicesDecoder::new()
            .push(&frame(&format!("{SERIAL_A}\t{word}\n")))
            .expect("one frame");
        let [list] = lists.as_slice() else {
            panic!("one list for {word}: {lists:?}");
        };
        assert_eq!(list.state_of(&handset), state, "{word}");
    }
}

#[test]
fn rd2_t1_an_output_of_unknown_shape_is_protocol_invalid() {
    let mut not_text = b"0002".to_vec();
    not_text.extend([0xff, 0xfe]);
    for (case, bytes) in [
        ("length not hexadecimal", b"00g1x".to_vec()),
        ("length with a sign", b"+001x".to_vec()),
        ("line without a tab", frame(&format!("{SERIAL_A} device\n"))),
        (
            "payload without its last newline",
            frame(&format!("{SERIAL_A}\tdevice")),
        ),
        (
            "state with a control character",
            frame(&format!("{SERIAL_A}\tdevice\r\n")),
        ),
        ("empty serial", frame("\tdevice\n")),
        ("empty state", frame(&format!("{SERIAL_A}\t\n"))),
        ("payload not text", not_text),
    ] {
        assert_eq!(
            TrackDevicesDecoder::new().push(&bytes),
            Err(TrackDevicesProtocolInvalid),
            "{case}"
        );
    }
    // A good frame before the bad one does not save the read.
    let mut bytes = frame(&format!("{SERIAL_A}\tdevice\n"));
    bytes.extend(b"zzzz");
    assert_eq!(
        TrackDevicesDecoder::new().push(&bytes),
        Err(TrackDevicesProtocolInvalid)
    );
}
