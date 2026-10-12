// SPDX-License-Identifier: AGPL-3.0-only

//! #402 RD-3 / #398 R1 RD-3i logic tests: a fake stream (protocol bytes in memory), a fake
//! decoder, a scripted clock and a fake snapshot backend. No device, no adb, no real time.

use super::*;
use crate::{CaptureBackendChoice, DeviceKind};
use actingcommand_contract::{
    CaptureGeometry, CaptureGeometrySource, CaptureRotationSource, CaptureWmSizeKind,
    TaskGeometryConclusion, TaskGeometryFrame, TaskGeometryObservation,
};
use std::cell::{Cell, RefCell};
use std::io::Cursor;

/// A decoder that turns every media packet into a picture of `size` whose pixels repeat the
/// packet's first byte; config packets (first byte 0) give none; a packet starting with 0xEE
/// fails to decode.
struct FakeDecoder {
    size: (u32, u32),
}

impl StreamDecoder for FakeDecoder {
    fn decode(&mut self, packet: &[u8]) -> Result<Option<StreamPicture>, StreamEnd> {
        match packet.first().copied() {
            Some(0) | None => Ok(None),
            Some(0xEE) => Err(StreamEnd::DecodeFailed),
            Some(value) => Ok(Some(picture(self.size, value))),
        }
    }
}

fn picture((width, height): (u32, u32), value: u8) -> StreamPicture {
    StreamPicture::rgb8(width, height, vec![value; (width * height * 3) as usize])
}

fn session_packet(width: u32, height: u32) -> Vec<u8> {
    let mut packet = 0x8000_0000u32.to_be_bytes().to_vec();
    packet.extend_from_slice(&width.to_be_bytes());
    packet.extend_from_slice(&height.to_be_bytes());
    packet
}

