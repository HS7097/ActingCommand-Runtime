// SPDX-License-Identifier: AGPL-3.0-only

//! §6.2: the controllable fake backend of the D1 rehearsal. It implements the same seams as the
//! real backends and can be told, per instance, backend and call number, to be slow, to fail
//! with each table A and table B cause, to poison, to fail a stdio restore, to end its stream,
//! to never return from a close, and to answer the device-side check with each kind of
//! unconfirmed result. Every frame carries its known source time (`T_src`).

use crate::apk::FakeApk;
use crate::backend::{
    BackendFactory, BackendFailure, CheckFailure, DeviceSideCheck, FailureClass, Floor, FloorCheck,
    GoneResult, IdentityMethod, IdentityTuple, MaterialCost, MaterialFactory, MaterialWriter,
    NativeSpan, OpenFailure, OpenFailureCause, OpenOptions, PixelLayout, PreviewBackend,
    SampledFrame, StreamFacts,
};
use crate::clock::{Clock, Micros, blocking};
use crate::model::{BackendKind, CloseOutcome};
use crate::supervisor::ExitHook;
use crate::timecode::BandGeometry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A sample fault (§3.11.4 table A and the S run-time errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFault {
    StdioBusy,
    PixelReturnNonZero,
    ProbeReturnNonZero,
    SizeChangedDuringCall,
    /// A caller timeout: the backend is poisoned and the native call does not return.
    Poisoned,
    StdioRestoreFailed,
    StreamEnded,
    FrameStale,
    /// #402 C2c: `capture_frame_stale` only for a sample with a time floor (`after_us`); a
    /// sample without one, or with only `after_sequence`, delivers as usual (no frame arrived
    /// after the floor time, while the slot still holds older frames).
    TimeFloorUnmet,
}

/// Where a fault applies: an instance, a backend and the 1-based call number from which on
/// (`count` calls).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultAt {
    pub instance: String,
    pub kind: BackendKind,
    pub from_call: u64,
    pub count: u64,
}

/// A latch the test opens to let a hung close return.
#[derive(Default)]
pub struct Latch {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Latch {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn open(&self) {
        *lock(&self.open) = true;
        self.opened.notify_all();
    }

