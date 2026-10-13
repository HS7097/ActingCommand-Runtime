// SPDX-License-Identifier: AGPL-3.0-only

//! #402 RD-3: the scrcpy continuous-stream capture backend for real devices (model #398 R1 §5).
//!
//! The scrcpy server is a pinned official release. The Windows exact-SHA build fetches it, checks
//! its size and SHA-256, and `build.rs` embeds it in the executable; the backend checks the
//! embedded bytes against the pin again before every push. The server encodes the device
//! screen as H.264 and sends it over an adb-forwarded socket. A reader thread decodes every
//! packet (OpenH264, built from source) and keeps only the latest picture with its sequence,
//! device presentation time and arrival time. A capture returns that picture, or waits for a
//! newer one when the caller names a floor (RD-3i, [`CaptureFloor`]). Touch stays on the
//! instance's own input backend.
//!
//! The server runs with every option that would change a device setting or the power state
//! turned off, and with the audio and control channels off ([`server_arguments`]). The video is
//! never downsampled: a display whose sides are not multiples of the size alignment is cropped
//! to a centred aligned region, the server refuses to retry at a smaller size, and a stream of
//! any other size is refused (`scrcpy_video_size_mismatch`). The geometry records the crop.

use super::{
    CaptureBackend, CaptureBackendName, CaptureExtent, CaptureFloor, CaptureFrameTransform,
    CaptureGeometryObservation, CaptureGeometryUnknownReason, CaptureRotation,
    CaptureRotationObservation, Frame, FrameStreamStamp, PixelFormat, read_adb_capture_geometry,
    require_geometry_open, verify_adb_device,
};
use crate::adb::{Adb, AdbConfig, parse_wm_size, stop_child};
use crate::{
    DeviceCloseAuthority, DeviceError, DeviceErrorCategory, DeviceErrorSeverity,
    DeviceResourceCloseOutcome, DeviceResourceQuiescence, DeviceResult, DeviceTarget,
    StreamCaptureCode,
};
use actingcommand_contract::CaptureFrameCrop;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fmt::Display;
use std::fs;
use std::io::{ErrorKind, Read};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

mod h264;
#[cfg(test)]
mod tests;

/// The pinned scrcpy server version (`build.rs`, from the pin in
/// `scripts/windows-tools/windows-tool-sources.v1.json`). The server refuses a client of any
/// other version.
pub const SCRCPY_SERVER_VERSION: &str = env!("ACTINGCOMMAND_SCRCPY_SERVER_VERSION");
/// The pinned scrcpy server SHA-256, lowercase hex.
pub const SCRCPY_SERVER_SHA256: &str = env!("ACTINGCOMMAND_SCRCPY_SERVER_SHA256");
/// The server `build.rs` embedded after checking it against the pin; empty when the build named
/// none (CI test jobs and local development builds).
static EMBEDDED_SERVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scrcpy-server.bin"));

const SERVER_CLASS: &str = "com.genymobile.scrcpy.Server";
const SOCKET_NAME_PREFIX: &str = "localabstract:scrcpy_";
const H264_CODEC_ID: u32 = 0x6832_3634;
const DEVICE_NAME_FIELD_BYTES: usize = 64;
const PACKET_HEADER_BYTES: usize = 12;
const MAX_PACKET_BYTES: u32 = 32 * 1024 * 1024;
const SESSION_PACKET_FLAG: u64 = 1 << 63;
const CONFIG_PACKET_FLAG: u64 = 1 << 62;
const PTS_MASK: u64 = (1 << 61) - 1;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(100);
const SERVER_STOP_TIMEOUT: Duration = Duration::from_millis(500);

static SCID_COUNTER: AtomicU32 = AtomicU32::new(0);
static LOCAL_COPY_COUNTER: AtomicU32 = AtomicU32::new(0);

/// The scrcpy stream's parameters. The defaults are starting values for the R1 real-device
/// check; the actingd configuration section that sets them follows the D1b mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrcpyConfig {
    /// Where the server is pushed; it must stay under `/data/local/tmp/`.
    pub remote_path: String,
    pub video_bit_rate: u32,
    /// The size alignment the video keeps: 1, 2, 4, 8 or 16. A display whose sides are not
    /// multiples of it is cropped to a centred aligned region, never scaled.
    pub size_alignment: u32,
    /// Bound for connecting to the started server and receiving its first frame.
    pub start_timeout: Duration,
    /// How long a capture with a floor waits for a frame that meets it before
    /// `capture_frame_stale`.
    pub fresh_wait: Duration,
    /// The latency allowance subtracted from a frame's estimated encode time before it is
    /// compared with a floor: the conservative side of the estimated clock offset.
    pub fresh_margin: Duration,
    /// The window of arrivals the device-to-host clock offset is estimated from.
    pub offset_window: Duration,
}

impl Default for ScrcpyConfig {
    fn default() -> Self {
        Self {
            remote_path: "/data/local/tmp/actingcommand-scrcpy-server.jar".to_owned(),
            video_bit_rate: 8_000_000,
            size_alignment: 8,
            start_timeout: Duration::from_secs(10),
            fresh_wait: Duration::from_secs(2),
            fresh_margin: Duration::from_millis(30),
            offset_window: Duration::from_secs(10),
        }
    }
}

