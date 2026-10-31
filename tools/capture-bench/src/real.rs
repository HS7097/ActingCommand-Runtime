// SPDX-License-Identifier: AGPL-3.0-only

//! The real backends on the experiment instances only: Nemu IPC and scrcpy through the device
//! crate (with its `bench-timing` points), the formal loop's `adb shell input tap`, and the
//! device-side check of §3.5.1 as read-only `adb -s <E> shell` and `adb -s <E> forward --list`
//! queries. Every adb call names a whitelisted serial; nothing here kills an adb server or
//! removes a forward the bench did not create.

use crate::backend::{
    BackendFactory, BackendFailure, CheckFailure, DeviceSideCheck, FailureClass, Floor, FloorCheck,
    GoneResult, IdentityMethod, IdentityTuple, NativeSpan, OpenFailure, OpenFailureCause,
    OpenOptions, PixelLayout, PreviewBackend, SampledFrame, StreamFacts,
};
use crate::clock::{Clock, Micros};
use crate::model::{BackendKind, CloseOutcome};
use crate::plan::{Plan, TouchKind};
use actingcommand_device::bench_timing::{BenchDetail, BenchEvent, BenchPoint, BenchSink};
use actingcommand_device::{
    Adb, AdbConfig, CaptureBackend, CaptureBackendChoice, CaptureBackendConfig, CaptureFloor,
    DeviceCloseAuthority, DeviceError, DeviceResourceQuiescence, DeviceResult, DeviceTarget,
    InputCheckPhase, InputExecutionContext, InputOperationCheck, NemuAppIndex,
    NemuApplicationTarget, NemuFrameGeometry, NemuInputConfig, NemuIpcBackend, NemuIpcConfig,
    NemuIpcSession, NemuSessionBackends, ScrcpyBackend, ScrcpyConfig,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The scrcpy server's class (the device process's argv item).
const SERVER_CLASS: &str = "com.genymobile.scrcpy.Server";
/// The bench's server path on the device (§3.10 item 2).
pub const REMOTE_SERVER_PATH: &str = "/data/local/tmp/ac402-scrcpy-server.jar";
const EVENTS_PER_KEY: usize = 512;

/// The process-wide timing sink (C1 points), keyed so each sample drains its own points.
#[derive(Default)]
pub struct TimingSink {
    nemu: Mutex<BTreeMap<i32, VecDeque<BenchEvent>>>,
    opens: Mutex<HashMap<ThreadId, Vec<BenchEvent>>>,
    meets: Mutex<BTreeMap<u32, BenchEvent>>,
    built: Mutex<HashMap<ThreadId, BenchEvent>>,
    stdio_restore_failed: Mutex<BTreeMap<i32, u64>>,
    dropped: AtomicU64,
}

impl TimingSink {
    pub fn install() -> Arc<Self> {
        let sink = Arc::new(Self::default());
        actingcommand_device::bench_timing::install(sink.clone());
        sink
    }

    fn drain_nemu(&self, instance: i32) -> Vec<BenchEvent> {
        lock(&self.nemu)
            .get_mut(&instance)
            .map(|events| events.drain(..).collect())
            .unwrap_or_default()
    }

    fn take_open(&self) -> Vec<BenchEvent> {
        lock(&self.opens)
            .remove(&std::thread::current().id())
            .unwrap_or_default()
    }

    fn take_built(&self) -> Option<BenchEvent> {
        lock(&self.built).remove(&std::thread::current().id())
    }

    fn last_meets(&self, scid: u32) -> Option<BenchEvent> {
        lock(&self.meets).remove(&scid)
    }

    fn restore_failures(&self, instance: i32) -> u64 {
        lock(&self.stdio_restore_failed)
            .get(&instance)
            .copied()
            .unwrap_or(0)
    }
}