    fn wait(&self) {
        let mut open = lock(&self.open);
        while !*open {
            open = self
                .opened
                .wait(open)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A close that never returns (until its latch opens), with an action at its entry.
pub struct CloseHang {
    pub at: FaultAt,
    pub latch: Arc<Latch>,
    pub on_entry: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// #402 C2b: the fake frames show the fake APK's timecode band (C3's protocol §4.3), and the
/// fake taps reach the fake APK's flip mode.
#[derive(Clone)]
pub struct FakePattern {
    pub apk: Arc<FakeApk>,
    pub geometry: BandGeometry,
}

/// #402 C2b (D2 item 10): the stand-ins for S's host-side `adb shell` child. `run --fake` starts
/// a real `fake-child` process per S open, outside the job object; the tests record the calls.
pub trait FakeChildren: Send + Sync {
    /// Starts the stand-in of `instance`'s stream; returns its pid.
    fn spawn(&self, instance: &str, stream: &StreamFacts) -> Result<u32, String>;
    /// Stops it (the S close's `adb shell` child stop); a hung close never calls this.
    fn stop(&self, scid: u32) -> Result<(), String>;
}

/// #402 INT-1 (D2 item 10, §3.11.5 checklist item 1): the stand-in's arguments after
/// `fake-child`, shaped as the real child's (`adb.rs:487-502`): `-s <serial> shell` and the
/// server's arguments with `scid=<hex>`, so the checklist's four items (parent pid, start time,
/// `-s <E serial> shell`, `scid=<scid>`) identify it as they would the real one.
pub fn fake_child_arguments(serial: &str, stream: &StreamFacts) -> Vec<String> {
    let mut arguments = vec!["-s".to_owned(), serial.to_owned(), "shell".to_owned()];
    arguments.extend(stream.arguments.iter().cloned());
    arguments
}

/// #402 INT-1: what the `fake-child` subcommand accepts: `-s <serial> shell`, then the server's
/// arguments with exactly one `scid=<8 hex digits>`, and an optional `--max-seconds <s>`
/// (default 3600). Returns how long the stand-in lives, or `None` for another command line.
pub fn fake_child_lifetime(arguments: &[String]) -> Option<u64> {
    let [dash_s, serial, shell, rest @ ..] = arguments else {
        return None;
    };
    if dash_s != "-s" || serial.is_empty() || shell != "shell" {
        return None;
    }
    let mut seconds = 3_600;
    let mut scids = 0;
    let mut index = 0;
    while index < rest.len() {
        let argument = &rest[index];
        if argument == "--max-seconds" {
            seconds = rest.get(index + 1)?.parse::<u64>().ok()?;
            index += 2;
            continue;
        }
        if let Some(scid) = argument.strip_prefix("scid=") {
            if scid.len() != 8 || !scid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            scids += 1;
        }
        index += 1;
    }
    (scids == 1).then_some(seconds)
}

/// The real stand-ins: `<bench> fake-child -s <serial> shell <server arguments>` processes, not
/// in the job object.
pub struct ProcessChildren {
    pub executable: std::path::PathBuf,
    /// #402 INT-1: the plan's serial of each instance (the stand-in's `-s <serial>`).
    pub serials: BTreeMap<String, String>,
    children: Mutex<BTreeMap<u32, std::process::Child>>,
}

impl ProcessChildren {
    pub fn new(executable: std::path::PathBuf, serials: BTreeMap<String, String>) -> Arc<Self> {
        Arc::new(Self {
            executable,
            serials,
            children: Mutex::new(BTreeMap::new()),
        })
    }
}

impl FakeChildren for ProcessChildren {
    fn spawn(&self, instance: &str, stream: &StreamFacts) -> Result<u32, String> {
        let scid = stream.scid;
        let serial = self
            .serials
            .get(instance)
            .ok_or_else(|| format!("no serial for {instance} in the plan"))?;
        let child = std::process::Command::new(&self.executable)
            .arg("fake-child")
            .args(fake_child_arguments(serial, stream))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("the fake adb shell child did not start: {error}"))?;
        let pid = child.id();
        lock(&self.children).insert(scid, child);
        Ok(pid)
    }

    fn stop(&self, scid: u32) -> Result<(), String> {
        let Some(mut child) = lock(&self.children).remove(&scid) else {
            return Err(format!("no fake adb shell child for {scid:08x}"));
        };
        child
            .kill()
            .and_then(|()| child.wait().map(|_| ()))
            .map_err(|error| format!("the fake adb shell child did not stop: {error}"))
    }
}

/// The recorded stand-ins of the D1 tests.
#[derive(Default)]
pub struct RecordedChildren {
    pub spawned: Mutex<Vec<u32>>,
    pub stopped: Mutex<Vec<u32>>,
}

impl RecordedChildren {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn spawned(&self) -> Vec<u32> {
        lock(&self.spawned).clone()
    }

    pub fn stopped(&self) -> Vec<u32> {
        lock(&self.stopped).clone()
    }
}

impl FakeChildren for RecordedChildren {
    fn spawn(&self, _instance: &str, stream: &StreamFacts) -> Result<u32, String> {
        let scid = stream.scid;
        lock(&self.spawned).push(scid);
        Ok(50_000 + scid % 1_000)
    }

    fn stop(&self, scid: u32) -> Result<(), String> {
        lock(&self.stopped).push(scid);
        Ok(())
    }
}

/// The fake's script.
pub struct FakeScript {
    pub width: u32,
    pub height: u32,
    pub nemu_latency_us: Micros,
    pub scrcpy_latency_us: Micros,
    pub open_latency_us: Micros,
    pub sample_faults: Vec<(FaultAt, SampleFault)>,
    pub open_faults: Vec<(FaultAt, OpenFailureCause, bool)>,
    pub close_hangs: Vec<CloseHang>,
    pub close_unconfirmed: Vec<FaultAt>,
    /// The fake `T_est` bias of S frames (positive: estimates late), for the analysis fixtures.
    pub scrcpy_estimate_bias_us: i64,
    /// Actions run when a sample call starts (a test writes `STOP` or `ABORT` at a known call).
    pub on_sample: Vec<(FaultAt, Arc<dyn Fn() + Send + Sync>)>,
    /// #402 C2b: the frames carry the fake APK's timecode (every instance's frames, unless the
    /// instance has its own in `patterns`).
    pub pattern: Option<FakePattern>,
    /// #402 INT-1: each instance's own fake APK: its frames show that APK's timecode, at that
    /// APK's display size, and its taps reach that APK (D2: E2's frames showed E1's APK).
    pub patterns: BTreeMap<String, FakePattern>,
}

impl Default for FakeScript {
    fn default() -> Self {
        Self {
            width: 16,
            height: 9,
            nemu_latency_us: 20_000,
            scrcpy_latency_us: 1_000,
            open_latency_us: 50_000,
            sample_faults: Vec::new(),
            open_faults: Vec::new(),
            close_hangs: Vec::new(),
            close_unconfirmed: Vec::new(),
            scrcpy_estimate_bias_us: 0,
            on_sample: Vec::new(),
            pattern: None,
            patterns: BTreeMap::new(),
        }
    }
}

impl FakeScript {
    /// The fake APK whose timecode `instance`'s frames show.
    pub fn pattern_of(&self, instance: &str) -> Option<&FakePattern> {
        self.patterns.get(instance).or(self.pattern.as_ref())
    }
}

/// #402 INT-1: the fake APKs of `run --fake`: one per instance with `apk = true`, each on a
/// device clock of its own (boot offsets 7 s, 8 s, … apart by instance), with a display of the
/// plan's orientation (640 × 360 landscape, 360 × 640 portrait) and the band of cell 24, so each
/// instance's frames show its own timecode and its taps flip its own APK.
pub fn fake_apks(clock: &Arc<dyn Clock>, plan: &crate::plan::Plan) -> FakeApkSet {
    let mut apks = BTreeMap::new();
    let mut patterns = BTreeMap::new();
    for (index, instance) in plan
        .instances
        .iter()
        .filter(|instance| instance.apk)
        .enumerate()
    {
        let apk = FakeApk::new(
            Arc::clone(clock),
            7_000_000_000 + 1_000_000_000 * index as i64,
        );
        match instance.orientation {
            crate::plan::Orientation::Portrait => apk.set_display(360, 640),
            crate::plan::Orientation::Landscape => apk.set_display(640, 360),
        }
        patterns.insert(
            instance.name.clone(),
            FakePattern {
                apk: Arc::clone(&apk),
                geometry: BandGeometry {
                    cell: 24,
                    offset_x: 0,
                    offset_y: 0,
                },
            },
        );
        apks.insert(instance.name.clone(), apk);
    }
    FakeApkSet { apks, patterns }
}

/// #402 INT-1: the fake APKs by instance (for the connector) and the frames' patterns (for
/// [`FakeScript::patterns`]).
pub struct FakeApkSet {
    pub apks: BTreeMap<String, Arc<FakeApk>>,
    pub patterns: BTreeMap<String, FakePattern>,
}

/// #402 INT-1: the fake's margin of `meets` (`fresh_margin`, 30 ms, plan §5.3).
pub const FAKE_FRESH_MARGIN_US: i64 = 30_000;

fn applies(at: &FaultAt, instance: &str, kind: BackendKind, call: u64) -> bool {
    at.instance == instance
        && at.kind == kind
        && call >= at.from_call
        && call < at.from_call.saturating_add(at.count)
}

/// What the fake did, for the assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeCall {
    Open {
        instance: String,
        kind: BackendKind,
        scid: Option<u32>,
    },
    Sample {
        instance: String,
        kind: BackendKind,
        call: u64,
        after_close: bool,
    },
    Tap {
        instance: String,
    },
    Close {
        instance: String,
        kind: BackendKind,
    },
}

struct Counters {
    opens: BTreeMap<(String, BackendKind), u64>,
    samples: BTreeMap<(String, BackendKind), u64>,
    closes: BTreeMap<(String, BackendKind), u64>,
}

/// The fake backend factory and its shared record.
pub struct FakeDevice {
    clock: Arc<dyn Clock>,
    script: FakeScript,
    counters: Mutex<Counters>,
    calls: Mutex<Vec<FakeCall>>,
    next_scid: AtomicU32,
    live_streams: Mutex<BTreeMap<u32, (String, bool)>>,
    pub device_script: FakeCheckScript,
    check_calls: Mutex<Vec<String>>,
    pub boot_id: Mutex<String>,
    /// #402 C2b: the streams whose server was stopped on the device (C10 reconnect).
    ended_streams: Mutex<BTreeSet<u32>>,
    /// #402 C2b: the stand-ins of S's host-side `adb shell` child.
    children: Mutex<Option<Arc<dyn FakeChildren>>>,
}

/// How the fake device-side check answers.
#[derive(Default)]
pub struct FakeCheckScript {
    /// Methods whose negative control fails (the field is unsupported on this E).
    pub negative_fails: Vec<IdentityMethod>,
    /// Instances whose positive check never succeeds.
    pub positive_fails: Vec<String>,
    /// Instances whose disappearance check reports unconfirmed (truncated output).
    pub gone_unconfirmed: Vec<String>,
    /// Instances whose server is still there after the close.
    pub gone_residual: Vec<String>,
    /// Instances whose E restarts before the disappearance check.
    pub rebooted: Vec<String>,
    /// #402 C2d: how long the disappearance check of an instance takes on the clock (the
    /// queries until one sees the server gone).
    pub gone_after_us: BTreeMap<String, Micros>,
}

impl FakeDevice {
    pub fn new(
        clock: Arc<dyn Clock>,
        script: FakeScript,
        device_script: FakeCheckScript,
    ) -> Arc<Self> {
        Arc::new(Self {
            clock,
            script,
            counters: Mutex::new(Counters {
                opens: BTreeMap::new(),
                samples: BTreeMap::new(),
                closes: BTreeMap::new(),
            }),
            calls: Mutex::new(Vec::new()),
            next_scid: AtomicU32::new(0x0100_0000),
            live_streams: Mutex::new(BTreeMap::new()),
            device_script,
            check_calls: Mutex::new(Vec::new()),
            boot_id: Mutex::new("fake-boot-1".to_owned()),
            ended_streams: Mutex::new(BTreeSet::new()),
            children: Mutex::new(None),
        })
    }

    /// #402 C2b: S opens start a stand-in `adb shell` child through `children`.
    pub fn set_children(&self, children: Arc<dyn FakeChildren>) {
        *lock(&self.children) = Some(children);
    }

    fn children(&self) -> Option<Arc<dyn FakeChildren>> {
        lock(&self.children).clone()
    }

    pub fn calls(&self) -> Vec<FakeCall> {
        lock(&self.calls).clone()
    }

    pub fn check_calls(&self) -> Vec<String> {
        lock(&self.check_calls).clone()
    }

    /// The scids of the S servers the fake considers running.
    pub fn live_streams(&self) -> Vec<u32> {
        lock(&self.live_streams)
            .iter()
            .filter(|(_, (_, alive))| *alive)
            .map(|(scid, _)| *scid)
            .collect()
    }

    fn count(
        map: &mut BTreeMap<(String, BackendKind), u64>,
        instance: &str,
        kind: BackendKind,
    ) -> u64 {
        let counter = map.entry((instance.to_owned(), kind)).or_insert(0);
        *counter += 1;
        *counter
    }

    fn stream_facts(scid: u32) -> StreamFacts {
        StreamFacts {
            scid,
            local_port: Some(27_000 + (scid % 1000) as u16),
            arguments: vec![
                "CLASSPATH=/data/local/tmp/ac402-scrcpy-server.jar".to_owned(),
                "app_process".to_owned(),
                "/".to_owned(),
                "com.genymobile.scrcpy.Server".to_owned(),
                "5.0.1".to_owned(),
                format!("scid={scid:08x}"),
            ],
            remote_path: "/data/local/tmp/ac402-scrcpy-server.jar".to_owned(),
        }
    }
}

impl BackendFactory for Arc<FakeDevice> {
    fn open(
        &self,
        instance: &str,
        kind: BackendKind,
        _options: &OpenOptions,
    ) -> Result<Box<dyn PreviewBackend>, OpenFailure> {
        let call = FakeDevice::count(&mut lock(&self.counters).opens, instance, kind);
        self.clock.sleep_us(self.script.open_latency_us);
        let scid =
            (kind == BackendKind::Scrcpy).then(|| self.next_scid.fetch_add(1, Ordering::Relaxed));
        lock(&self.calls).push(FakeCall::Open {
            instance: instance.to_owned(),
            kind,
            scid,
        });
        if let Some((_, cause, cleanup)) = self
            .script
            .open_faults
            .iter()
            .find(|(at, _, _)| applies(at, instance, kind, call))
        {
            if let Some(scid) = scid {
                // The server started and the open's cleanup stopped it (or did not).
                lock(&self.live_streams).insert(scid, (instance.to_owned(), !*cleanup));
            }
            return Err(OpenFailure {
                cause: *cause,
                detail: format!("fake open failure {} on call {call}", cause.as_str()),
                stream: scid.map(FakeDevice::stream_facts),
                cleanup_confirmed: *cleanup,
            });
        }
        if let Some(scid) = scid {
            lock(&self.live_streams).insert(scid, (instance.to_owned(), true));
            if let Some(children) = self.children()
                && let Err(detail) = children.spawn(instance, &FakeDevice::stream_facts(scid))
            {
                lock(&self.live_streams).insert(scid, (instance.to_owned(), false));
                return Err(OpenFailure {
                    cause: OpenFailureCause::ShellSpawn,
                    detail,
                    stream: Some(FakeDevice::stream_facts(scid)),
                    cleanup_confirmed: true,
                });
            }
        }
        Ok(Box::new(FakeBackend {
            device: Arc::clone(self),
            instance: instance.to_owned(),
            kind,
            scid,
            poisoned: false,
            closed: false,
            sequence: 0,
        }))
    }
}

struct FakeBackend {
    device: Arc<FakeDevice>,
    instance: String,
    kind: BackendKind,
    scid: Option<u32>,
    poisoned: bool,
    closed: bool,
    sequence: u64,
}

impl PreviewBackend for FakeBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }

    fn sample(&mut self, floor: Option<Floor>) -> Result<SampledFrame, BackendFailure> {
        let device = Arc::clone(&self.device);
        let call = FakeDevice::count(
            &mut lock(&device.counters).samples,
            &self.instance,
            self.kind,
        );
        lock(&device.calls).push(FakeCall::Sample {
            instance: self.instance.clone(),
            kind: self.kind,
            call,
            after_close: self.closed,
        });
        for (at, action) in &device.script.on_sample {
            if applies(at, &self.instance, self.kind, call) {
                action();
            }
        }
        if self.poisoned {
            return Err(BackendFailure::new(
                "poisoned",
                FailureClass::Poisoned,
                "the fake backend is poisoned after a previous timeout",
            )
            .without_native_call());
        }
        if let Some(scid) = self.scid
            && lock(&device.ended_streams).contains(&scid)
        {
            return Err(BackendFailure::new(
                "scrcpy_stream_ended",
                FailureClass::CellPath,
                "fake: the stream's server was stopped on the device",
            )
            .without_native_call());
        }
        let time_floor = floor.is_some_and(|floor| floor.after_us.is_some());
        let fault = device
            .script
            .sample_faults
            .iter()
            .find(|(at, fault)| {
                applies(at, &self.instance, self.kind, call)
                    && (*fault != SampleFault::TimeFloorUnmet || time_floor)
            })
            .map(|(_, fault)| *fault);
        let latency = match self.kind {
            BackendKind::Nemu => device.script.nemu_latency_us,
            BackendKind::Scrcpy => device.script.scrcpy_latency_us,
        };
        if let Some(fault) = fault {
            return Err(match fault {
                SampleFault::StdioBusy => BackendFailure::new(
                    "vendor_stdio_redirect_busy",
                    FailureClass::StdioBusy,
                    "fake: another vendor call holds the stdio redirect",
                )
                .without_native_call(),
                SampleFault::PixelReturnNonZero => BackendFailure::new(
                    "nemu_capture_return_nonzero",
                    FailureClass::CellPath,
                    "fake: Nemu IPC capture failed with code -1",
                ),
                SampleFault::ProbeReturnNonZero => BackendFailure::new(
                    "nemu_probe_return_nonzero",
                    FailureClass::CellPath,
                    "fake: Nemu IPC resolution probe failed with code -1",
                ),
                SampleFault::SizeChangedDuringCall => BackendFailure::new(
                    "nemu_size_changed_during_call",
                    FailureClass::CellPath,
                    "fake: the frame dimensions changed during capture",
                ),
                SampleFault::Poisoned => {
                    self.poisoned = true;
                    self.device.clock.sleep_us(latency);
                    BackendFailure::new(
                        "poisoned",
                        FailureClass::Poisoned,
                        "fake: the worker timed out; the backend is poisoned",
                    )
                    .native_not_returned()
                }
                SampleFault::StdioRestoreFailed => BackendFailure::new(
                    "stdio_unrestored",
                    FailureClass::StdioUnrestored,
                    "fake: the vendor stdio restore failed",
                ),
                SampleFault::StreamEnded => BackendFailure::new(
                    "scrcpy_stream_ended",
                    FailureClass::CellPath,
                    "fake: the scrcpy stream ended",
                ),
                SampleFault::FrameStale => BackendFailure::new(
                    "capture_frame_stale",
                    FailureClass::Measured,
                    "fake: no frame after the floor within the fresh wait",
                ),
                SampleFault::TimeFloorUnmet => BackendFailure::new(
                    "capture_frame_stale",
                    FailureClass::Measured,
                    "fake: no frame after the time floor within the fresh wait",
                ),
            });
        }
        let started = device.clock.now_us();
        let bias = device.script.scrcpy_estimate_bias_us;
        let time_floor = floor.and_then(|floor| floor.after_us);
        // #402 INT-1 (F.s_late): the fake's `T_est` follows `scrcpy.rs:462-488`. The fake's
        // device clock is the host clock (PTS = T_src) and the transport takes `latency`, so the
        // offset minimum is `latency` (+ the scripted bias) and `T_est = T_src + latency + bias
        // − margin`. `meets` takes the first frame with `T_recv ≥ floor` and `T_est ≥ floor`:
        // the one whose source is `floor + margin − latency − bias` (or the call's start, when
        // later); the call returns when that frame arrives. (C2d added the margin to the offset
        // as well, so its `T_est` was `T_src + latency`: late by the latency.)
        let (source, ended) = match (self.kind, time_floor) {
            (BackendKind::Scrcpy, Some(floor_us)) => {
                let first_met = floor_us as i64 + FAKE_FRESH_MARGIN_US - latency as i64 - bias;
                let source = first_met.max(started as i64).max(0) as Micros;
                let arrival = source.saturating_add(latency).max(floor_us);
                device
                    .clock
                    .sleep_us(arrival.saturating_sub(device.clock.now_us()));
                (source, device.clock.now_us())
            }
            _ => {
                device.clock.sleep_us(latency);
                (started, device.clock.now_us())
            }
        };
        self.sequence += 1;
        let (layout, bytes_per_pixel) = match self.kind {
            BackendKind::Nemu => (PixelLayout::Rgba8, 4),
            BackendKind::Scrcpy => (PixelLayout::Rgb8, 3),
        };
        // #402 INT-1: the instance's own fake APK (or the script's one for every instance).
        let pattern = device.script.pattern_of(&self.instance).cloned();
        // With the pattern the frames have the fake APK's display size (it turns with ORIENT).
        let (width, height) = match &pattern {
            Some(pattern) => {
                let (width, height, _) = pattern.apk.display();
                (width, height)
            }
            None => (device.script.width, device.script.height),
        };
        let size = (width * height) as usize * bytes_per_pixel;
        let pixels = match &pattern {
            Some(pattern) => {
                let mut pixels = vec![128u8; size];
                if let Some(payload) = pattern.apk.payload_at(source) {
                    crate::timecode::render(
                        &mut pixels,
                        width,
                        bytes_per_pixel,
                        pattern.geometry,
                        &payload,
                    );
                }
                pixels
            }
            None => vec![(self.sequence % 251) as u8; size],
        };
        // S with a floor: the fake's `meets` inputs, as above.
        let floor_check = match (self.kind, time_floor) {
            (BackendKind::Scrcpy, Some(floor_us)) => {
                let margin = FAKE_FRESH_MARGIN_US;
                let offset = latency as i64 + bias;
                let t_est = source as i64 + offset - margin;
                Some(FloorCheck {
                    pts_us: source,
                    offset_min_us: Some(offset),
                    margin_us: margin as u64,
                    t_est_us: Some(t_est),
                    floor_us,
                    t_recv_us: Some(ended),
                    admitted: ended >= floor_us && t_est >= floor_us as i64,
                    t_src_known_us: Some(source),
                })
            }
            _ => None,
        };
        // #402 INT-1 (F.n_floor): N's two native calls (the probe, then the pixels), as the C1
        // timing points report them on the real backend; S has none.
        let native = match self.kind {
            BackendKind::Nemu => {
                let middle = started + ended.saturating_sub(started) / 2;
                vec![
                    NativeSpan {
                        kind: "probe",
                        start_us: Some(started),
                        end_us: Some(middle),
                        code: Some(0),
                    },
                    NativeSpan {
                        kind: "pixels",
                        start_us: Some(middle),
                        end_us: Some(ended),
                        code: Some(0),
                    },
                ]
            }
            BackendKind::Scrcpy => Vec::new(),
        };
        Ok(SampledFrame {
            width,
            height,
            layout,
            pixels,
            stream_sequence: (self.kind == BackendKind::Scrcpy).then_some(self.sequence),
            device_pts_us: (self.kind == BackendKind::Scrcpy).then_some(source),
            received_us: (self.kind == BackendKind::Scrcpy).then_some(ended),
            built_us: (self.kind == BackendKind::Nemu).then_some(ended),
            built_unix_ms: (self.kind == BackendKind::Nemu).then_some(device.clock.unix_ms()),
            native,
            known_source_us: Some(source),
            floor_check,
        })
    }