impl ScrcpyConfig {
    fn validate(&self) -> DeviceResult<()> {
        let valid = matches!(self.size_alignment, 1 | 2 | 4 | 8 | 16)
            && self.video_bit_rate > 0
            && i32::try_from(self.video_bit_rate).is_ok()
            && self.remote_path.starts_with("/data/local/tmp/")
            && !self.remote_path.chars().any(char::is_whitespace)
            && !self.start_timeout.is_zero()
            && !self.fresh_wait.is_zero()
            && !self.offset_window.is_zero();
        if valid {
            Ok(())
        } else {
            Err(start_failure(
                "capture.scrcpy.config",
                "the scrcpy configuration is invalid",
            ))
        }
    }
}

/// The region of the display, in its natural orientation, that the server encodes (`crop=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrcpyCrop {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// #398 R1 §5.3: the centred region of a `width`×`height` natural display whose sides are
/// multiples of `alignment`, or `None` when the display already is one (no crop).
pub(crate) fn aligned_crop(width: u32, height: u32, alignment: u32) -> Option<ScrcpyCrop> {
    let aligned_width = width / alignment * alignment;
    let aligned_height = height / alignment * alignment;
    if aligned_width == width && aligned_height == height {
        return None;
    }
    Some(ScrcpyCrop {
        x: (width - aligned_width) / 2,
        y: (height - aligned_height) / 2,
        width: aligned_width,
        height: aligned_height,
    })
}

/// #398 R1 §5.3: what a delivered frame of size `video` is, relative to the logical display the
/// geometry read observed: the display itself, the crop the server encodes (in the display's
/// current orientation; scrcpy applies a natural-orientation crop transposed when the display is
/// rotated by a quarter turn), or unknown (`None`) when the stream matches neither. An unknown
/// relation leaves the host's frame-and-display comparison to judge the frame.
pub(crate) fn stream_frame_transform(
    natural_crop: Option<ScrcpyCrop>,
    video: (u32, u32),
    display: CaptureExtent,
    rotation: CaptureRotationObservation,
) -> Option<CaptureFrameTransform> {
    let Some(crop) = natural_crop else {
        return (video == (display.width(), display.height()))
            .then_some(CaptureFrameTransform::Identity);
    };
    let CaptureRotationObservation::Observed { rotation, .. } = rotation else {
        return None;
    };
    let region = match rotation {
        CaptureRotation::R90 | CaptureRotation::R270 => CaptureFrameCrop {
            x: crop.y,
            y: crop.x,
            width: crop.height,
            height: crop.width,
        },
        CaptureRotation::R0 | CaptureRotation::R180 => CaptureFrameCrop {
            x: crop.x,
            y: crop.y,
            width: crop.width,
            height: crop.height,
        },
    };
    (video == (region.width, region.height)).then_some(CaptureFrameTransform::Crop(region))
}

/// #402 RD-3 / #398 R1 §5.5: the server's command line after `adb shell`. Video only, H.264
/// only, no control channel (touch stays on the instance's input backend), no audio; no option
/// that changes a device setting or the power state; no size limit, a fixed alignment and no
/// retry at a smaller size, so the video is never downsampled.
pub(crate) fn server_arguments(
    config: &ScrcpyConfig,
    scid: u32,
    crop: Option<ScrcpyCrop>,
) -> Vec<String> {
    let mut arguments = vec![
        format!("CLASSPATH={}", config.remote_path),
        "app_process".to_owned(),
        "/".to_owned(),
        SERVER_CLASS.to_owned(),
        SCRCPY_SERVER_VERSION.to_owned(),
        format!("scid={scid:08x}"),
        "log_level=warn".to_owned(),
        "video=true".to_owned(),
        "audio=false".to_owned(),
        "control=false".to_owned(),
        "tunnel_forward=true".to_owned(),
        "video_codec=h264".to_owned(),
        format!("video_bit_rate={}", config.video_bit_rate),
        "max_size=0".to_owned(),
        format!("min_size_alignment={}", config.size_alignment),
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
    ];
    if let Some(crop) = crop {
        arguments.push(format!(
            "crop={}:{}:{}:{}",
            crop.width, crop.height, crop.x, crop.y
        ));
    }
    arguments
}

/// Why a stream stopped delivering. Every end is `scrcpy_stream_ended` except the size refusal.
///
/// `scrcpy_stream_ended` is a transient device error (coordinator ruling 10-13, the
/// instance-level rule of board #403): only that instance's run fails and the ladder handles
/// it; the host never treats it as fatal for the Runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamEnd {
    SocketClosed,
    ProtocolInvalid,
    DecoderUnavailable,
    DecodeFailed,
    VideoSizeMismatch,
}