fn media_packet(pts: u64, config: bool, payload: &[u8]) -> Vec<u8> {
    let flags = if config { CONFIG_PACKET_FLAG } else { pts };
    let mut packet = flags.to_be_bytes().to_vec();
    packet.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

/// Pumps `bytes` until the stream ends; returns how it ended.
fn pump_all(
    bytes: Vec<u8>,
    expected: ExpectedVideo,
    decoded: (u32, u32),
    slot: &FrameSlot,
) -> StreamEnd {
    let mut reader = Cursor::new(bytes);
    let mut decoder = FakeDecoder { size: decoded };
    let mut pump = StreamPump::new(expected);
    let base = Instant::now();
    let tick = Cell::new(0u64);
    let arrival = || {
        tick.set(tick.get() + 1);
        base + Duration::from_millis(tick.get())
    };
    loop {
        if let Err(end) = pump.pump_one(&mut reader, &mut decoder, slot, &arrival) {
            return end;
        }
    }
}

#[test]
fn fake_stream_publishes_each_decoded_frame_with_its_sequence_and_presentation_time() {
    let slot = FrameSlot::new(Instant::now(), Duration::from_secs(10));
    let mut bytes = session_packet(1080, 2336);
    bytes.extend(media_packet(0, true, &[0, 0, 0, 1, 0x67]));
    bytes.extend(media_packet(1_000, false, &[7]));
    bytes.extend(media_packet(2_000, false, &[9]));
    let expected = ExpectedVideo::of(aligned_crop(1080, 2340, 8), (1080, 2340));
    assert_eq!(
        pump_all(bytes, expected, (1080, 2336), &slot),
        StreamEnd::SocketClosed
    );
    let latest = slot
        .wait_fresh(None, Duration::ZERO, Duration::ZERO, &SystemSlotClock)
        .expect_err("a stream that ended delivers nothing");
    assert_eq!(latest, FreshWaitFailure::Ended(StreamEnd::SocketClosed));
    let state = slot.state.lock().expect("slot state");
    let frame = state.latest.as_ref().expect("latest frame");
    assert_eq!(frame.stamp().sequence, 2);
    assert_eq!(frame.stamp().device_pts_us, 2_000);
    assert_eq!(frame.picture.dimensions(), (1080, 2336));
    assert_eq!(state.video, Some((1080, 2336)));
}

#[test]
fn fake_stream_ends_with_the_code_its_failure_requires() {
    let crop = aligned_crop(1080, 2340, 8);
    let expected = ExpectedVideo::of(crop, (1080, 2340));
    let session = session_packet(1080, 2336);
    let cases: Vec<(&str, Vec<u8>, (u32, u32), StreamEnd, StreamCaptureCode)> = vec![
        (
            "a session of another size than the unscaled crop",
            session_packet(1080, 2328),
            (1080, 2328),
            StreamEnd::VideoSizeMismatch,
            StreamCaptureCode::ScrcpyVideoSizeMismatch,
        ),
        (
            "a decoded picture of another size than the session",
            [session.clone(), media_packet(1, false, &[5])].concat(),
            (540, 1168),
            StreamEnd::VideoSizeMismatch,
            StreamCaptureCode::ScrcpyVideoSizeMismatch,
        ),
        (
            "a media packet before any session",
            media_packet(1, false, &[5]),
            (1080, 2336),
            StreamEnd::ProtocolInvalid,
            StreamCaptureCode::ScrcpyStreamEnded,
        ),
        (
            "a packet the decoder rejects",
            [session.clone(), media_packet(1, false, &[0xEE])].concat(),
            (1080, 2336),
            StreamEnd::DecodeFailed,
            StreamCaptureCode::ScrcpyStreamEnded,
        ),
        (
            "a socket that closes mid-packet",
            [
                session.clone(),
                media_packet(1, false, &[5, 5, 5])[..14].to_vec(),
            ]
            .concat(),
            (1080, 2336),
            StreamEnd::SocketClosed,
            StreamCaptureCode::ScrcpyStreamEnded,
        ),
    ];
    for (case, bytes, decoded, end, code) in cases {
        let slot = FrameSlot::new(Instant::now(), Duration::from_secs(10));
        assert_eq!(pump_all(bytes, expected, decoded, &slot), end, "{case}");
        let error = end.error(false);
        assert!(
            error.message().starts_with(code.as_str()),
            "{case}: {}",
            error.message()
        );
        assert_eq!(error.severity(), DeviceErrorSeverity::Fatal, "{case}");
    }
}

/// A clock that only moves when a capture waits: each wait delivers the next scripted arrival
/// that falls before the deadline, or jumps to the deadline.
struct ScriptedClock {
    now: Cell<Instant>,
    arrivals: RefCell<VecDeque<(Instant, u64, u8)>>,
}

impl SlotClock for ScriptedClock {
    fn now(&self) -> Instant {
        self.now.get()
    }

    fn wait<'a>(
        &self,
        _ready: &Condvar,
        mut state: MutexGuard<'a, SlotState>,
        deadline: Instant,
    ) -> Result<MutexGuard<'a, SlotState>, FreshWaitFailure> {
        let next = self.arrivals.borrow_mut().pop_front();
        match next {
            Some((at, pts, value)) if at <= deadline => {
                self.now.set(at);
                state.publish(picture((4, 4), value), pts, at);
            }
            Some(later) => {
                self.arrivals.borrow_mut().push_front(later);
                self.now.set(deadline);
            }
            None => self.now.set(deadline),
        }
        Ok(state)
    }
}

const MARGIN: Duration = Duration::from_millis(30);

/// One stream: frame 1 arrives 10 ms after the epoch with presentation time 1 s (so the device
/// clock reads 990 ms behind the host). The input completes at 100 ms. Frame 2 was encoded at
/// 60 ms but arrives late, at 150 ms; frame 3 was encoded at 290 ms and arrives at 300 ms.
fn stream_after_input(fresh_wait: Duration) -> (FrameSlot, ScriptedClock, CaptureFloor, Duration) {
    let epoch = Instant::now();
    let slot = FrameSlot::new(epoch, Duration::from_secs(10));
    slot.update(|state| {
        state.publish(
            picture((4, 4), 1),
            1_000_000,
            epoch + Duration::from_millis(10),
        );
    });
    let input_completed = epoch + Duration::from_millis(100);
    let clock = ScriptedClock {
        now: Cell::new(input_completed),
        arrivals: RefCell::new(VecDeque::from([
            (epoch + Duration::from_millis(150), 1_050_000, 2),
            (epoch + Duration::from_millis(300), 1_280_000, 3),
        ])),
    };
    let floor = CaptureFloor {
        after: Some(input_completed),
        after_sequence: Some(1),
    };
    (slot, clock, floor, fresh_wait)
}