    fn tap(&mut self, x: i32, y: i32) -> Result<(), BackendFailure> {
        lock(&self.device.calls).push(FakeCall::Tap {
            instance: self.instance.clone(),
        });
        self.device.clock.sleep_us(2_000);
        if let Some(pattern) = self.device.script.pattern_of(&self.instance) {
            pattern.apk.tap(x, y);
        }
        Ok(())
    }

    fn close(&mut self) -> CloseOutcome {
        let device = Arc::clone(&self.device);
        let call = FakeDevice::count(
            &mut lock(&device.counters).closes,
            &self.instance,
            self.kind,
        );
        lock(&device.calls).push(FakeCall::Close {
            instance: self.instance.clone(),
            kind: self.kind,
        });
        self.closed = true;
        if let Some(hang) = device
            .script
            .close_hangs
            .iter()
            .find(|hang| applies(&hang.at, &self.instance, self.kind, call))
        {
            if let Some(action) = &hang.on_entry {
                action();
            }
            let latch = Arc::clone(&hang.latch);
            blocking(device.clock.as_ref(), || latch.wait());
            return CloseOutcome::Unconfirmed("the hung fake close was released".to_owned());
        }
        if self.poisoned {
            return CloseOutcome::Unconfirmed(
                "the backend is poisoned; its worker is unconfirmed".to_owned(),
            );
        }
        if device
            .script
            .close_unconfirmed
            .iter()
            .any(|at| applies(at, &self.instance, self.kind, call))
        {
            return CloseOutcome::Unconfirmed("fake: the close is unconfirmed".to_owned());
        }
        if let Some(scid) = self.scid {
            lock(&device.live_streams).insert(scid, (self.instance.clone(), false));
            if let Some(children) = device.children()
                && let Err(detail) = children.stop(scid)
            {
                return CloseOutcome::Unconfirmed(detail);
            }
        }
        CloseOutcome::Confirmed
    }