impl StreamEnd {
    fn error(self, server_exited: bool) -> DeviceError {
        let (code, category, step, detail) = match self {
            Self::SocketClosed if server_exited => (
                StreamCaptureCode::ScrcpyStreamEnded,
                DeviceErrorCategory::ChildExit,
                "capture.scrcpy.stream.server_exited",
                "the scrcpy server exited",
            ),
            Self::SocketClosed => (
                StreamCaptureCode::ScrcpyStreamEnded,
                DeviceErrorCategory::Response,
                "capture.scrcpy.stream.socket_closed",
                "the scrcpy video socket closed",
            ),
            Self::ProtocolInvalid => (
                StreamCaptureCode::ScrcpyStreamEnded,
                DeviceErrorCategory::Protocol,
                "capture.scrcpy.stream.protocol",
                "the scrcpy video stream is malformed",
            ),
            Self::DecoderUnavailable => (
                StreamCaptureCode::ScrcpyStreamEnded,
                DeviceErrorCategory::Native,
                "capture.scrcpy.stream.decoder_open",
                "the H.264 decoder could not be created",
            ),
            Self::DecodeFailed => (
                StreamCaptureCode::ScrcpyStreamEnded,
                DeviceErrorCategory::Native,
                "capture.scrcpy.stream.decode",
                "the H.264 stream could not be decoded",
            ),
            Self::VideoSizeMismatch => (
                StreamCaptureCode::ScrcpyVideoSizeMismatch,
                DeviceErrorCategory::Protocol,
                "capture.scrcpy.stream.video_size",
                "the stream's video size is not the unscaled size requested",
            ),
        };
        let severity = if matches!(code, StreamCaptureCode::ScrcpyStreamEnded) {
            DeviceErrorSeverity::Transient
        } else {
            DeviceErrorSeverity::Fatal
        };
        coded_error(code, severity, detail).with_diagnostic(category, step)
    }
}

/// Why a capture got no frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshWaitFailure {
    Ended(StreamEnd),
    Stale { latest_sequence: Option<u64> },
    Poisoned,
}

/// One decoded picture, kept as the decoder produced it and converted to RGB only when a
/// capture takes it.
pub(crate) struct StreamPicture {
    width: u32,
    height: u32,
    pixels: PicturePixels,
}

enum PicturePixels {
    #[cfg_attr(not(test), allow(dead_code))]
    Rgb8(Vec<u8>),
    Yuv420(h264::Yuv420Picture),
}

impl StreamPicture {
    #[cfg(test)]
    pub(crate) fn rgb8(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels: PicturePixels::Rgb8(pixels),
        }
    }

    fn yuv420(picture: h264::Yuv420Picture) -> Self {
        Self {
            width: picture.width(),
            height: picture.height(),
            pixels: PicturePixels::Yuv420(picture),
        }
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn to_rgb8(&self) -> Vec<u8> {
        match &self.pixels {
            PicturePixels::Rgb8(pixels) => pixels.clone(),
            PicturePixels::Yuv420(picture) => picture.to_rgb8(),
        }
    }
}

/// Decodes the stream's packets (Annex B). A config packet, or a packet that completes no
/// picture, returns `None`.
pub(crate) trait StreamDecoder {
    fn decode(&mut self, packet: &[u8]) -> Result<Option<StreamPicture>, StreamEnd>;
}

/// The latest frame of a stream with its stamp.
#[derive(Clone)]
pub(crate) struct SlotFrame {
    picture: Arc<StreamPicture>,
    stamp: FrameStreamStamp,
}

impl std::fmt::Debug for SlotFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SlotFrame")
            .field("stamp", &self.stamp)
            .finish_non_exhaustive()
    }
}

impl SlotFrame {
    #[cfg(test)]
    pub(crate) fn stamp(&self) -> FrameStreamStamp {
        self.stamp
    }

    fn into_frame(self) -> DeviceResult<Frame> {
        let (width, height) = self.picture.dimensions();
        let mut frame = Frame::from_pixels(
            width,
            height,
            self.picture.to_rgb8(),
            PixelFormat::Rgb8,
            CaptureBackendName::Scrcpy,
        )?;
        let age = Instant::now().saturating_duration_since(self.stamp.received_at);
        if let Some(captured_at) = SystemTime::now().checked_sub(age) {
            frame.captured_at = captured_at;
        }
        frame.stream = Some(self.stamp);
        Ok(frame)
    }
}

/// What the reader thread shares with captures: the latest frame, the arrivals the clock
/// offset is estimated from, the current video size and whether the stream ended.
pub(crate) struct SlotState {
    epoch: Instant,
    offset_window: Duration,
    latest: Option<SlotFrame>,
    next_sequence: u64,
    offsets: VecDeque<(Instant, i128)>,
    video: Option<(u32, u32)>,
    ended: Option<StreamEnd>,
}

impl SlotState {
    fn new(epoch: Instant, offset_window: Duration) -> Self {
        Self {
            epoch,
            offset_window,
            latest: None,
            next_sequence: 1,
            offsets: VecDeque::new(),
            video: None,
            ended: None,
        }
    }

    fn session(&mut self, width: u32, height: u32) {
        self.video = Some((width, height));
    }

