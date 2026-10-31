// SPDX-License-Identifier: AGPL-3.0-only

//! The seams between the bench and the device: the backends, the device-side check, the
//! B_model material writer and the formal loop's touch. The real implementations are in
//! `real.rs`; the controllable fakes of §6.2 in `fake.rs`.

use crate::clock::Micros;
use crate::model::{BackendKind, CloseOutcome, PathKind};
use serde::{Deserialize, Serialize};

/// A capture floor on the bench's clock (§4.2): after a host time, after a stream sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Floor {
    pub after_us: Option<Micros>,
    pub after_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelLayout {
    Rgba8,
    Rgb8,
}

/// One native call of a sample (§3.2 item 1, from the timing points), on the bench's clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeSpan {
    pub kind: &'static str,
    pub start_us: Option<Micros>,
    pub end_us: Option<Micros>,
    pub code: Option<i32>,
}

/// What a sample returned.
#[derive(Debug, Clone, Serialize)]
pub struct SampledFrame {
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
    #[serde(skip)]
    pub pixels: Vec<u8>,
    /// S: the stream sequence, presentation time and arrival time (`T_recv`).
    pub stream_sequence: Option<u64>,
    pub device_pts_us: Option<u64>,
    pub received_us: Option<Micros>,
    /// N: the frame's build time on the bench's clock (`T_built`, with the wall clock apart).
    pub built_us: Option<Micros>,
    pub built_unix_ms: Option<u64>,
    /// The native calls and stdio gate waits the timing points reported.
    pub native: Vec<NativeSpan>,
    /// The fake backend only: the content's known source time (`T_src`, §5.3 "受控夹具").
    pub known_source_us: Option<Micros>,
    /// S with a floor: the `meets` decision's inputs and result (the C1 timing point).
    pub floor_check: Option<FloorCheck>,
}

/// One `meets` decision on the host's scale (analysis schema §3.3 `floor_check`):
/// `t_est_us = pts_us + offset_min_us − margin_us`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FloorCheck {
    pub pts_us: u64,
    pub offset_min_us: Option<i64>,
    pub margin_us: u64,
    pub t_est_us: Option<i64>,
    pub floor_us: Micros,
    pub t_recv_us: Option<Micros>,
    pub admitted: bool,
    /// The fake backend only (`backend: "fake"`).
    pub t_src_known_us: Option<Micros>,
}

/// What a failure means for the run (§3.11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Table A `vendor_stdio_redirect_busy`: retryable under its three conditions.
    StdioBusy,
    /// A measured result, not a failure of the path (`capture_frame_stale`).
    Measured,
    /// 本格停该路径: the path (and the paths sharing its backend) stop for this cell.
    CellPath,
    /// The device is gone: every path of this instance stops for this cell.
    CellInstance,
    /// The backend is poisoned: the whole run stops, `unconfirmed`.
    Poisoned,
    /// A resource is unconfirmed: the whole run stops, `unconfirmed`.
    Unconfirmed,
    /// A vendor stdio restore failed: the whole run stops, `stdio_unrestored`.
    StdioUnrestored,
}

/// A backend failure with its first error kept as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackendFailure {
    pub code: String,
    pub class: FailureClass,
    pub detail: String,
    /// Whether the timing points saw the native call start (`T_native_start`).
    pub native_started: bool,
    /// Whether the native call returned (`T_native_return`); a caller timeout leaves it false.
    pub native_returned: bool,
}

impl BackendFailure {
    pub fn new(code: impl Into<String>, class: FailureClass, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            class,
            detail: detail.into(),
            native_started: true,
            native_returned: true,
        }
    }

    pub fn without_native_call(mut self) -> Self {
        self.native_started = false;
        self.native_returned = false;
        self
    }

    pub fn native_not_returned(mut self) -> Self {
        self.native_returned = false;
        self
    }
}

/// S open failures by cause (§3.11.4 table B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFailureCause {
    ConfigInvalid,
    ServerDigestMismatch,
    CropAlignment,
    LocalCopy,
    DeviceVerify,
    WmSize,
    Push,
    Forward,
    ShellSpawn,
    ServerExitedBeforeAccept,
    ProtocolFirstByteOrCodec,
    SocketOrReader,
    StreamEndedBeforeFirstFrame,
    ConnectTimeout,
    HeaderTimeout,
    FirstFrameTimeout,
    CleanupUnconfirmed,
    /// Nemu open failures (the DLL, the connection, the first probe): the path stops.
    NemuOpen,
}