#[test]
fn rd3i_t2_a_stream_delivers_only_a_frame_encoded_after_the_floor_within_fresh_wait() {
    // fresh_wait 500 ms: frame 2 (encoded before the input) is passed over, frame 3 delivered.
    let (slot, clock, floor, wait) = stream_after_input(Duration::from_millis(500));
    let delivered = slot
        .wait_fresh(Some(floor), wait, MARGIN, &clock)
        .expect("frame 3 arrives within 500 ms");
    assert_eq!(delivered.stamp().sequence, 3);
    assert_eq!(delivered.stamp().device_pts_us, 1_280_000);

    // fresh_wait 150 ms, the same stream: frame 3 arrives after the deadline, so the capture
    // fails capture_frame_stale and never hands back frame 2.
    let (slot, clock, floor, wait) = stream_after_input(Duration::from_millis(150));
    let failure = slot
        .wait_fresh(Some(floor), wait, MARGIN, &clock)
        .expect_err("no fresh frame within 150 ms");
    assert_eq!(
        failure,
        FreshWaitFailure::Stale {
            latest_sequence: Some(2)
        }
    );
    let mut backend = detached_backend(Duration::from_millis(150));
    let error = backend.fresh_failure(failure);
    assert!(
        error
            .message()
            .starts_with(StreamCaptureCode::CaptureFrameStale.as_str()),
        "{}",
        error.message()
    );
    assert_eq!(error.severity(), DeviceErrorSeverity::Transient);
}

#[test]
fn rd3i_t2_a_sequence_floor_never_hands_back_the_frame_already_delivered() {
    let epoch = Instant::now();
    let slot = FrameSlot::new(epoch, Duration::from_secs(10));
    slot.update(|state| {
        state.publish(picture((4, 4), 1), 1_000, epoch);
    });
    let polled = CaptureFloor {
        after: None,
        after_sequence: Some(1),
    };
    let quiet = ScriptedClock {
        now: Cell::new(epoch),
        arrivals: RefCell::new(VecDeque::new()),
    };
    assert_eq!(
        slot.wait_fresh(Some(polled), Duration::from_millis(200), MARGIN, &quiet)
            .expect_err("frame 1 was delivered already"),
        FreshWaitFailure::Stale {
            latest_sequence: Some(1)
        }
    );
    let repeating = ScriptedClock {
        now: Cell::new(epoch),
        arrivals: RefCell::new(VecDeque::from([(
            epoch + Duration::from_millis(100),
            101_000,
            1,
        )])),
    };
    let next = slot
        .wait_fresh(Some(polled), Duration::from_millis(200), MARGIN, &repeating)
        .expect("the repeated frame is a new frame");
    assert_eq!(next.stamp().sequence, 2);
}

/// A backend with no device behind it, for the error mapping.
fn detached_backend(fresh_wait: Duration) -> ScrcpyBackend {
    ScrcpyBackend {
        adb_config: AdbConfig::default(),
        config: ScrcpyConfig {
            fresh_wait,
            ..ScrcpyConfig::default()
        },
        serial: String::new(),
        crop: None,
        forward_port: None,
        child: None,
        stream: None,
        close_result: None,
    }
}

/// A snapshot backend: every capture is a new acquisition.
struct SnapshotBackend {
    captures: u8,
}

impl CaptureBackend for SnapshotBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.captures += 1;
        Frame::from_pixels(
            2,
            2,
            vec![self.captures; 12],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }
}