    /// Keeps `picture` as the latest frame and returns its sequence.
    pub(crate) fn publish(
        &mut self,
        picture: StreamPicture,
        device_pts_us: u64,
        received_at: Instant,
    ) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let offset = signed_micros(self.epoch, received_at) - i128::from(device_pts_us);
        self.offsets.push_back((received_at, offset));
        while self.offsets.len() > 1
            && self.offsets.front().is_some_and(|(arrived, _)| {
                received_at.saturating_duration_since(*arrived) > self.offset_window
            })
        {
            self.offsets.pop_front();
        }
        self.latest = Some(SlotFrame {
            picture: Arc::new(picture),
            stamp: FrameStreamStamp {
                sequence,
                device_pts_us,
                received_at,
            },
        });
        sequence
    }

    fn end(&mut self, end: StreamEnd) {
        if self.ended.is_none() {
            self.ended = Some(end);
        }
    }

    /// #398 R1 §5.2 (RD-3i): whether `frame` meets `floor`. A sequence floor needs a newer
    /// frame. A time floor needs a frame that arrived after it and whose encode time, estimated
    /// on the conservative side, is not earlier: the device clock's offset is the smallest
    /// arrival-minus-presentation difference in the window (no frame arrives before it is
    /// encoded), less `margin` for the transport latency that smallest difference still holds.
    fn meets(&self, frame: &SlotFrame, floor: &CaptureFloor, margin: Duration) -> bool {
        if floor
            .after_sequence
            .is_some_and(|sequence| frame.stamp.sequence <= sequence)
        {
            return false;
        }
        let Some(after) = floor.after else {
            return true;
        };
        if frame.stamp.received_at < after {
            return false;
        }
        let Some(offset) = self.offsets.iter().map(|(_, offset)| *offset).min() else {
            return false;
        };
        let margin = i128::try_from(margin.as_micros()).unwrap_or(i128::MAX);
        let encoded_no_earlier_than = i128::from(frame.stamp.device_pts_us)
            .saturating_add(offset)
            .saturating_sub(margin);
        encoded_no_earlier_than >= signed_micros(self.epoch, after)
    }
}

fn signed_micros(epoch: Instant, at: Instant) -> i128 {
    match at.checked_duration_since(epoch) {
        Some(since) => i128::try_from(since.as_micros()).unwrap_or(i128::MAX),
        None => -i128::try_from(epoch.duration_since(at).as_micros()).unwrap_or(i128::MAX),
    }
}

/// How a fresh-frame wait reads the time and blocks. Production blocks on the condition
/// variable; a test clock advances its own time and delivers scripted arrivals.
pub(crate) trait SlotClock {
    fn now(&self) -> Instant;

    fn wait<'a>(
        &self,
        ready: &Condvar,
        state: MutexGuard<'a, SlotState>,
        deadline: Instant,
    ) -> Result<MutexGuard<'a, SlotState>, FreshWaitFailure>;
}

struct SystemSlotClock;

impl SlotClock for SystemSlotClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wait<'a>(
        &self,
        ready: &Condvar,
        state: MutexGuard<'a, SlotState>,
        deadline: Instant,
    ) -> Result<MutexGuard<'a, SlotState>, FreshWaitFailure> {
        let timeout = deadline.saturating_duration_since(Instant::now());
        ready
            .wait_timeout(state, timeout)
            .map(|(state, _)| state)
            .map_err(|_| FreshWaitFailure::Poisoned)
    }
}

/// The latest frame of one stream, shared by its reader thread and its captures.
pub(crate) struct FrameSlot {
    state: Mutex<SlotState>,
    ready: Condvar,
}

impl FrameSlot {
    pub(crate) fn new(epoch: Instant, offset_window: Duration) -> Self {
        Self {
            state: Mutex::new(SlotState::new(epoch, offset_window)),
            ready: Condvar::new(),
        }
    }

    /// Changes the state and wakes every waiting capture. A poisoned state is left alone; the
    /// waiting capture reports it.
    pub(crate) fn update(&self, change: impl FnOnce(&mut SlotState)) {
        if let Ok(mut state) = self.state.lock() {
            change(&mut state);
        }
        self.ready.notify_all();
    }

    fn video(&self) -> Option<(u32, u32)> {
        self.state.lock().ok().and_then(|state| state.video)
    }

    /// #398 R1 §5.2 (RD-3i): the latest frame once it meets `floor` (any frame without one),
    /// waiting at most `wait`. A stream that ended fails at once.
    pub(crate) fn wait_fresh(
        &self,
        floor: Option<CaptureFloor>,
        wait: Duration,
        margin: Duration,
        clock: &dyn SlotClock,
    ) -> Result<SlotFrame, FreshWaitFailure> {
        let deadline = clock.now() + wait;
        let mut state = self.state.lock().map_err(|_| FreshWaitFailure::Poisoned)?;
        loop {
            if let Some(end) = state.ended {
                return Err(FreshWaitFailure::Ended(end));
            }
            if let Some(frame) = &state.latest
                && floor
                    .as_ref()
                    .is_none_or(|floor| state.meets(frame, floor, margin))
            {
                return Ok(frame.clone());
            }
            if clock.now() >= deadline {
                return Err(FreshWaitFailure::Stale {
                    latest_sequence: state.latest.as_ref().map(|frame| frame.stamp.sequence),
                });
            }
            state = clock.wait(&self.ready, state, deadline)?;
        }
    }
}

/// The video sizes the server may send: the requested (cropped or full) size, upright or
/// rotated by a quarter turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExpectedVideo {
    width: u32,
    height: u32,
}