impl OpenFailureCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "config_invalid",
            Self::ServerDigestMismatch => "server_digest_mismatch",
            Self::CropAlignment => "crop_alignment",
            Self::LocalCopy => "local_copy",
            Self::DeviceVerify => "device_verify",
            Self::WmSize => "wm_size",
            Self::Push => "push",
            Self::Forward => "forward",
            Self::ShellSpawn => "shell_spawn",
            Self::ServerExitedBeforeAccept => "server_exited_before_accept",
            Self::ProtocolFirstByteOrCodec => "protocol_first_byte_or_codec",
            Self::SocketOrReader => "socket_or_reader",
            Self::StreamEndedBeforeFirstFrame => "stream_ended_before_first_frame",
            Self::ConnectTimeout => "connect_timeout",
            Self::HeaderTimeout => "header_timeout",
            Self::FirstFrameTimeout => "first_frame_timeout",
            Self::CleanupUnconfirmed => "cleanup_unconfirmed",
            Self::NemuOpen => "nemu_open",
        }
    }

    /// Only the three start-deadline timeouts may reopen (table B).
    pub fn reopenable(self) -> bool {
        matches!(
            self,
            Self::ConnectTimeout | Self::HeaderTimeout | Self::FirstFrameTimeout
        )
    }
}

/// What table B does with an open failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFailureHandling {
    /// Refused at the start check (`refused_start`).
    RefuseStart,
    /// S is not ready for the whole run; its cells are skipped by condition.
    PathNotReady,
    /// The path stops for this cell.
    CellPath,
    /// Every path of the instance stops for this cell; the instance's later cells are untested.
    CellInstance,
    /// Reopen after the cleanup and the device-side check confirm (at most 2 per cell).
    Reopen,
    /// The whole run stops, `unconfirmed`.
    WholeUnconfirmed,
}

pub fn open_failure_handling(cause: OpenFailureCause) -> OpenFailureHandling {
    use OpenFailureCause::*;
    match cause {
        ConfigInvalid => OpenFailureHandling::RefuseStart,
        ServerDigestMismatch | CropAlignment => OpenFailureHandling::PathNotReady,
        DeviceVerify => OpenFailureHandling::CellInstance,
        LocalCopy
        | WmSize
        | Push
        | Forward
        | ShellSpawn
        | ServerExitedBeforeAccept
        | ProtocolFirstByteOrCodec
        | SocketOrReader
        | StreamEndedBeforeFirstFrame
        | NemuOpen => OpenFailureHandling::CellPath,
        ConnectTimeout | HeaderTimeout | FirstFrameTimeout => OpenFailureHandling::Reopen,
        CleanupUnconfirmed => OpenFailureHandling::WholeUnconfirmed,
    }
}

/// An open that failed: the cause and what the open itself left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenFailure {
    pub cause: OpenFailureCause,
    pub detail: String,
    /// The stream's facts when the open got as far as the forward.
    pub stream: Option<StreamFacts>,
    /// Whether the open's own cleanup returned success (table B reopen condition 1).
    pub cleanup_confirmed: bool,
}

/// What the bench knows about an S stream it opened: the scid, the forward and the argument
/// list it started the server with (from the C1 timing points).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFacts {
    pub scid: u32,
    pub local_port: Option<u16>,
    /// The server's arguments after `adb shell`: the `CLASSPATH=` assignment, then argv.
    pub arguments: Vec<String>,
    pub remote_path: String,
}

impl StreamFacts {
    pub fn scid_text(&self) -> String {
        format!("{:08x}", self.scid)
    }

    /// The argv the device process must carry: the arguments without the environment
    /// assignment (`scrcpy.rs` sets `CLASSPATH=` in front of `app_process`).
    pub fn expected_argv(&self) -> Vec<String> {
        self.arguments
            .iter()
            .filter(|argument| !argument.starts_with("CLASSPATH="))
            .cloned()
            .collect()
    }
}

/// The device-side identity of an S server (§3.5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityTuple {
    pub serial_label: String,
    pub method: IdentityMethod,
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
    pub argv: Vec<String>,
    pub server_path: String,
    pub scid: u32,
    pub forward: Option<(u16, String)>,
}