#[test]
fn rd3i_t1_a_snapshot_backend_meets_any_floor_exactly_as_capture_timed() {
    let floor = CaptureFloor {
        after: Some(Instant::now()),
        after_sequence: Some(41),
    };
    let mut fresh = SnapshotBackend { captures: 0 };
    let mut timed = SnapshotBackend { captures: 0 };
    for _ in 0..2 {
        let by_floor = fresh.capture_fresh(Some(floor)).expect("fresh capture");
        let by_timed = timed.capture_timed().expect("timed capture");
        assert_eq!(by_floor.pixels, by_timed.pixels);
        assert_eq!(
            (by_floor.width, by_floor.height),
            (by_timed.width, by_timed.height)
        );
        assert_eq!(by_floor.backend_name, by_timed.backend_name);
        assert_eq!(by_floor.stream, None);
        assert!(by_floor.capture_acquire_us().is_some());
    }
    assert_eq!(fresh.captures, 2);

    // The primed frame of an explicitly opened snapshot backend is returned for a floor too,
    // exactly as the first plain capture returns it.
    let primed_frame = Frame::from_pixels(
        2,
        2,
        vec![9; 12],
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .expect("primed frame");
    let mut primed = super::super::PrimedCaptureBackend {
        inner: Box::new(SnapshotBackend { captures: 0 }),
        primed: Some(primed_frame),
        close_result: None,
    };
    let first = primed.capture_fresh(Some(floor)).expect("primed capture");
    assert_eq!(first.pixels, vec![9; 12]);
    let second = primed.capture_fresh(Some(floor)).expect("next capture");
    assert_eq!(second.pixels, vec![1; 12]);
}

#[test]
fn rd3i_t4_a_cropped_stream_records_its_crop_and_an_unrecorded_one_is_judged_as_before() {
    let crop = aligned_crop(1080, 2340, 8).expect("2340 is not a multiple of 8");
    assert_eq!(
        crop,
        ScrcpyCrop {
            x: 0,
            y: 2,
            width: 1080,
            height: 2336
        }
    );
    assert_eq!(aligned_crop(1080, 1920, 8), None);
    let display = CaptureExtent::new(1080, 2340).expect("display");
    let rotation = CaptureRotationObservation::Observed {
        rotation: CaptureRotation::R0,
        source: CaptureRotationSource::DumpsysDisplayOrientation,
    };
    let transform = stream_frame_transform(Some(crop), (1080, 2336), display, rotation);
    assert_eq!(
        transform,
        Some(CaptureFrameTransform::Crop(CaptureFrameCrop {
            x: 0,
            y: 2,
            width: 1080,
            height: 2336
        }))
    );
    // A quarter turn transposes the natural-orientation crop.
    let landscape = CaptureExtent::new(2340, 1080).expect("display");
    let turned = CaptureRotationObservation::Observed {
        rotation: CaptureRotation::R90,
        source: CaptureRotationSource::DumpsysDisplayOrientation,
    };
    assert_eq!(
        stream_frame_transform(Some(crop), (2336, 1080), landscape, turned),
        Some(CaptureFrameTransform::Crop(CaptureFrameCrop {
            x: 2,
            y: 0,
            width: 2336,
            height: 1080
        }))
    );

    let geometry = |frame_transform| CaptureGeometry {
        backend: CaptureBackendName::Scrcpy,
        source: CaptureGeometrySource::AdbDefaultDisplay {
            serial: "TESTSERIAL0001".to_owned(),
            wm_extent: display,
            wm_size_kind: CaptureWmSizeKind::Physical,
        },
        logical_display_extent: display,
        rotation,
        sampled_at: SystemTime::UNIX_EPOCH,
        frame_transform,
    };
    let frame = CaptureExtent::new(1080, 2336).expect("frame");
    let recorded = geometry(transform);
    assert!(recorded.frame_matches_display(frame));
    let encoded = serde_json::to_string(&recorded).expect("serialize geometry");
    assert_eq!(
        serde_json::from_str::<CaptureGeometry>(&encoded).expect("deserialize geometry"),
        recorded
    );

    // The same frame without a recorded transform is a frame that differs from its display.
    let unrecorded = geometry(None);
    assert!(!unrecorded.frame_matches_display(frame));
    let frame_id = *actingcommand_contract::IdentifierIssuer::new()
        .expect("ids")
        .mint_frame_id()
        .expect("frame id")
        .transport();
    let conclusion = TaskGeometryObservation::assess(
        &TaskGeometryFrame {
            frame_id,
            extent: frame,
            backend: CaptureBackendName::Scrcpy,
            captured_at: SystemTime::UNIX_EPOCH,
            producer: CaptureGeometryObservation::Unknown(
                CaptureGeometryUnknownReason::ProducerObservationAbsent,
            ),
        },
        &CaptureGeometryObservation::Observed(unrecorded),
    );
    assert!(
        matches!(conclusion, TaskGeometryConclusion::AspectMismatch { .. }),
        "{conclusion:?}"
    );
}

#[test]
fn rd3i_t5_scrcpy_is_never_an_automatic_candidate_and_only_a_handset_may_select_it() {
    assert!(!super::super::AUTO_CAPTURE_BACKEND_ORDER.contains(&CaptureBackendName::Scrcpy));
    assert_eq!(
        CaptureBackendChoice::parse("scrcpy").expect("scrcpy parses"),
        CaptureBackendChoice::Scrcpy
    );
    assert!(!CaptureBackendChoice::Scrcpy.allowed_on(DeviceKind::Emulator));
    assert!(CaptureBackendChoice::Scrcpy.allowed_on(DeviceKind::Handset));
    for choice in [
        CaptureBackendChoice::Adb,
        CaptureBackendChoice::DroidcastRaw,
        CaptureBackendChoice::NemuIpc,
    ] {
        assert!(choice.allowed_on(DeviceKind::Emulator), "{choice:?}");
    }
}

#[test]
fn server_arguments_change_no_device_setting_and_never_downsample() {
    let config = ScrcpyConfig::default();
    let arguments = server_arguments(&config, 0x0123_abcd, aligned_crop(1080, 2340, 8));
    let expected = [
        "CLASSPATH=/data/local/tmp/actingcommand-scrcpy-server.jar".to_owned(),
        "app_process".to_owned(),
        "/".to_owned(),
        "com.genymobile.scrcpy.Server".to_owned(),
        SCRCPY_SERVER_VERSION.to_owned(),
        "scid=0123abcd".to_owned(),
        "log_level=warn".to_owned(),
        "video=true".to_owned(),
        "audio=false".to_owned(),
        "control=false".to_owned(),
        "tunnel_forward=true".to_owned(),
        "video_codec=h264".to_owned(),
        "video_bit_rate=8000000".to_owned(),
        "max_size=0".to_owned(),
        "min_size_alignment=8".to_owned(),
        "downsize_on_error=false".to_owned(),
        "power_on=false".to_owned(),
        "show_touches=false".to_owned(),
        "stay_awake=false".to_owned(),
        "power_off_on_close=false".to_owned(),
        "keep_active=false".to_owned(),
        "clipboard_autosync=false".to_owned(),
        "cleanup=false".to_owned(),
        "send_device_meta=true".to_owned(),
        "send_frame_meta=true".to_owned(),
        "send_dummy_byte=true".to_owned(),
        "send_stream_meta=true".to_owned(),
        "crop=1080:2336:0:2".to_owned(),
    ];
    assert_eq!(arguments, expected);
    for forbidden in [
        "screen_off_timeout",
        "display_ime_policy",
        "new_display",
        "turn_screen_off",
        "max_fps",
        "angle",
    ] {
        assert!(
            arguments
                .iter()
                .all(|argument| !argument.starts_with(&format!("{forbidden}="))),
            "{forbidden}"
        );
    }
}

/// Coordinator ruling 10-13 (board #403 direction): a server that is not the pinned one makes
/// only this instance's scrcpy capture backend unavailable, with its code; it never stops the
/// Runtime. The code is an error of class `none`; the refusal is not the non-transient device
/// error the host treats as fatal on a capture open, and it holds no device resource.
#[test]
fn a_server_that_is_not_the_pinned_one_makes_only_this_instance_backend_unavailable() {
    use actingcommand_contract::outcome::{Category, LadderClass};
    let code = StreamCaptureCode::ScrcpyServerDigestMismatch;
    assert_eq!(code.category(), Category::Error);
    assert_eq!(code.ladder_class(), LadderClass::NotApplicable);
    let error = checked_server(b"not the pinned server").expect_err("refused");
    assert!(
        error.message().starts_with(code.as_str()),
        "{}",
        error.message()
    );
    assert_eq!(error.severity(), DeviceErrorSeverity::Transient);
    assert!(error.resource_quiescence().is_none());
    assert!(error.resource_close_causes().is_empty());
}