impl ExpectedVideo {
    pub(crate) fn of(crop: Option<ScrcpyCrop>, natural: (u32, u32)) -> Self {
        let (width, height) = crop.map_or(natural, |crop| (crop.width, crop.height));
        Self { width, height }
    }

    fn accepts(self, width: u32, height: u32) -> bool {
        (width, height) == (self.width, self.height) || (width, height) == (self.height, self.width)
    }
}

/// Reads the stream after its header: session packets (the video size of each capture session)
/// and media packets (config or frame), one per call.
pub(crate) struct StreamPump {
    expected: ExpectedVideo,
    video: Option<(u32, u32)>,
}

impl StreamPump {
    pub(crate) fn new(expected: ExpectedVideo) -> Self {
        Self {
            expected,
            video: None,
        }
    }

    pub(crate) fn pump_one(
        &mut self,
        reader: &mut dyn Read,
        decoder: &mut dyn StreamDecoder,
        slot: &FrameSlot,
        arrival: &dyn Fn() -> Instant,
    ) -> Result<(), StreamEnd> {
        let mut header = [0u8; PACKET_HEADER_BYTES];
        read_stream(reader, &mut header)?;
        let flags_and_pts = u64::from_be_bytes([
            header[0], header[1], header[2], header[3], header[4], header[5], header[6], header[7],
        ]);
        let last = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
        if flags_and_pts & SESSION_PACKET_FLAG != 0 {
            let width = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
            let height = last;
            if !self.expected.accepts(width, height) {
                return Err(StreamEnd::VideoSizeMismatch);
            }
            self.video = Some((width, height));
            slot.update(|state| state.session(width, height));
            return Ok(());
        }
        let Some(video) = self.video else {
            return Err(StreamEnd::ProtocolInvalid);
        };
        if last == 0 || last > MAX_PACKET_BYTES {
            return Err(StreamEnd::ProtocolInvalid);
        }
        let mut payload = vec![0u8; last as usize];
        read_stream(reader, &mut payload)?;
        let received_at = arrival();
        let picture = decoder.decode(&payload)?;
        if flags_and_pts & CONFIG_PACKET_FLAG != 0 {
            return Ok(());
        }
        if let Some(picture) = picture {
            if picture.dimensions() != video {
                return Err(StreamEnd::VideoSizeMismatch);
            }
            let device_pts_us = flags_and_pts & PTS_MASK;
            slot.update(|state| {
                state.publish(picture, device_pts_us, received_at);
            });
        }
        Ok(())
    }

    /// Pumps packets until the stream ends, then records the end in `slot`, so every capture
    /// from then on fails with it and no frame of the ended stream is delivered (#398 R1 §5.4).
    /// Returns the end.
    pub(crate) fn run(
        &mut self,
        reader: &mut dyn Read,
        decoder: &mut dyn StreamDecoder,
        slot: &FrameSlot,
        arrival: &dyn Fn() -> Instant,
    ) -> StreamEnd {
        let end = loop {
            if let Err(end) = self.pump_one(reader, decoder, slot, arrival) {
                break end;
            }
        };
        slot.update(|state| state.end(end));
        end
    }
}

fn read_stream(reader: &mut dyn Read, buffer: &mut [u8]) -> Result<(), StreamEnd> {
    reader
        .read_exact(buffer)
        .map_err(|_| StreamEnd::SocketClosed)
}

fn run_stream(mut socket: TcpStream, slot: Arc<FrameSlot>, expected: ExpectedVideo) {
    match h264::OpenH264Decoder::open() {
        Err(end) => slot.update(|state| state.end(end)),
        Ok(mut decoder) => {
            StreamPump::new(expected).run(&mut socket, &mut decoder, &slot, &Instant::now);
        }
    }
}

struct StreamHandle {
    slot: Arc<FrameSlot>,
    socket: TcpStream,
    reader: JoinHandle<()>,
}

/// #402 RD-3: the scrcpy stream capture backend. Explicit selection only, on a real device only
/// (`CaptureBackendChoice::allowed_on`).
pub struct ScrcpyBackend {
    adb_config: AdbConfig,
    config: ScrcpyConfig,
    serial: String,
    crop: Option<ScrcpyCrop>,
    forward_port: Option<u16>,
    child: Option<Child>,
    stream: Option<StreamHandle>,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

impl ScrcpyBackend {
    /// Pushes the verified server, forwards its socket, starts it and waits for its first
    /// frame. A failure after the forward closes what was opened.
    pub fn open(
        adb_config: AdbConfig,
        target: DeviceTarget,
        config: ScrcpyConfig,
    ) -> DeviceResult<Self> {
        config.validate()?;
        let server = checked_server(EMBEDDED_SERVER)?;
        let serial = target.resolved_serial();
        let adb = Adb::new(adb_config.clone());
        verify_adb_device(&adb, &target, &serial, CaptureBackendName::Scrcpy)?;
        let natural = parse_wm_size(&adb.screen_size(&serial)?)?;
        let crop = aligned_crop(natural.width, natural.height, config.size_alignment);
        if crop.is_some_and(|crop| crop.width == 0 || crop.height == 0) {
            return Err(start_failure(
                "capture.scrcpy.crop",
                "the display is smaller than the size alignment",
            ));
        }
        push_server(&adb, &serial, server, &config.remote_path)?;
        let scid = next_scid();
        let forwarded = adb
            .forward(&serial, "tcp:0", &format!("{SOCKET_NAME_PREFIX}{scid:08x}"))
            .map_err(|error| start_failure_from(error, "capture.scrcpy.forward"))?;
        let port = parse_forward_port(&forwarded.stdout)?;
        let mut backend = Self {
            adb_config,
            config,
            serial,
            crop,
            forward_port: Some(port),
            child: None,
            stream: None,
            close_result: None,
        };
        match backend.start(&adb, scid, port, (natural.width, natural.height)) {
            Ok(()) => Ok(backend),
            Err(primary) => match backend.close_once(DeviceCloseAuthority::LocalOnly) {
                Ok(_) => Err(primary),
                Err(cleanup) => Err(primary.merge_resource_cleanup(cleanup)),
            },
        }
    }