impl BenchSink for TimingSink {
    fn record(&self, event: BenchEvent) {
        match event.point {
            BenchPoint::ScrcpyForwarded | BenchPoint::ScrcpyServerArguments => {
                lock(&self.opens)
                    .entry(std::thread::current().id())
                    .or_default()
                    .push(event);
            }
            BenchPoint::ScrcpyMeets => {
                if let Some(scid) = event.stream_scid {
                    lock(&self.meets).insert(scid, event);
                }
            }
            BenchPoint::NemuFrameBuilt => {
                lock(&self.built).insert(std::thread::current().id(), event);
            }
            _ => {
                if let (Some(instance), true) = (
                    event.nemu_instance,
                    matches!(
                        event.point,
                        BenchPoint::NemuStdioResumeStart
                            | BenchPoint::NemuStdioResumeEnd
                            | BenchPoint::NemuStdioRestoreEnd
                            | BenchPoint::NemuNativeProbeStart
                            | BenchPoint::NemuNativeProbeEnd
                            | BenchPoint::NemuNativeCaptureStart
                            | BenchPoint::NemuNativeCaptureEnd
                            | BenchPoint::NemuFlipStart
                            | BenchPoint::NemuFlipEnd
                    ),
                ) {
                    if event.point == BenchPoint::NemuStdioRestoreEnd
                        && event.detail == BenchDetail::Succeeded(false)
                    {
                        *lock(&self.stdio_restore_failed)
                            .entry(instance)
                            .or_insert(0) += 1;
                    }
                    let mut nemu = lock(&self.nemu);
                    let events = nemu.entry(instance).or_default();
                    if events.len() >= EVENTS_PER_KEY {
                        events.pop_front();
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    events.push_back(event);
                }
            }
        }
    }
}

/// What the real factory needs from the plan.
pub struct RealDevice {
    pub clock: Arc<dyn Clock>,
    pub sink: Arc<TimingSink>,
    pub adb_path: String,
    pub mumu_folder: PathBuf,
    pub instances: BTreeMap<String, (String, i32)>,
    pub serial_whitelist: Vec<String>,
    pub nemu_whitelist: Vec<i32>,
    /// #402 C2b: each instance's touch path (§5.6).
    pub touch: BTreeMap<String, TouchKind>,
    markers: AtomicU64,
}

impl RealDevice {
    pub fn new(plan: &Plan, clock: Arc<dyn Clock>, sink: Arc<TimingSink>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            sink,
            adb_path: plan.safety.adb_path.clone(),
            mumu_folder: PathBuf::from(&plan.safety.mumu_folder),
            instances: plan
                .instances
                .iter()
                .map(|instance| {
                    (
                        instance.name.clone(),
                        (instance.serial.clone(), instance.nemu_instance),
                    )
                })
                .collect(),
            serial_whitelist: plan.safety.serial_whitelist.clone(),
            nemu_whitelist: plan.safety.nemu_whitelist.clone(),
            touch: plan
                .instances
                .iter()
                .map(|instance| (instance.name.clone(), instance.touch))
                .collect(),
            markers: AtomicU64::new(1),
        })
    }

    /// The instance's serial and Nemu index, only when both are whitelisted (§3.10 item 1).
    fn identity(&self, instance: &str) -> Result<(String, i32), String> {
        let (serial, nemu) = self
            .instances
            .get(instance)
            .cloned()
            .ok_or_else(|| format!("unknown instance {instance}"))?;
        if !self.serial_whitelist.contains(&serial) || !self.nemu_whitelist.contains(&nemu) {
            return Err(format!("safety_boundary: {instance} is not whitelisted"));
        }
        Ok((serial, nemu))
    }

    fn adb(&self, timeout: Duration) -> Adb {
        Adb::new(AdbConfig {
            adb_path: self.adb_path.clone(),
            command_timeout: timeout,
        })
    }

    fn target(serial: &str) -> DeviceTarget {
        let (host, port) = serial
            .rsplit_once(':')
            .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host.to_owned(), port)))
            .unwrap_or_else(|| (serial.to_owned(), 0));
        DeviceTarget {
            serial: None,
            host,
            port,
            connect: true,
        }
    }

    fn micros(&self, instant: Instant) -> Option<Micros> {
        self.clock.micros_of(instant)
    }

    fn spans(&self, events: &[BenchEvent]) -> Vec<NativeSpan> {
        let mut spans = Vec::new();
        let mut open: BTreeMap<&'static str, Micros> = BTreeMap::new();
        for event in events {
            let at = self.micros(event.at);
            let (kind, start) = match event.point {
                BenchPoint::NemuNativeProbeStart => ("probe", true),
                BenchPoint::NemuNativeProbeEnd => ("probe", false),
                BenchPoint::NemuNativeCaptureStart => ("pixels", true),
                BenchPoint::NemuNativeCaptureEnd => ("pixels", false),
                BenchPoint::NemuStdioResumeStart => ("stdio_wait", true),
                BenchPoint::NemuStdioResumeEnd => ("stdio_wait", false),
                BenchPoint::NemuFlipStart => ("flip", true),
                BenchPoint::NemuFlipEnd => ("flip", false),
                _ => continue,
            };
            if start {
                if let Some(at) = at {
                    open.insert(kind, at);
                }
            } else {
                let code = match &event.detail {
                    BenchDetail::NativeReturn { code, .. } => Some(*code),
                    _ => None,
                };
                spans.push(NativeSpan {
                    kind,
                    start_us: open.remove(kind),
                    end_us: at,
                    code,
                });
            }
        }
        for (kind, start) in open {
            spans.push(NativeSpan {
                kind,
                start_us: Some(start),
                end_us: None,
                code: None,
            });
        }
        spans
    }

    fn marker(&self) -> String {
        format!(
            "{:016x}",
            self.clock.now_us() ^ self.markers.fetch_add(1, Ordering::Relaxed).rotate_left(29)
        )
    }
}