    fn poisoned(&self) -> bool {
        self.poisoned
    }

    fn stream(&self) -> Option<StreamFacts> {
        self.scid.map(FakeDevice::stream_facts)
    }
}

impl DeviceSideCheck for Arc<FakeDevice> {
    fn negative_control(&self, instance: &str, method: IdentityMethod) -> Result<(), CheckFailure> {
        lock(&self.check_calls).push(format!("negative:{instance}:{method:?}"));
        if self.device_script.negative_fails.contains(&method) {
            return Err(CheckFailure::Unconfirmed(format!(
                "fake: {method:?} is not supported on this image"
            )));
        }
        Ok(())
    }

    fn positive(
        &self,
        instance: &str,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<IdentityTuple, CheckFailure> {
        lock(&self.check_calls).push(format!("positive:{instance}:{:08x}", stream.scid));
        if self
            .device_script
            .positive_fails
            .iter()
            .any(|name| name == instance)
        {
            return Err(CheckFailure::Unconfirmed(
                "fake: the output is truncated (no end marker)".to_owned(),
            ));
        }
        Ok(IdentityTuple {
            serial_label: instance.to_owned(),
            method,
            pid: 4_000 + stream.scid % 1000,
            start_ticks: 12_345,
            boot_id: lock(&self.boot_id).clone(),
            argv: stream.expected_argv(),
            server_path: stream.remote_path.clone(),
            scid: stream.scid,
            forward: stream
                .local_port
                .map(|port| (port, format!("localabstract:scrcpy_{}", stream.scid_text()))),
        })
    }

    fn gone(&self, instance: &str, identity: &IdentityTuple) -> Result<GoneResult, CheckFailure> {
        lock(&self.check_calls).push(format!("gone:{instance}:{:08x}", identity.scid));
        if let Some(delay) = self.device_script.gone_after_us.get(instance) {
            self.clock.sleep_us(*delay);
        }
        if self
            .device_script
            .gone_unconfirmed
            .iter()
            .any(|name| name == instance)
        {
            return Err(CheckFailure::Unconfirmed(
                "fake: the process table output is truncated".to_owned(),
            ));
        }
        if self
            .device_script
            .gone_residual
            .iter()
            .any(|name| name == instance)
        {
            return Err(CheckFailure::Residual(
                "fake: the same identity still matches after 2 s".to_owned(),
            ));
        }
        if self
            .device_script
            .rebooted
            .iter()
            .any(|name| name == instance)
        {
            return Ok(GoneResult::Rebooted);
        }
        let alive = lock(&self.live_streams)
            .get(&identity.scid)
            .is_some_and(|(_, alive)| *alive);
        if alive {
            return Err(CheckFailure::Residual(
                "fake: the server is still running".to_owned(),
            ));
        }
        Ok(GoneResult::Gone)
    }

    fn stream_absent(
        &self,
        instance: &str,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<(), CheckFailure> {
        lock(&self.check_calls).push(format!("absent:{instance}:{method:?}:{:08x}", stream.scid));
        let alive = lock(&self.live_streams)
            .get(&stream.scid)
            .is_some_and(|(_, alive)| *alive);
        if alive {
            return Err(CheckFailure::Residual(
                "fake: the failed open's server is still running".to_owned(),
            ));
        }
        Ok(())
    }

    fn stop_server(&self, instance: &str, identity: &IdentityTuple) -> Result<u32, CheckFailure> {
        lock(&self.check_calls).push(format!("stop:{instance}:{:08x}", identity.scid));
        if *lock(&self.boot_id) != identity.boot_id {
            return Err(CheckFailure::Unconfirmed(
                "fake: boot_id differs; nothing is stopped".to_owned(),
            ));
        }
        let mut live = lock(&self.live_streams);
        let running = live
            .get(&identity.scid)
            .is_some_and(|(owner, alive)| *alive && owner == instance);
        if !running {
            return Err(CheckFailure::Unconfirmed(format!(
                "fake: no running server {:08x} on {instance}",
                identity.scid
            )));
        }
        live.insert(identity.scid, (instance.to_owned(), false));
        drop(live);
        lock(&self.ended_streams).insert(identity.scid);
        Ok(identity.pid)
    }
}

/// The fake B_model writer: counts the bytes and events the real one would write.
pub struct FakeMaterials {
    pub writes: AtomicU64,
}

impl MaterialFactory for Arc<FakeMaterials> {
    fn open(&self, _instance: &str, _segment: &str) -> Result<Box<dyn MaterialWriter>, String> {
        Ok(Box::new(FakeMaterialWriter {
            owner: Arc::clone(self),
        }))
    }
}

struct FakeMaterialWriter {
    owner: Arc<FakeMaterials>,
}

impl MaterialWriter for FakeMaterialWriter {
    fn write(&mut self, frame: &SampledFrame) -> Result<MaterialCost, BackendFailure> {
        self.owner.writes.fetch_add(1, Ordering::Relaxed);
        Ok(MaterialCost {
            bytes: frame.pixels.len() as u64,
            events: 11,
        })
    }
}

/// The test exit: records the code instead of ending the process.
#[derive(Default)]
pub struct RecordedExit {
    codes: Mutex<Vec<i32>>,
    exited: Condvar,
}

impl RecordedExit {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn codes(&self) -> Vec<i32> {
        lock(&self.codes).clone()
    }

    /// Waits (as a test, not a participant) for the first exit code.
    pub fn wait_first(&self) -> i32 {
        let mut codes = lock(&self.codes);
        while codes.is_empty() {
            codes = self
                .exited
                .wait(codes)
                .unwrap_or_else(PoisonError::into_inner);
        }
        codes[0]
    }
}

impl ExitHook for RecordedExit {
    fn exit(&self, code: i32) {
        lock(&self.codes).push(code);
        self.exited.notify_all();
    }

    fn pending(&self, _since_us: Micros) {
        // Self-exit is off: the pending state stands for the exit the test waits for.
        lock(&self.codes).push(-1);
        self.exited.notify_all();
    }
}