    fn start(&mut self, adb: &Adb, scid: u32, port: u16, natural: (u32, u32)) -> DeviceResult<()> {
        let arguments = server_arguments(&self.config, scid, self.crop);
        let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let child = adb
            .shell_spawn(&self.serial, &arguments)
            .map_err(|error| start_failure_from(error, "capture.scrcpy.server.spawn"))?;
        self.child = Some(child);
        let deadline = Instant::now() + self.config.start_timeout;
        let mut socket = self.connect(port, deadline)?;
        read_stream_header(&mut socket, deadline)?;
        socket
            .set_read_timeout(None)
            .map_err(|error| start_failure("capture.scrcpy.socket", error))?;
        let shutdown = socket
            .try_clone()
            .map_err(|error| start_failure("capture.scrcpy.socket", error))?;
        let slot = Arc::new(FrameSlot::new(Instant::now(), self.config.offset_window));
        let reader_slot = Arc::clone(&slot);
        let expected = ExpectedVideo::of(self.crop, natural);
        let reader = thread::Builder::new()
            .name("scrcpy-stream".to_owned())
            .spawn(move || run_stream(socket, reader_slot, expected))
            .map_err(|error| start_failure("capture.scrcpy.stream.spawn", error))?;
        self.stream = Some(StreamHandle {
            slot: Arc::clone(&slot),
            socket: shutdown,
            reader,
        });
        let remaining = deadline.saturating_duration_since(Instant::now());
        match slot.wait_fresh(None, remaining, self.config.fresh_margin, &SystemSlotClock) {
            Ok(_) => Ok(()),
            Err(FreshWaitFailure::Stale { .. }) => Err(start_failure(
                "capture.scrcpy.first_frame",
                "no frame arrived within the start timeout",
            )),
            Err(failure) => Err(self.fresh_failure(failure)),
        }
    }

    /// Connects through the forward until the server answers with its dummy byte. Before the
    /// server listens, adb accepts and then closes the connection; that is retried.
    fn connect(&mut self, port: u16, deadline: Instant) -> DeviceResult<TcpStream> {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        loop {
            if let Some(child) = self.child.as_mut()
                && let Ok(Some(_)) = child.try_wait()
            {
                return Err(start_failure(
                    "capture.scrcpy.server.exited",
                    "the scrcpy server exited before it accepted the connection",
                ));
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| {
                    start_failure(
                        "capture.scrcpy.connect",
                        "the scrcpy server did not accept the connection within the start timeout",
                    )
                })?;
            if let Ok(mut stream) = TcpStream::connect_timeout(&address, remaining) {
                stream
                    .set_read_timeout(Some(remaining))
                    .map_err(|error| start_failure("capture.scrcpy.socket", error))?;
                let mut dummy = [0u8; 1];
                match stream.read(&mut dummy) {
                    Ok(1) if dummy[0] == 0 => return Ok(stream),
                    Ok(1) => {
                        return Err(start_failure(
                            "capture.scrcpy.connect",
                            "the scrcpy server's first byte is not the connection byte",
                        ));
                    }
                    _ => {}
                }
            }
            thread::sleep(CONNECT_RETRY_DELAY.min(remaining));
        }
    }