/// Table A: what a Nemu device error is.
fn classify_nemu(
    error: &DeviceError,
    events: &[BenchEvent],
    restore_failed: bool,
) -> BackendFailure {
    let message = error.message().to_owned();
    let started = events.iter().any(|event| {
        matches!(
            event.point,
            BenchPoint::NemuNativeProbeStart | BenchPoint::NemuNativeCaptureStart
        )
    });
    let starts = events
        .iter()
        .filter(|event| {
            matches!(
                event.point,
                BenchPoint::NemuNativeProbeStart | BenchPoint::NemuNativeCaptureStart
            )
        })
        .count();
    let ends = events
        .iter()
        .filter(|event| {
            matches!(
                event.point,
                BenchPoint::NemuNativeProbeEnd | BenchPoint::NemuNativeCaptureEnd
            )
        })
        .count();
    let (code, class) = if restore_failed {
        ("stdio_unrestored", FailureClass::StdioUnrestored)
    } else if message.contains("vendor_stdio_redirect_busy") {
        ("vendor_stdio_redirect_busy", FailureClass::StdioBusy)
    } else if message.contains("poisoned")
        || message.contains("timed out after")
        || message.contains("worker disconnected")
    {
        ("poisoned", FailureClass::Poisoned)
    } else if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
        ("unconfirmed", FailureClass::Unconfirmed)
    } else if message.contains("capture failed with code") {
        ("nemu_capture_return_nonzero", FailureClass::CellPath)
    } else if message.contains("resolution probe failed") || message.contains("invalid resolution")
    {
        ("nemu_probe_return_nonzero", FailureClass::CellPath)
    } else if message.contains("dimensions changed during capture") {
        ("nemu_size_changed_during_call", FailureClass::CellPath)
    } else {
        ("nemu_fatal", FailureClass::CellPath)
    };
    BackendFailure {
        code: code.to_owned(),
        class,
        detail: message,
        native_started: started,
        native_returned: starts == ends,
    }
}

/// Table B: an S open failure by its cause.
fn classify_scrcpy_open(error: &DeviceError) -> OpenFailureCause {
    let message = error.message();
    let has = |text: &str| message.contains(text);
    let timeout = has("10060") || has("timed out") || has("WouldBlock") || has("TimedOut");
    if has("scrcpy_server_digest_mismatch") {
        OpenFailureCause::ServerDigestMismatch
    } else if has("configuration is invalid") {
        OpenFailureCause::ConfigInvalid
    } else if has("smaller than the size alignment") {
        OpenFailureCause::CropAlignment
    } else if has("did not accept the connection within the start timeout") {
        OpenFailureCause::ConnectTimeout
    } else if has("stream header did not arrive within the start timeout") {
        OpenFailureCause::HeaderTimeout
    } else if has("no frame arrived within the start timeout") {
        OpenFailureCause::FirstFrameTimeout
    } else if has("exited before it accepted") {
        OpenFailureCause::ServerExitedBeforeAccept
    } else if has("first byte is not the connection byte") || has("not H.264") {
        OpenFailureCause::ProtocolFirstByteOrCodec
    } else if has("local port") || has("forward") {
        OpenFailureCause::Forward
    } else if has("push") {
        OpenFailureCause::Push
    } else if has("wm size") || has("screen size") {
        OpenFailureCause::WmSize
    } else if has("scrcpy_stream_ended") || has("scrcpy_video_size_mismatch") {
        OpenFailureCause::StreamEndedBeforeFirstFrame
    } else if has("device") && (has("offline") || has("not found") || has("unauthorized")) {
        OpenFailureCause::DeviceVerify
    } else if timeout {
        OpenFailureCause::HeaderTimeout
    } else {
        OpenFailureCause::SocketOrReader
    }
}

fn adb_tap(adb: &Adb, serial: &str, x: i32, y: i32) -> Result<(), BackendFailure> {
    let x = x.to_string();
    let y = y.to_string();
    adb.run(&["-s", serial, "shell", "input", "tap", &x, &y])
        .map(|_| ())
        .map_err(|error| {
            BackendFailure::new(
                "adb_input_tap_failed",
                FailureClass::CellPath,
                error.message(),
            )
            .without_native_call()
        })
}

/// #402 C2b: the bench's own input check: the formal loop's tap has the Nemu command timeout.
struct BenchInputCheck;

impl InputOperationCheck for BenchInputCheck {
    fn check(&self, _phase: InputCheckPhase) -> DeviceResult<Duration> {
        Ok(Duration::from_secs(5))
    }
}

/// #402 C2b: the experiment's fenced-write witness for the formal loop's `nemu_ipc` taps. Only
/// the Runtime's scheduler issues one on the production graph; the workspace guard exempts
/// `tools/capture-bench` on this experiment branch only (Workflow #402, never merged).
fn bench_witness() -> Result<actingcommand_contract::FencedWrite, String> {
    let issuer = actingcommand_contract::IdentifierIssuer::new()
        .map_err(|error| format!("identifier issuer: {error:?}"))?;
    let token = actingcommand_contract::LeaseToken::new(
        *issuer
            .mint_owner_epoch()
            .map_err(|error| format!("owner epoch: {error:?}"))?
            .transport(),
        *issuer
            .mint_lease_id()
            .map_err(|error| format!("lease id: {error:?}"))?
            .transport(),
        *issuer
            .mint_instance_id()
            .map_err(|error| format!("instance id: {error:?}"))?
            .transport(),
        *issuer
            .mint_holder_id()
            .map_err(|error| format!("holder id: {error:?}"))?
            .transport(),
        u64::MAX,
    )
    .map_err(|error| format!("lease token: {error:?}"))?;
    Ok(actingcommand_contract::issue_fenced_write(
        token,
        1,
        std::num::NonZeroU64::MIN,
        actingcommand_contract::FencedWritePurpose::Business,
    ))
}