/// The candidate identity methods C0 proves on each E (§3.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityMethod {
    /// `/proc/<pid>/{stat,cmdline,environ}` and `boot_id`.
    Proc,
    /// `ps -A -w -o PID,PPID,ARGS`, with the start time and environment from `/proc`.
    PsArgs,
}

impl IdentityMethod {
    pub const CANDIDATES: [IdentityMethod; 2] = [IdentityMethod::Proc, IdentityMethod::PsArgs];
}

/// Why a device-side check is not a confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "check", content = "detail")]
pub enum CheckFailure {
    /// `device_side_unconfirmed`: a field unsupported, truncated output, permission, adb failure
    /// or timeout, an unparseable line.
    Unconfirmed(String),
    /// `device_side_residual`: the same identity still matches after 2 s.
    Residual(String),
}

impl CheckFailure {
    pub fn detail(&self) -> &str {
        match self {
            Self::Unconfirmed(detail) | Self::Residual(detail) => detail,
        }
    }
}

/// The result of a disappearance check (§3.5.1 消失).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoneResult {
    Gone,
    /// `boot_id` changed: E restarted; the server counts as gone, and the cell ends.
    Rebooted,
}

/// The device-side check (§3.5.1); every call is a read-only adb query of the whitelisted
/// serial.
pub trait DeviceSideCheck: Send + Sync {
    /// Before an open: one full scan with `method` succeeds (negative control).
    fn negative_control(&self, instance: &str, method: IdentityMethod) -> Result<(), CheckFailure>;
    /// While the server lives: exactly one process matches the stream and every member of the
    /// identity tuple is read; the forward table holds exactly the stream's row.
    fn positive(
        &self,
        instance: &str,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<IdentityTuple, CheckFailure>;
    /// After the close: the same identity is gone and the exact forward is removed.
    fn gone(&self, instance: &str, identity: &IdentityTuple) -> Result<GoneResult, CheckFailure>;
    /// After a failed open without an identity: a full scan and the forward table show nothing
    /// of the stream.
    fn stream_absent(
        &self,
        instance: &str,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<(), CheckFailure>;
}

/// A device backend of one instance (§3.2 item 3). Every call runs under the instance's
/// admission lock; `close` runs on the close thread (§3.5.2).
pub trait PreviewBackend: Send {
    fn kind(&self) -> BackendKind;

    /// One sample for `path`: N and `b_model` make the two native calls; S takes the newest
    /// frame of its slot, or waits for one that meets `floor` (§4.2).
    fn sample(&mut self, floor: Option<Floor>) -> Result<SampledFrame, BackendFailure>;

    /// The formal loop's tap at `(x, y)`, on the instance's touch path.
    fn tap(&mut self, x: i32, y: i32) -> Result<(), BackendFailure>;

    /// Closes the backend's resources once (host side); the device side is checked apart.
    fn close(&mut self) -> CloseOutcome;

    /// Whether a caller timeout poisoned the backend.
    fn poisoned(&self) -> bool;

    /// The S stream's facts.
    fn stream(&self) -> Option<StreamFacts> {
        None
    }
}

/// Opens the backends (real or fake).
pub trait BackendFactory: Send + Sync {
    fn open(
        &self,
        instance: &str,
        kind: BackendKind,
        options: &OpenOptions,
    ) -> Result<Box<dyn PreviewBackend>, OpenFailure>;
}

/// Per-open options from the plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OpenOptions {
    pub scrcpy_bit_rate: Option<u32>,
    pub scrcpy_max_fps: Option<u32>,
    pub scrcpy_encoder: Option<String>,
}

/// B_model's material work for one frame (§5.9): PNG, the material on disk, the event drafts.
pub trait MaterialWriter: Send {
    /// Writes one frame and returns the bytes and event records it produced.
    fn write(&mut self, frame: &SampledFrame) -> Result<MaterialCost, BackendFailure>;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MaterialCost {
    pub bytes: u64,
    pub events: u64,
}

/// Opens one B_model writer per segment.
pub trait MaterialFactory: Send + Sync {
    fn open(&self, instance: &str, segment: &str) -> Result<Box<dyn MaterialWriter>, String>;
}

/// The S stream's facts for a path, when it has one.
pub fn path_has_stream(path: PathKind) -> bool {
    path.backend() == BackendKind::Scrcpy
}