    fn fresh_failure(&mut self, failure: FreshWaitFailure) -> DeviceError {
        match failure {
            FreshWaitFailure::Ended(end) => {
                let exited = self
                    .child
                    .as_mut()
                    .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))));
                end.error(exited)
            }
            FreshWaitFailure::Stale { latest_sequence } => coded_error(
                StreamCaptureCode::CaptureFrameStale,
                DeviceErrorSeverity::Transient,
                format!(
                    "no frame encoded after the capture floor arrived within {} ms (latest sequence {latest_sequence:?})",
                    self.config.fresh_wait.as_millis()
                ),
            )
            .with_diagnostic(DeviceErrorCategory::Response, "capture.scrcpy.fresh_wait"),
            FreshWaitFailure::Poisoned => coded_error(
                StreamCaptureCode::ScrcpyStreamEnded,
                DeviceErrorSeverity::Transient,
                "the stream state lock was poisoned",
            )
            .with_diagnostic(DeviceErrorCategory::Native, "capture.scrcpy.stream.state"),
        }
    }

    fn release(&mut self) -> DeviceResult<DeviceResourceCloseOutcome> {
        let mut outcome = DeviceResourceCloseOutcome::confirmed(0);
        let mut failure: Option<DeviceError> = None;
        let stream = self.stream.take();
        // Closing the socket ends the server's encoder and the reader's blocking read.
        if let Some(stream) = &stream
            && let Err(error) = stream.socket.shutdown(Shutdown::Both)
            && error.kind() != ErrorKind::NotConnected
        {
            fold_failure(
                &mut failure,
                DeviceError::fatal(format!("failed to close the scrcpy video socket: {error}")),
            );
        }
        if let Some(child) = self.child.as_mut() {
            match stop_child(child, SERVER_STOP_TIMEOUT, "scrcpy") {
                Ok(stopped) => {
                    outcome = outcome.combine(stopped);
                    self.child = None;
                }
                Err(error) => {
                    if error.resource_quiescence() == Some(DeviceResourceQuiescence::Confirmed) {
                        self.child = None;
                    }
                    fold_failure(&mut failure, error);
                }
            }
        }
        if let Some(port) = self.forward_port.take() {
            let local = format!("tcp:{port}");
            if let Err(error) = Adb::new(self.adb_config.clone()).run(&[
                "-s",
                self.serial.as_str(),
                "forward",
                "--remove",
                local.as_str(),
            ]) {
                fold_failure(&mut failure, error);
            }
        }
        if let Some(stream) = stream {
            match stream.reader.join() {
                Ok(()) => outcome = outcome.combine(DeviceResourceCloseOutcome::confirmed(1)),
                Err(_) => fold_failure(
                    &mut failure,
                    DeviceError::fatal("the scrcpy stream reader panicked")
                        .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1),
                ),
            }
        }
        match failure {
            None => Ok(outcome),
            Some(error) => Err(error.with_close_outcome(&outcome)),
        }
    }
}

impl CaptureBackend for ScrcpyBackend {
    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        require_geometry_open(&self.close_result)?;
        let Some(video) = self.stream.as_ref().and_then(|stream| stream.slot.video()) else {
            return Ok(CaptureGeometryObservation::Unknown(
                CaptureGeometryUnknownReason::ProducerUnavailable,
            ));
        };
        let observation = read_adb_capture_geometry(
            &Adb::new(self.adb_config.clone()),
            &self.serial,
            CaptureBackendName::Scrcpy,
            deadline,
        )?;
        Ok(match observation {
            CaptureGeometryObservation::Observed(mut geometry) => {
                geometry.frame_transform = stream_frame_transform(
                    self.crop,
                    video,
                    geometry.logical_display_extent,
                    geometry.rotation,
                );
                CaptureGeometryObservation::Observed(geometry)
            }
            other => other,
        })
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        self.capture_fresh(None)
    }

    fn capture_timed(&mut self) -> DeviceResult<Frame> {
        self.capture_fresh(None)
    }

    fn capture_fresh(&mut self, floor: Option<CaptureFloor>) -> DeviceResult<Frame> {
        let Some(slot) = self.stream.as_ref().map(|stream| Arc::clone(&stream.slot)) else {
            return Err(DeviceError::fatal(
                "scrcpy capture after the stream was closed",
            ));
        };
        let started = Instant::now();
        let picked = slot.wait_fresh(
            floor,
            self.config.fresh_wait,
            self.config.fresh_margin,
            &SystemSlotClock,
        );
        let picked = match picked {
            Ok(picked) => picked,
            Err(failure) => return Err(self.fresh_failure(failure)),
        };
        let acquired = Instant::now().checked_duration_since(started);
        let mut frame = picked.into_frame()?;
        frame.capture_acquire_us = acquired.and_then(|span| u64::try_from(span.as_micros()).ok());
        Ok(frame)
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = self.release();
        self.close_result = Some(result.clone());
        result
    }
}

impl Drop for ScrcpyBackend {
    fn drop(&mut self) {
        let first_close = self.close_result.is_none();
        if let Err(error) = self.close_once(DeviceCloseAuthority::LocalOnly) {
            if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed)
                && let Some(owned) = self.child.take()
            {
                std::mem::forget(owned);
            }
            if first_close && !thread::panicking() {
                panic!("{error}");
            }
        }
    }
}

fn read_stream_header(socket: &mut TcpStream, deadline: Instant) -> DeviceResult<()> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            start_failure(
                "capture.scrcpy.header",
                "the scrcpy stream header did not arrive within the start timeout",
            )
        })?;
    socket
        .set_read_timeout(Some(remaining))
        .map_err(|error| start_failure("capture.scrcpy.socket", error))?;
    // The device name field is read and dropped; nothing of it is kept.
    let mut device_name = [0u8; DEVICE_NAME_FIELD_BYTES];
    socket
        .read_exact(&mut device_name)
        .map_err(|error| start_failure("capture.scrcpy.header", error))?;
    let mut codec = [0u8; 4];
    socket
        .read_exact(&mut codec)
        .map_err(|error| start_failure("capture.scrcpy.header", error))?;
    match u32::from_be_bytes(codec) {
        H264_CODEC_ID => Ok(()),
        other => Err(start_failure(
            "capture.scrcpy.header",
            format!("the scrcpy server sent codec id {other:#010x}, not H.264"),
        )),
    }
}