/// The Nemu session of an instance: a capture-only backend, or (`touch = "nemu_ipc"`) the
/// paired session whose captures and taps share one worker thread.
#[allow(clippy::large_enum_variant)]
enum NemuSession {
    Capture(NemuIpcBackend),
    Paired {
        session: PairedViews,
        geometry: Option<Arc<NemuFrameGeometry>>,
        witness: actingcommand_contract::FencedWrite,
    },
}

/// The paired session's owner and its two views. The device crate hands the views out as
/// `Box<dyn InputBackend>` and `Box<dyn CaptureBackend>` without a `Send` bound; the bench moves
/// them, with the backend, into the instance's slot and calls them from the activity that holds
/// the instance's admission lock, one thread at a time.
struct PairedViews(NemuSessionBackends);

// SAFETY: the two boxes are the device crate's `NemuCaptureView` and `NemuInputView` built by
// `NemuIpcSession::open` (`crates/device/src/capture/nemu_input.rs`): their fields are an
// `Arc<NemuIpcSession>` (a `Mutex` over the backend whose worker thread does every native call),
// an `Arc` of plain selection data, a `Vec` of captured vendor output, a `String` and a `bool`,
// all `Send`. The bench never shares a `&PairedViews` between threads (the slot's `Mutex` and the
// admission lock serialize every call), so moving it to another thread is sound.
unsafe impl Send for PairedViews {}

impl std::ops::Deref for PairedViews {
    type Target = NemuSessionBackends;

    fn deref(&self) -> &NemuSessionBackends {
        &self.0
    }
}

impl std::ops::DerefMut for PairedViews {
    fn deref_mut(&mut self) -> &mut NemuSessionBackends {
        &mut self.0
    }
}

struct RealNemu {
    device: Arc<RealDevice>,
    backend: NemuSession,
    serial: String,
    nemu: i32,
    poisoned: bool,
}

impl PreviewBackend for RealNemu {
    fn kind(&self) -> BackendKind {
        BackendKind::Nemu
    }

    fn sample(&mut self, _floor: Option<Floor>) -> Result<SampledFrame, BackendFailure> {
        // A snapshot backend acquires after the call, so it meets any floor (capture.rs:79-85).
        let _ = self.device.sink.drain_nemu(self.nemu);
        let restores_before = self.device.sink.restore_failures(self.nemu);
        let _ = self.device.sink.take_built();
        let captured = match &mut self.backend {
            NemuSession::Capture(backend) => backend.capture_timed(),
            NemuSession::Paired {
                session, geometry, ..
            } => {
                let captured = session.capture.capture_timed();
                if let Ok(frame) = &captured {
                    // The taps need the geometry of the committed source frame.
                    *geometry = frame
                        .selection
                        .as_ref()
                        .and_then(|selection| selection.nemu_frame.clone());
                }
                captured
            }
        };
        let events = self.device.sink.drain_nemu(self.nemu);
        let built = self.device.sink.take_built();
        match captured {
            Ok(frame) => {
                let built_unix_ms = frame
                    .captured_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|since| since.as_millis() as u64);
                Ok(SampledFrame {
                    width: frame.width,
                    height: frame.height,
                    layout: PixelLayout::Rgba8,
                    pixels: frame.pixels,
                    stream_sequence: None,
                    device_pts_us: None,
                    received_us: None,
                    built_us: built.and_then(|event| self.device.micros(event.at)),
                    built_unix_ms,
                    native: self.device.spans(&events),
                    known_source_us: None,
                    floor_check: None,
                })
            }
            Err(error) => {
                let restore_failed = self.device.sink.restore_failures(self.nemu) > restores_before;
                let failure = classify_nemu(&error, &events, restore_failed);
                if failure.class == FailureClass::Poisoned {
                    self.poisoned = true;
                }
                Err(failure)
            }
        }
    }

    fn tap(&mut self, x: i32, y: i32) -> Result<(), BackendFailure> {
        match &mut self.backend {
            NemuSession::Capture(_) => {
                adb_tap(&self.device.adb(Duration::from_secs(5)), &self.serial, x, y)
            }
            NemuSession::Paired {
                session,
                geometry,
                witness,
            } => {
                let context = InputExecutionContext {
                    geometry: geometry.clone(),
                    check: Arc::new(BenchInputCheck),
                };
                let _ = self.device.sink.drain_nemu(self.nemu);
                let tapped = session.input.tap_in_frame(witness, x, y, &context);
                let events = self.device.sink.drain_nemu(self.nemu);
                tapped.map_err(|error| {
                    let mut failure = classify_nemu(&error, &events, false);
                    if failure.class == FailureClass::Poisoned {
                        self.poisoned = true;
                    } else if failure.code == "nemu_fatal" {
                        failure.code = "nemu_input_tap_failed".to_owned();
                    }
                    failure
                })
            }
        }
    }

    fn close(&mut self) -> CloseOutcome {
        let closed = match &mut self.backend {
            NemuSession::Capture(backend) => backend.close_once(DeviceCloseAuthority::LocalOnly),
            NemuSession::Paired { session, .. } => {
                let _ = session.capture.close_once(DeviceCloseAuthority::LocalOnly);
                let _ = session.input.close_once(DeviceCloseAuthority::LocalOnly);
                session
                    .owner
                    .close_once(DeviceCloseAuthority::LocalOnly, None)
            }
        };
        match closed {
            Ok(_) => CloseOutcome::Confirmed,
            Err(error) => CloseOutcome::Unconfirmed(error.message().to_owned()),
        }
    }

    fn poisoned(&self) -> bool {
        self.poisoned
    }
}

struct RealScrcpy {
    device: Arc<RealDevice>,
    backend: ScrcpyBackend,
    serial: String,
    facts: StreamFacts,
}

impl PreviewBackend for RealScrcpy {
    fn kind(&self) -> BackendKind {
        BackendKind::Scrcpy
    }

    fn sample(&mut self, floor: Option<Floor>) -> Result<SampledFrame, BackendFailure> {
        let floor_us = floor.and_then(|floor| floor.after_us);
        let device_floor = floor.map(|floor| CaptureFloor {
            after: floor
                .after_us
                .and_then(|at| self.device.clock.instant_at(at)),
            after_sequence: floor.after_sequence,
        });
        let _ = self.device.sink.last_meets(self.facts.scid);
        let captured = self.backend.capture_fresh(device_floor);
        let meets = self.device.sink.last_meets(self.facts.scid);
        match captured {
            Ok(frame) => {
                let stamp = frame.stream;
                let floor_check = match (floor_us, meets) {
                    (Some(floor_us), Some(event)) => match event.detail {
                        BenchDetail::Meets(inputs) => {
                            // The offset on the host's scale: the stream epoch plus the device
                            // crate's epoch-relative offset, so `T_est = pts + offset - margin`.
                            let offset_min_us = self
                                .device
                                .micros(inputs.epoch)
                                .zip(inputs.offset_min_us)
                                .map(|(epoch, offset)| epoch as i64 + offset as i64);
                            Some(FloorCheck {
                                pts_us: inputs.device_pts_us,
                                offset_min_us,
                                margin_us: inputs.margin_us as u64,
                                t_est_us: offset_min_us.map(|offset| {
                                    inputs.device_pts_us as i64 + offset - inputs.margin_us as i64
                                }),
                                floor_us,
                                t_recv_us: self.device.micros(inputs.received_at),
                                admitted: inputs.met,
                                t_src_known_us: None,
                            })
                        }
                        _ => None,
                    },
                    _ => None,
                };
                Ok(SampledFrame {
                    width: frame.width,
                    height: frame.height,
                    layout: PixelLayout::Rgb8,
                    pixels: frame.pixels,
                    stream_sequence: stamp.map(|stamp| stamp.sequence),
                    device_pts_us: stamp.map(|stamp| stamp.device_pts_us),
                    received_us: stamp.and_then(|stamp| self.device.micros(stamp.received_at)),
                    built_us: None,
                    built_unix_ms: None,
                    native: Vec::new(),
                    known_source_us: None,
                    floor_check,
                })
            }
            Err(error) => {
                let message = error.message().to_owned();
                let (code, class) = if message.contains("capture_frame_stale") {
                    ("capture_frame_stale", FailureClass::Measured)
                } else if message.contains("scrcpy_video_size_mismatch") {
                    ("scrcpy_video_size_mismatch", FailureClass::CellPath)
                } else if message.contains("scrcpy_stream_ended") {
                    ("scrcpy_stream_ended", FailureClass::CellPath)
                } else {
                    ("scrcpy_capture_failed", FailureClass::CellPath)
                };
                Err(BackendFailure::new(code, class, message).without_native_call())
            }
        }
    }

    fn tap(&mut self, x: i32, y: i32) -> Result<(), BackendFailure> {
        adb_tap(&self.device.adb(Duration::from_secs(5)), &self.serial, x, y)
    }

    fn close(&mut self) -> CloseOutcome {
        // Never by Drop (§1.4): the explicit close; its first result is cached.
        match self.backend.close_once(DeviceCloseAuthority::LocalOnly) {
            Ok(_) => CloseOutcome::Confirmed,
            Err(error) => CloseOutcome::Unconfirmed(error.message().to_owned()),
        }
    }

    fn poisoned(&self) -> bool {
        false
    }

    fn stream(&self) -> Option<StreamFacts> {
        Some(self.facts.clone())
    }
}

fn stream_facts(events: &[BenchEvent]) -> Option<StreamFacts> {
    let mut scid = None;
    let mut port = None;
    let mut arguments = Vec::new();
    for event in events {
        match &event.detail {
            BenchDetail::Forwarded {
                scid: id,
                local_port,
            } => {
                scid = Some(*id);
                port = Some(*local_port);
            }
            BenchDetail::ServerArguments {
                scid: id,
                arguments: list,
            } => {
                scid = Some(*id);
                arguments = list.clone();
            }
            _ => {}
        }
    }
    scid.map(|scid| StreamFacts {
        scid,
        local_port: port,
        arguments,
        remote_path: REMOTE_SERVER_PATH.to_owned(),
    })
}