fn push_server(adb: &Adb, serial: &str, server: &[u8], remote: &str) -> DeviceResult<()> {
    let local = LocalServerCopy::write(server)?;
    let pushed = adb.push(serial, &local.path.to_string_lossy(), remote);
    let removed = local.remove();
    pushed.map_err(|error| start_failure_from(error, "capture.scrcpy.server.push"))?;
    removed
}

/// A file copy of the embedded server for `adb push`, checked against the pin on disk so the
/// bytes pushed are the pinned server, and removed after the push.
struct LocalServerCopy {
    path: PathBuf,
}

impl LocalServerCopy {
    fn write(server: &[u8]) -> DeviceResult<Self> {
        let directory = std::env::temp_dir().join("actingcommand-scrcpy");
        fs::create_dir_all(&directory)
            .map_err(|error| start_failure("capture.scrcpy.server.local_copy", error))?;
        let path = directory.join(format!(
            "scrcpy-server-v{SCRCPY_SERVER_VERSION}-{}-{}.jar",
            std::process::id(),
            LOCAL_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, server)
            .map_err(|error| start_failure("capture.scrcpy.server.local_copy", error))?;
        let copy = Self { path };
        let written = match fs::read(&copy.path) {
            Ok(written) => written,
            Err(error) => {
                let primary = start_failure("capture.scrcpy.server.local_copy", error);
                return Err(match copy.remove() {
                    Ok(()) => primary,
                    Err(cleanup) => primary.merge_resource_cleanup(cleanup),
                });
            }
        };
        if let Err(primary) = checked_server(&written) {
            return Err(match copy.remove() {
                Ok(()) => primary,
                Err(cleanup) => primary.merge_resource_cleanup(cleanup),
            });
        }
        Ok(copy)
    }

    fn remove(self) -> DeviceResult<()> {
        fs::remove_file(&self.path)
            .map_err(|error| start_failure("capture.scrcpy.server.local_copy", error))
    }
}

/// #402 RD-3: the server bytes, only when their SHA-256 is the pinned one.
///
/// A mismatch is an error, not fatal (coordinator ruling 10-13, board #403 direction): only the
/// scrcpy capture backend of the instance that opens it is unavailable, and the Runtime goes on.
/// The refusal is therefore not a non-transient device error, which the host would treat as
/// fatal on a capture open, and it is raised before anything is opened on the device.
pub(crate) fn checked_server(bytes: &[u8]) -> DeviceResult<&[u8]> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        digest.push(char::from(HEX[usize::from(byte >> 4)]));
        digest.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    if digest == SCRCPY_SERVER_SHA256 {
        Ok(bytes)
    } else {
        Err(coded_error(
            StreamCaptureCode::ScrcpyServerDigestMismatch,
            DeviceErrorSeverity::Transient,
            "the scrcpy server this build carries does not match the pinned SHA-256",
        )
        .with_diagnostic(
            DeviceErrorCategory::BackendLaunch,
            "capture.scrcpy.server.digest",
        ))
    }
}

fn parse_forward_port(stdout: &str) -> DeviceResult<u16> {
    stdout
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| {
            start_failure(
                "capture.scrcpy.forward",
                "adb forward did not report the local port it allocated",
            )
        })
}

fn next_scid() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let counter = SCID_COUNTER.fetch_add(1, Ordering::Relaxed);
    (std::process::id().rotate_left(13) ^ nanos ^ counter.wrapping_mul(0x9E37_79B9)) & 0x7FFF_FFFF
}

fn coded_error(
    code: StreamCaptureCode,
    severity: DeviceErrorSeverity,
    detail: impl Display,
) -> DeviceError {
    DeviceError::with_severity(severity, format!("{}: {detail}", code.as_str()))
}

/// `scrcpy_server_start_failed` is a transient device error (coordinator ruling 10-13, the
/// instance-level rule of board #403): only the open of that instance's capture backend, and so
/// that instance's run, fails and the ladder handles it; the host never treats it as fatal for
/// the Runtime. A device resource left unconfirmed keeps its own handling.
fn start_failure(step: &'static str, detail: impl Display) -> DeviceError {
    coded_error(
        StreamCaptureCode::ScrcpyServerStartFailed,
        DeviceErrorSeverity::Transient,
        detail,
    )
    .with_diagnostic(DeviceErrorCategory::BackendLaunch, step)
}

/// An adb step of the start failed: `scrcpy_server_start_failed`, transient as in
/// [`start_failure`], keeping the adb error's resource summary.
fn start_failure_from(error: DeviceError, step: &'static str) -> DeviceError {
    let message = format!(
        "{}: {}",
        StreamCaptureCode::ScrcpyServerStartFailed.as_str(),
        error.message()
    );
    error
        .with_severity_and_message(DeviceErrorSeverity::Transient, message)
        .with_diagnostic(DeviceErrorCategory::BackendLaunch, step)
}

fn fold_failure(failure: &mut Option<DeviceError>, error: DeviceError) {
    *failure = Some(match failure.take() {
        Some(primary) => primary.merge_resource_cleanup(error),
        None => error,
    });
}