impl BackendFactory for Arc<RealDevice> {
    fn open(
        &self,
        instance: &str,
        kind: BackendKind,
        options: &OpenOptions,
    ) -> Result<Box<dyn PreviewBackend>, OpenFailure> {
        let (serial, nemu) = self.identity(instance).map_err(|detail| OpenFailure {
            cause: OpenFailureCause::ConfigInvalid,
            detail,
            stream: None,
            cleanup_confirmed: true,
        })?;
        match kind {
            BackendKind::Nemu => {
                let mut config = NemuIpcConfig::default();
                config.nemu_folder = Some(self.mumu_folder.clone());
                config.instance_id = Some(nemu);
                config.display_id = 0;
                let _ = self.sink.drain_nemu(nemu);
                let nemu_failure = |error: DeviceError| OpenFailure {
                    cause: OpenFailureCause::NemuOpen,
                    detail: error.message().to_owned(),
                    stream: None,
                    cleanup_confirmed: error.resource_quiescence()
                        != Some(DeviceResourceQuiescence::Unconfirmed),
                };
                let touch = self.touch.get(instance).copied();
                let session = if touch == Some(TouchKind::NemuIpc) {
                    // §5.6: captures and taps on one paired session, one worker thread.
                    let witness = bench_witness().map_err(|detail| OpenFailure {
                        cause: OpenFailureCause::NemuOpen,
                        detail,
                        stream: None,
                        cleanup_confirmed: true,
                    })?;
                    let mut capture = CaptureBackendConfig::new(
                        AdbConfig {
                            adb_path: self.adb_path.clone(),
                            command_timeout: Duration::from_secs(12),
                        },
                        RealDevice::target(&serial),
                    );
                    capture.requested = CaptureBackendChoice::NemuIpc;
                    capture.capture_timeout = Duration::from_secs(5);
                    capture.nemu = config;
                    let application = NemuAppIndex::try_from(0_u32)
                        .and_then(|index| NemuApplicationTarget::new(crate::apk::PACKAGE, index))
                        .map_err(nemu_failure)?;
                    let opened = NemuIpcSession::open(
                        capture,
                        application,
                        NemuInputConfig {
                            command_timeout: Duration::from_secs(5),
                            shutdown_timeout: Duration::from_secs(5),
                            tap_hold: Duration::from_millis(50),
                        },
                        None,
                    )
                    .map_err(nemu_failure)?;
                    NemuSession::Paired {
                        session: PairedViews(opened.backend),
                        geometry: None,
                        witness,
                    }
                } else {
                    NemuSession::Capture(
                        NemuIpcBackend::new(
                            RealDevice::target(&serial),
                            config,
                            Duration::from_secs(5),
                        )
                        .map_err(nemu_failure)?,
                    )
                };
                Ok(Box::new(RealNemu {
                    device: Arc::clone(self),
                    backend: session,
                    serial,
                    nemu,
                    poisoned: false,
                }))
            }
            BackendKind::Scrcpy => {
                let mut config = ScrcpyConfig {
                    remote_path: REMOTE_SERVER_PATH.to_owned(),
                    ..ScrcpyConfig::default()
                };
                if let Some(bit_rate) = options.scrcpy_bit_rate {
                    config.video_bit_rate = bit_rate;
                }
                config.max_fps = options.scrcpy_max_fps;
                config.video_encoder = options.scrcpy_encoder.clone();
                let _ = self.sink.take_open();
                let opened = ScrcpyBackend::open(
                    AdbConfig {
                        adb_path: self.adb_path.clone(),
                        command_timeout: Duration::from_secs(12),
                    },
                    RealDevice::target(&serial),
                    config,
                );
                let facts = stream_facts(&self.sink.take_open());
                match (opened, facts) {
                    (Ok(backend), Some(facts)) => Ok(Box::new(RealScrcpy {
                        device: Arc::clone(self),
                        backend,
                        serial,
                        facts,
                    })),
                    (Ok(mut backend), None) => {
                        // Without the timing points the stream's identity is unknown, so its
                        // close cannot be confirmed on the device.
                        let host_closed =
                            backend.close_once(DeviceCloseAuthority::LocalOnly).is_ok();
                        Err(OpenFailure {
                            cause: OpenFailureCause::CleanupUnconfirmed,
                            detail: format!(
                                "the scrcpy open reported no scid (bench-timing off?); host close ok: {host_closed}"
                            ),
                            stream: None,
                            cleanup_confirmed: false,
                        })
                    }
                    (Err(error), facts) => Err(OpenFailure {
                        cause: classify_scrcpy_open(&error),
                        detail: error.message().to_owned(),
                        stream: facts,
                        cleanup_confirmed: error.resource_quiescence()
                            != Some(DeviceResourceQuiescence::Unconfirmed),
                    }),
                }
            }
        }
    }
}

/// One process of a device-side scan.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScannedProcess {
    pid: u32,
    start_ticks: u64,
    argv: Vec<String>,
    classpath: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Scan {
    boot_id: String,
    processes: Vec<ScannedProcess>,
}

fn scan_script(method: IdentityMethod, marker: &str) -> String {
    // `com[.]genymobile` never matches this script's own command line.
    let per_pid = "echo PID $p $(cut -d' ' -f22 /proc/$p/stat); \
        tr '\\000' '\\n' < /proc/$p/cmdline | sed 's/^/ARG /'; \
        tr '\\000' '\\n' < /proc/$p/environ 2>/dev/null | grep '^CLASSPATH=' | sed 's/^/ENV /'; \
        echo ENDPID";
    let listing = match method {
        IdentityMethod::Proc => format!(
            "for d in /proc/[0-9]*; do if grep -q 'com[.]genymobile' $d/cmdline 2>/dev/null; then p=${{d#/proc/}}; {per_pid}; fi; done"
        ),
        IdentityMethod::PsArgs => format!(
            "ps -A -w -o PID,ARGS | grep 'com[.]genymobile' | while read p rest; do {per_pid}; done"
        ),
    };
    format!("echo BOOT $(cat /proc/sys/kernel/random/boot_id); {listing}; echo END_{marker}")
}

fn parse_scan(output: &str, marker: &str) -> Result<Scan, String> {
    let mut boot_id = None;
    let mut processes = Vec::new();
    let mut current: Option<ScannedProcess> = None;
    let mut ended = false;
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if ended {
            return Err(format!("output after the end marker: {line}"));
        }
        if let Some(rest) = line.strip_prefix("BOOT ") {
            boot_id = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("PID ") {
            let mut fields = rest.split_whitespace();
            let pid = fields.next().and_then(|value| value.parse::<u32>().ok());
            let ticks = fields.next().and_then(|value| value.parse::<u64>().ok());
            match (pid, ticks, current.is_some()) {
                (Some(pid), Some(start_ticks), false) => {
                    current = Some(ScannedProcess {
                        pid,
                        start_ticks,
                        argv: Vec::new(),
                        classpath: None,
                    });
                }
                _ => return Err(format!("unparseable process line: {line}")),
            }
        } else if let Some(rest) = line.strip_prefix("ARG ") {
            current
                .as_mut()
                .ok_or_else(|| format!("argument outside a process: {line}"))?
                .argv
                .push(rest.to_owned());
        } else if line == "ARG" {
            current
                .as_mut()
                .ok_or_else(|| format!("argument outside a process: {line}"))?
                .argv
                .push(String::new());
        } else if let Some(rest) = line.strip_prefix("ENV CLASSPATH=") {
            current
                .as_mut()
                .ok_or_else(|| format!("environment outside a process: {line}"))?
                .classpath = Some(rest.to_owned());
        } else if line == "ENDPID" {
            processes.push(current.take().ok_or("ENDPID without a process")?);
        } else if line == format!("END_{marker}") {
            ended = true;
        } else {
            return Err(format!("unparseable line: {line}"));
        }
    }
    if !ended {
        return Err("the end marker is missing (truncated output)".to_owned());
    }
    if current.is_some() {
        return Err("a process record is not closed".to_owned());
    }
    let boot_id = boot_id
        .filter(|id| !id.is_empty())
        .ok_or("boot_id was not read")?;
    Ok(Scan { boot_id, processes })
}

fn parse_forwards(output: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [serial, local, remote] = fields.as_slice() else {
            return Err(format!("unparseable forward row: {line}"));
        };
        rows.push((
            (*serial).to_owned(),
            (*local).to_owned(),
            (*remote).to_owned(),
        ));
    }
    Ok(rows)
}

impl RealDevice {
    fn scan(&self, serial: &str, method: IdentityMethod) -> Result<Scan, CheckFailure> {
        let marker = self.marker();
        let script = scan_script(method, &marker);
        let output = self
            .adb(Duration::from_secs(5))
            .run(&["-s", serial, "shell", script.as_str()])
            .map_err(|error| {
                CheckFailure::Unconfirmed(format!("adb shell: {}", error.message()))
            })?;
        parse_scan(&output.stdout, &marker).map_err(CheckFailure::Unconfirmed)
    }

    fn forwards(&self, serial: &str) -> Result<Vec<(String, String, String)>, CheckFailure> {
        let output = self
            .adb(Duration::from_secs(5))
            .run(&["-s", serial, "forward", "--list"])
            .map_err(|error| {
                CheckFailure::Unconfirmed(format!("adb forward --list: {}", error.message()))
            })?;
        parse_forwards(&output.stdout).map_err(CheckFailure::Unconfirmed)
    }

    fn serial_of(&self, instance: &str) -> Result<String, CheckFailure> {
        self.identity(instance)
            .map(|(serial, _)| serial)
            .map_err(CheckFailure::Unconfirmed)
    }
}

fn forward_row(serial: &str, stream: &StreamFacts) -> Option<(String, String, String)> {
    stream.local_port.map(|port| {
        (
            serial.to_owned(),
            format!("tcp:{port}"),
            format!("localabstract:scrcpy_{}", stream.scid_text()),
        )
    })
}

impl DeviceSideCheck for Arc<RealDevice> {
    fn negative_control(&self, instance: &str, method: IdentityMethod) -> Result<(), CheckFailure> {
        let serial = self.serial_of(instance)?;
        let scan = self.scan(&serial, method)?;
        let stale = scan
            .processes
            .iter()
            .filter(|process| process.classpath.as_deref() == Some(REMOTE_SERVER_PATH))
            .count();
        if stale > 0 {
            return Err(CheckFailure::Residual(format!(
                "{stale} bench scrcpy server(s) already run on {instance}"
            )));
        }
        self.forwards(&serial)?;
        Ok(())
    }

    fn positive(
        &self,
        instance: &str,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<IdentityTuple, CheckFailure> {
        let serial = self.serial_of(instance)?;
        let scan = self.scan(&serial, method)?;
        let expected = stream.expected_argv();
        if expected.is_empty() || !expected.iter().any(|argument| argument == SERVER_CLASS) {
            return Err(CheckFailure::Unconfirmed(
                "the stream's argument list is not known".to_owned(),
            ));
        }
        let matching: Vec<&ScannedProcess> = scan
            .processes
            .iter()
            .filter(|process| {
                process
                    .argv
                    .iter()
                    .any(|argument| *argument == format!("scid={}", stream.scid_text()))
            })
            .collect();
        let [process] = matching.as_slice() else {
            return Err(CheckFailure::Unconfirmed(format!(
                "{} processes carry scid {}",
                matching.len(),
                stream.scid_text()
            )));
        };
        if process.argv != expected {
            return Err(CheckFailure::Unconfirmed(format!(
                "argv differs from the started arguments: {:?}",
                process.argv
            )));
        }
        if process.classpath.as_deref() != Some(stream.remote_path.as_str()) {
            return Err(CheckFailure::Unconfirmed(format!(
                "the server path is {:?}",
                process.classpath
            )));
        }
        let row = forward_row(&serial, stream)
            .ok_or_else(|| CheckFailure::Unconfirmed("the forward port is not known".to_owned()))?;
        let forwards = self.forwards(&serial)?;
        let rows = forwards.iter().filter(|known| **known == row).count();
        if rows != 1 {
            return Err(CheckFailure::Unconfirmed(format!(
                "{rows} forward rows match the stream's exact binding"
            )));
        }
        Ok(IdentityTuple {
            serial_label: instance.to_owned(),
            method,
            pid: process.pid,
            start_ticks: process.start_ticks,
            boot_id: scan.boot_id,
            argv: process.argv.clone(),
            server_path: stream.remote_path.clone(),
            scid: stream.scid,
            forward: stream.local_port.map(|port| (port, row.2.clone())),
        })
    }

    fn gone(&self, instance: &str, identity: &IdentityTuple) -> Result<GoneResult, CheckFailure> {
        let serial = self.serial_of(instance)?;
        let scid = format!("scid={:08x}", identity.scid);
        let started = Instant::now();
        loop {
            let scan = self.scan(&serial, identity.method)?;
            if scan.boot_id != identity.boot_id {
                return Ok(GoneResult::Rebooted);
            }
            let same = scan.processes.iter().any(|process| {
                (process.pid == identity.pid && process.start_ticks == identity.start_ticks)
                    || process.argv.iter().any(|argument| *argument == scid)
            });
            let forwards = self.forwards(&serial)?;
            let forwarded = identity.forward.as_ref().is_some_and(|(port, remote)| {
                forwards.iter().any(|(row_serial, local, row_remote)| {
                    *row_serial == serial && *local == format!("tcp:{port}") && row_remote == remote
                })
            });
            if !same && !forwarded {
                return Ok(GoneResult::Gone);
            }
            if started.elapsed() >= Duration::from_secs(2) {
                return Err(CheckFailure::Residual(format!(
                    "after 2 s: process present {same}, forward present {forwarded}"
                )));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn stream_absent(
        &self,
        instance: &str,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<(), CheckFailure> {
        let serial = self.serial_of(instance)?;
        let scid = format!("scid={}", stream.scid_text());
        let scan = self.scan(&serial, method)?;
        if scan
            .processes
            .iter()
            .any(|process| process.argv.iter().any(|argument| *argument == scid))
        {
            return Err(CheckFailure::Residual(format!(
                "the failed open's server {} still runs",
                stream.scid_text()
            )));
        }
        let remote = format!("localabstract:scrcpy_{}", stream.scid_text());
        if self
            .forwards(&serial)?
            .iter()
            .any(|(row_serial, _, row_remote)| *row_serial == serial && *row_remote == remote)
        {
            return Err(CheckFailure::Residual(format!(
                "the failed open's forward {remote} is still there"
            )));
        }
        Ok(())
    }

    fn stop_server(&self, instance: &str, identity: &IdentityTuple) -> Result<u32, CheckFailure> {
        let serial = self.serial_of(instance)?;
        let scan = self.scan(&serial, identity.method)?;
        if scan.boot_id != identity.boot_id {
            return Err(CheckFailure::Unconfirmed(
                "boot_id changed; nothing is stopped".to_owned(),
            ));
        }
        let same = scan.processes.iter().find(|process| {
            process.pid == identity.pid
                && process.start_ticks == identity.start_ticks
                && process.argv == identity.argv
                && process.classpath.as_deref() == Some(identity.server_path.as_str())
        });
        let Some(process) = same else {
            return Err(CheckFailure::Unconfirmed(format!(
                "no process matches the identity of {:08x} exactly; nothing is stopped",
                identity.scid
            )));
        };
        let pid = process.pid.to_string();
        self.adb(Duration::from_secs(5))
            .run(&["-s", serial.as_str(), "shell", "kill", pid.as_str()])
            .map_err(|error| {
                CheckFailure::Unconfirmed(format!("adb shell kill: {}", error.message()))
            })?;
        Ok(process.pid)
    }
}
