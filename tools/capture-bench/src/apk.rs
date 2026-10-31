// SPDX-License-Identifier: AGPL-3.0-only

//! #402 C2b: the test-pattern APK's control, per C3's protocol v1
//! (`tools/capture-bench-apk/PROTOCOL.md` on `exp/402-test-apk`, §5-§8).
//!
//! - [`ApkClient`]: the line protocol over one connection (HELLO, PING, INFO, MODE, STEP,
//!   ORIENT, PUT, RECORDS, BYE); every response is checked for `ok` and its verb.
//! - [`calibrate`]: the plan's §5.1 calibration from PING samples: the device time of a sample is
//!   `(t_rx_dev_ns + t_tx_dev_ns) / 2`, converted to µs; `θ = device − (s + r) / 2` and
//!   `u = (r − s) / 2` of the minimum-round-trip sample.
//! - [`ApkStreams`]: the record cursors of one APK process and the files they go to. n, k and
//!   the counter restart with the APK process (HELLO `instance`), so each APK process of a run
//!   writes its own file; the device clock restarts with the instance, so a reset starts a new
//!   clock epoch and needs a new calibration.
//! - [`AdbApkConnector`]: the real link (`adb -s <E> forward tcp:0
//!   localabstract:capture_bench_pattern`, then TCP), removing only its own forward.
//! - [`FakeApk`]: the deterministic APK of the fake backend (D1, D2).

use crate::clock::{Clock, Micros};
use crate::timecode::{BandGeometry, Payload};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's package (PROTOCOL §1).
pub const PACKAGE: &str = "io.github.hs7097.capturebench.pattern";
/// The time service's abstract socket (PROTOCOL §1).
pub const SOCKET: &str = "localabstract:capture_bench_pattern";
/// The wire protocol the bench speaks (HELLO `protocol`).
pub const PROTOCOL: u64 = 1;
/// The device clock may drift this far from the last calibration's prediction before the bench
/// takes it for a reset (an instance restart): 1 s.
pub const CLOCK_RESET_TOLERANCE_US: f64 = 1_000_000.0;

/// A failed APK call: the APK's error code, or the bench's (`apk_io`, `apk_bad_response`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApkError {
    pub code: String,
    pub detail: String,
}

impl ApkError {
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }

    /// Whether the connection is unusable after this error (PROTOCOL §5.3 "closed").
    pub fn closes_connection(&self) -> bool {
        matches!(
            self.code.as_str(),
            "apk_io"
                | "apk_bad_response"
                | "line_too_long"
                | "payload_too_large"
                | "payload_short"
                | "peer_not_allowed"
                | "too_many_connections"
                | "io_error"
                | "internal_error"
        )
    }
}

/// One connection to the APK's time service.
pub trait ApkLink: Send {
    /// Sends one request line (without its LF) and returns the response line's JSON.
    fn request(&mut self, line: &str) -> Result<Value, ApkError>;
    /// `PUT <slot> <length>` followed by the bytes.
    fn put(&mut self, slot: &str, bytes: &[u8]) -> Result<Value, ApkError>;
    /// Ends the connection and removes what the link created (its own forward).
    fn close(&mut self) -> Result<(), String>;
}

/// Opens connections to the APK of an instance.
pub trait ApkConnector: Send + Sync {
    fn connect(&self, instance: &str) -> Result<Box<dyn ApkLink>, ApkError>;
}

/// What HELLO reported.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hello {
    /// 16 hex digits per APK process (PROTOCOL §5.2).
    pub instance: String,
    pub t_now_dev_ns: i64,
    pub raw: Value,
}

/// One PING round trip: host times on the bench clock (µs), device times in ns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PingSample {
    pub sent_us: Micros,
    pub received_us: Micros,
    pub t_rx_dev_ns: i64,
    pub t_tx_dev_ns: i64,
}

impl PingSample {
    pub fn rtt_us(&self) -> u64 {
        self.received_us.saturating_sub(self.sent_us)
    }

    /// The device time of the sample in µs: the midpoint of the APK's two clock reads.
    pub fn device_us(&self) -> f64 {
        (self.t_rx_dev_ns as f64 + self.t_tx_dev_ns as f64) / 2_000.0
    }

    /// The host midpoint in µs.
    pub fn host_us(&self) -> f64 {
        (self.sent_us as f64 + self.received_us as f64) / 2.0
    }

    /// `θ = device − (s + r) / 2` in µs.
    pub fn theta_us(&self) -> f64 {
        self.device_us() - self.host_us()
    }
}

/// One calibration (plan §5.1 items 1, 2): `map(T) = [T − θ − u, T − θ + u]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Calibration {
    /// The device time of the minimum-round-trip sample, µs.
    pub t_dev_us: f64,
    /// The host time of the same sample (the bench clock), µs.
    pub t_host_us: f64,
    /// Device minus host, µs.
    pub theta_us: f64,
    /// Half the minimum round trip, µs.
    pub u_us: f64,
    pub rtt_min_us: u64,
    pub samples: usize,
}

impl Calibration {
    /// The host interval `[lo, hi]` (µs on the bench clock) of a device time in ns.
    pub fn map_dev_ns(&self, t_dev_ns: i64) -> (f64, f64) {
        let host = t_dev_ns as f64 / 1_000.0 - self.theta_us;
        (host - self.u_us, host + self.u_us)
    }

    /// The device time (µs) this calibration predicts at host time `host_us`.
    pub fn predict_dev_us(&self, host_us: f64) -> f64 {
        host_us + self.theta_us
    }
}

/// The calibration of `samples`: θ and u of the sample with the smallest round trip (ties:
/// the earliest). `None` without samples.
pub fn calibrate(samples: &[PingSample]) -> Option<Calibration> {
    let best = samples
        .iter()
        .enumerate()
        .min_by_key(|(index, sample)| (sample.rtt_us(), *index))
        .map(|(_, sample)| *sample)?;
    Some(Calibration {
        t_dev_us: best.device_us(),
        t_host_us: best.host_us(),
        theta_us: best.theta_us(),
        u_us: best.rtt_us() as f64 / 2.0,
        rtt_min_us: best.rtt_us(),
        samples: samples.len(),
    })
}

/// The interpolation residual and the drift of `current` against the earlier calibrations of
/// the same clock epoch (plan §5.1 item 2): the residual is `|θ − θ_predicted|`, θ predicted by
/// the line through the last two earlier calibrations (0 with fewer than two); the drift is the
/// slope of θ against the device time since the last calibration, in parts per million.
pub fn residual_and_drift(previous: &[Calibration], current: &Calibration) -> (f64, Option<f64>) {
    let drift = previous.last().and_then(|last| {
        let span = current.t_dev_us - last.t_dev_us;
        if span.abs() > 0.0 {
            Some((current.theta_us - last.theta_us) / span * 1_000_000.0)
        } else {
            None
        }
    });
    let residual = match previous {
        [.., a, b] => {
            let span = b.t_dev_us - a.t_dev_us;
            if span.abs() > 0.0 {
                let slope = (b.theta_us - a.theta_us) / span;
                let predicted = b.theta_us + slope * (current.t_dev_us - b.t_dev_us);
                (current.theta_us - predicted).abs()
            } else {
                0.0
            }
        }
        _ => 0.0,
    };
    (residual, drift)
}

/// One page of `RECORDS`.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordsPage {
    pub next: u64,
    pub newest: u64,
    pub lost: u64,
    pub records: Vec<Value>,
}

/// The protocol client on one link.
pub struct ApkClient {
    link: Box<dyn ApkLink>,
    clock: Arc<dyn Clock>,
    tokens: u64,
}

fn checked(response: Value, verb: &str) -> Result<Value, ApkError> {
    if response["ok"] == json!(true) && response["cmd"] == json!(verb) {
        return Ok(response);
    }
    if response["ok"] == json!(false) {
        return Err(ApkError::new(
            response["error"].as_str().unwrap_or("apk_bad_response"),
            response["detail"].as_str().unwrap_or_default(),
        ));
    }
    Err(ApkError::new(
        "apk_bad_response",
        format!("{verb}: unexpected response {response}"),
    ))
}

impl ApkClient {
    pub fn new(link: Box<dyn ApkLink>, clock: Arc<dyn Clock>) -> Self {
        Self {
            link,
            clock,
            tokens: 0,
        }
    }

    fn call(&mut self, line: &str, verb: &str) -> Result<Value, ApkError> {
        let response = self.link.request(line)?;
        checked(response, verb)
    }

    pub fn hello(&mut self) -> Result<Hello, ApkError> {
        let raw = self.call("HELLO", "HELLO")?;
        if raw["protocol"].as_u64() != Some(PROTOCOL) {
            return Err(ApkError::new(
                "apk_protocol_mismatch",
                format!("HELLO protocol {}", raw["protocol"]),
            ));
        }
        let instance = raw["instance"]
            .as_str()
            .filter(|instance| !instance.is_empty())
            .ok_or_else(|| ApkError::new("apk_bad_response", "HELLO without an instance"))?
            .to_owned();
        let t_now_dev_ns = raw["t_now_dev_ns"]
            .as_i64()
            .ok_or_else(|| ApkError::new("apk_bad_response", "HELLO without t_now_dev_ns"))?;
        Ok(Hello {
            instance,
            t_now_dev_ns,
            raw,
        })
    }

    /// One PING: the bench clock is read right before the request and right after the reply.
    pub fn ping(&mut self) -> Result<PingSample, ApkError> {
        self.tokens += 1;
        let token = format!("p{}", self.tokens);
        let sent_us = self.clock.now_us();
        let response = self.link.request(&format!("PING {token}"));
        let received_us = self.clock.now_us();
        let response = checked(response?, "PING")?;
        if response["token"] != json!(token) {
            return Err(ApkError::new(
                "apk_bad_response",
                format!("PING {token} answered {}", response["token"]),
            ));
        }
        let read = |name: &str| {
            response[name]
                .as_i64()
                .ok_or_else(|| ApkError::new("apk_bad_response", format!("PING without {name}")))
        };
        Ok(PingSample {
            sent_us,
            received_us,
            t_rx_dev_ns: read("t_rx_dev_ns")?,
            t_tx_dev_ns: read("t_tx_dev_ns")?,
        })
    }

    pub fn info(&mut self) -> Result<Value, ApkError> {
        self.call("INFO", "INFO")
    }

    /// `MODE <mode> [key=value …]`.
    pub fn mode(&mut self, mode: &str, options: &[(&str, String)]) -> Result<Value, ApkError> {
        let mut line = format!("MODE {mode}");
        for (key, value) in options {
            line.push_str(&format!(" {key}={value}"));
        }
        self.call(&line, "MODE")
    }

    /// `STEP [level=…]`; returns k.
    pub fn step(&mut self, level: Option<u8>) -> Result<u64, ApkError> {
        let line = match level {
            Some(level) => format!("STEP level={level}"),
            None => "STEP".to_owned(),
        };
        let response = self.call(&line, "STEP")?;
        response["k"]
            .as_u64()
            .ok_or_else(|| ApkError::new("apk_bad_response", "STEP without k"))
    }

    pub fn orient(&mut self, value: &str) -> Result<Value, ApkError> {
        self.call(&format!("ORIENT {value}"), "ORIENT")
    }

    pub fn put(&mut self, slot: &str, bytes: &[u8]) -> Result<Value, ApkError> {
        checked(self.link.put(slot, bytes)?, "PUT")
    }

    pub fn records(&mut self, stream: &str, after: u64, max: u32) -> Result<RecordsPage, ApkError> {
        let response = self.call(&format!("RECORDS {stream} {after} {max}"), "RECORDS")?;
        let read = |name: &str| {
            response[name]
                .as_u64()
                .ok_or_else(|| ApkError::new("apk_bad_response", format!("RECORDS without {name}")))
        };
        Ok(RecordsPage {
            next: read("next")?,
            newest: read("newest")?,
            lost: read("lost")?,
            records: response["records"].as_array().cloned().unwrap_or_default(),
        })
    }

    /// `BYE` (best effort) and the link's own cleanup, whose failure is returned.
    pub fn close(mut self) -> Result<(), String> {
        let _ = self.link.request("BYE");
        self.link.close()
    }
}

/// The band geometry INFO reports: the layout's cell and the view's offset in the frame.
pub fn geometry_of_info(info: &Value) -> Option<BandGeometry> {
    let cell = info["layout"]["cell"]
        .as_u64()
        .or_else(|| info["cell"].as_u64())?;
    Some(BandGeometry {
        cell: u32::try_from(cell).ok()?,
        offset_x: u32::try_from(info["view"]["x"].as_u64().unwrap_or(0)).ok()?,
        offset_y: u32::try_from(info["view"]["y"].as_u64().unwrap_or(0)).ok()?,
    })
}

/// The record types the analysis reads from `apk-<instance>.jsonl` (PROTOCOL §8).
pub fn analysis_record(record: &Value) -> bool {
    matches!(record["type"].as_str(), Some("flip" | "step" | "vsync"))
}

/// The APK file of an APK process: `apk-<instance>.jsonl` for the first APK process of the
/// run, `apk-<instance>.e<epoch>.jsonl` for each later one (n, k and the counter restarted).
pub fn apk_file_name(instance: &str, epoch: u32) -> String {
    if epoch <= 1 {
        format!("apk-{instance}.jsonl")
    } else {
        format!("apk-{instance}.e{epoch}.jsonl")
    }
}

/// The file of the APK's other records (mode, config, orient, put, …): not an `apk-*` name, so
/// the analysis never reads them as APK records.
pub fn apk_raw_file_name(instance: &str) -> String {
    format!("apkraw-{instance}.jsonl")
}

/// The cursors and epochs of one instance's APK.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ApkStreams {
    /// The HELLO `instance` of the APK process the cursors belong to.
    pub apk_instance: Option<String>,
    /// 1 for the run's first APK process, +1 at each APK restart.
    pub epoch: u32,
    /// 1 for the device clock of the run's first calibration, +1 at each reset.
    pub clock_epoch: u32,
    pub vsync_after: u64,
    pub event_after: u64,
}

/// What a HELLO means for the streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HelloChange {
    /// The APK process is another one than before (n, k and the counter start over).
    pub apk_restarted: bool,
    /// The device clock is not the one of the last calibration.
    pub clock_reset: bool,
}

impl ApkStreams {
    /// Takes a HELLO at host time `host_us`; `last` is the newest calibration of the current
    /// clock epoch, if any. A new APK process starts a new epoch and resets the cursors; a
    /// device time away from the calibration's prediction by more than
    /// [`CLOCK_RESET_TOLERANCE_US`] starts a new clock epoch.
    pub fn on_hello(
        &mut self,
        hello: &Hello,
        host_us: Micros,
        last: Option<&Calibration>,
    ) -> HelloChange {
        let apk_restarted = self
            .apk_instance
            .as_deref()
            .is_some_and(|known| known != hello.instance);
        let first = self.apk_instance.is_none();
        if first || apk_restarted {
            self.apk_instance = Some(hello.instance.clone());
            self.epoch += 1;
            self.vsync_after = 0;
            self.event_after = 0;
        }
        let clock_reset = last.is_some_and(|calibration| {
            let predicted = calibration.predict_dev_us(host_us as f64);
            (hello.t_now_dev_ns as f64 / 1_000.0 - predicted).abs() > CLOCK_RESET_TOLERANCE_US
        });
        if self.clock_epoch == 0 {
            self.clock_epoch = 1;
        } else if clock_reset {
            self.clock_epoch += 1;
        }
        HelloChange {
            apk_restarted,
            clock_reset,
        }
    }

    /// The marks every written APK record carries.
    pub fn marks(&self) -> Value {
        json!({
            "apk_instance": self.apk_instance,
            "apk_epoch": self.epoch,
            "clock_epoch": self.clock_epoch,
        })
    }
}

/// Adds the stream marks to an APK record.
pub fn marked(record: &Value, marks: &Value) -> Value {
    let mut line = record.clone();
    if let (Value::Object(map), Value::Object(extra)) = (&mut line, marks) {
        for (key, value) in extra {
            map.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    line
}

// ---- the real link ----

/// The real connector: an adb forward of the APK's socket to a local port the adb server
/// picks, then TCP; only the whitelisted serial of the instance is named.
pub struct AdbApkConnector {
    pub adb_path: String,
    /// instance name → adb serial (whitelisted).
    pub serials: BTreeMap<String, String>,
    pub timeout: std::time::Duration,
}

struct AdbApkLink {
    adb: actingcommand_device::Adb,
    serial: String,
    port: u16,
    stream: std::net::TcpStream,
    reader: std::io::BufReader<std::net::TcpStream>,
    closed: bool,
}

impl AdbApkLink {
    fn read_response(&mut self) -> Result<Value, ApkError> {
        use std::io::BufRead;
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .map_err(|error| ApkError::new("apk_io", format!("read: {error}")))?;
        if read == 0 {
            return Err(ApkError::new("apk_io", "the APK closed the connection"));
        }
        serde_json::from_str(line.trim_end())
            .map_err(|error| ApkError::new("apk_bad_response", format!("{error}: {line}")))
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), ApkError> {
        use std::io::Write;
        self.stream
            .write_all(bytes)
            .and_then(|()| self.stream.flush())
            .map_err(|error| ApkError::new("apk_io", format!("write: {error}")))
    }
}

impl ApkLink for AdbApkLink {
    fn request(&mut self, line: &str) -> Result<Value, ApkError> {
        self.write_all(format!("{line}\n").as_bytes())?;
        self.read_response()
    }

    fn put(&mut self, slot: &str, bytes: &[u8]) -> Result<Value, ApkError> {
        self.write_all(format!("PUT {slot} {}\n", bytes.len()).as_bytes())?;
        self.write_all(bytes)?;
        self.read_response()
    }

    fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        // Only the forward this link created.
        let local = format!("tcp:{}", self.port);
        self.adb
            .run(&[
                "-s",
                self.serial.as_str(),
                "forward",
                "--remove",
                local.as_str(),
            ])
            .map(|_| ())
            .map_err(|error| format!("adb forward --remove {local}: {}", error.message()))
    }
}

impl ApkConnector for AdbApkConnector {
    fn connect(&self, instance: &str) -> Result<Box<dyn ApkLink>, ApkError> {
        let serial = self
            .serials
            .get(instance)
            .cloned()
            .ok_or_else(|| ApkError::new("apk_unknown_instance", instance.to_owned()))?;
        let adb = actingcommand_device::Adb::new(actingcommand_device::AdbConfig {
            adb_path: self.adb_path.clone(),
            command_timeout: self.timeout,
        });
        let forwarded = adb
            .run(&["-s", serial.as_str(), "forward", "tcp:0", SOCKET])
            .map_err(|error| ApkError::new("apk_forward_failed", error.message().to_owned()))?;
        let port = forwarded.stdout.trim().parse::<u16>().map_err(|_| {
            ApkError::new(
                "apk_forward_failed",
                format!("adb forward reported {:?}", forwarded.stdout),
            )
        })?;
        let connected = std::net::TcpStream::connect(("127.0.0.1", port)).and_then(|stream| {
            stream.set_read_timeout(Some(self.timeout))?;
            stream.set_write_timeout(Some(self.timeout))?;
            stream.set_nodelay(true)?;
            let reader = stream.try_clone()?;
            Ok((stream, reader))
        });
        match connected {
            Ok((stream, reader)) => Ok(Box::new(AdbApkLink {
                adb,
                serial,
                port,
                stream,
                reader: std::io::BufReader::new(reader),
                closed: false,
            })),
            Err(error) => {
                let local = format!("tcp:{port}");
                let removed = adb
                    .run(&["-s", serial.as_str(), "forward", "--remove", local.as_str()])
                    .map(|_| ())
                    .map_err(|error| error.message().to_owned());
                Err(ApkError::new(
                    "apk_connect_failed",
                    format!("TCP to {local}: {error}; forward removed: {removed:?}"),
                ))
            }
        }
    }
}

// ---- the fake APK ----

/// The fake APK's state: one APK process on a device clock that runs at the bench clock's rate
/// with an offset (`device_ns = host_us × 1000 + offset_ns`).
struct FakeApkState {
    instance: String,
    restarts: u64,
    offset_ns: i64,
    mode: String,
    mode_seq: u64,
    step_k: u64,
    step_level: u8,
    flip_n: u64,
    flip_j: u8,
    flip_level: u8,
    orientation: String,
    events: Vec<Value>,
    vsync: Vec<Value>,
    slots: BTreeMap<String, usize>,
    /// The device's processing time between the two PING clock reads, ns.
    ping_hold_ns: i64,
    /// One-way transport delay of a PING each way, µs (the fake advances no clock; it adds the
    /// delay to the device times it reports).
    ping_one_way_us: i64,
    /// The band's cell edge INFO reports (the fake frames draw the band with it).
    cell: u32,
}

/// The fake APK (PROTOCOL §5 on an in-memory link).
pub struct FakeApk {
    clock: Arc<dyn Clock>,
    state: Mutex<FakeApkState>,
    /// Frames are 16 ms apart on the device clock.
    pub frame_period_ns: i64,
}

impl FakeApk {
    pub fn new(clock: Arc<dyn Clock>, offset_ns: i64) -> Arc<Self> {
        Arc::new(Self {
            clock,
            state: Mutex::new(FakeApkState {
                instance: "fa00000000000001".to_owned(),
                restarts: 0,
                offset_ns,
                mode: "pattern".to_owned(),
                mode_seq: 0,
                step_k: 0,
                step_level: 0,
                flip_n: 0,
                flip_j: 0,
                flip_level: 0,
                orientation: "landscape".to_owned(),
                events: Vec::new(),
                vsync: Vec::new(),
                slots: BTreeMap::new(),
                ping_hold_ns: 1_000,
                ping_one_way_us: 0,
                cell: 24,
            }),
            frame_period_ns: 16_000_000,
        })
    }

    /// The device time now.
    pub fn device_now_ns(&self) -> i64 {
        let state = lock(&self.state);
        self.clock.now_us() as i64 * 1_000 + state.offset_ns
    }

    /// The device time of a host time.
    pub fn device_ns_at(&self, host_us: Micros) -> i64 {
        host_us as i64 * 1_000 + lock(&self.state).offset_ns
    }

    /// The APK process restarts (n, k and the counter start over); with `reboot` the device
    /// clock restarts too (an instance restart).
    pub fn restart(&self, reboot: bool) {
        let mut state = lock(&self.state);
        state.restarts += 1;
        state.instance = format!("fa{:014x}", state.restarts + 1);
        if reboot {
            // The device clock starts again: 5 s after the instance's new boot.
            let now_ns = self.clock.now_us() as i64 * 1_000;
            state.offset_ns = 5_000_000_000 - now_ns;
        }
        state.mode = "pattern".to_owned();
        state.step_k = 0;
        state.step_level = 0;
        state.flip_n = 0;
        state.events.clear();
        state.vsync.clear();
    }

    /// The one-way PING delay (µs) the fake reports, for the calibration tests.
    pub fn set_ping_one_way_us(&self, delay: i64) {
        lock(&self.state).ping_one_way_us = delay;
    }

    /// The cell edge INFO reports.
    pub fn set_cell(&self, cell: u32) {
        lock(&self.state).cell = cell;
    }

    pub fn orientation(&self) -> String {
        lock(&self.state).orientation.clone()
    }

    pub fn mode(&self) -> String {
        lock(&self.state).mode.clone()
    }

    /// The timecode payload of the frame the APK shows at host time `host_us`: the vsync of the
    /// frame period that contains it.
    pub fn payload_at(&self, host_us: Micros) -> Option<Payload> {
        let state = lock(&self.state);
        let device = host_us as i64 * 1_000 + state.offset_ns;
        let counter = device.div_euclid(self.frame_period_ns);
        let t_vsync = counter * self.frame_period_ns;
        let (mode, event, aux) = match state.mode.as_str() {
            "pattern" => (1, 0, 0),
            "flip" => (
                2,
                (state.flip_n & 0x00ff_ffff) as u32,
                (state.flip_j << 4) | (state.flip_level << 3),
            ),
            "step" => (
                3,
                (state.step_k & 0x00ff_ffff) as u32,
                state.step_level << 3,
            ),
            _ => return None,
        };
        Some(Payload {
            version: 1,
            mode,
            seq: counter as u32,
            t_vsync_dev_ns: t_vsync,
            event,
            aux,
        })
    }

    /// A tap at `(x, y)` on a `width` × `height` view (flip mode: n + 1, j of the 4×4 grid).
    pub fn tap(&self, x: i32, y: i32, width: i32, height: i32) {
        let now = self.clock.now_us();
        let mut state = lock(&self.state);
        if state.mode != "flip" {
            return;
        }
        state.flip_n += 1;
        let column = (4 * x / width.max(1)).clamp(0, 3);
        let row = (4 * y / height.max(1)).clamp(0, 3);
        state.flip_j = (row * 4 + column) as u8;
        state.flip_level ^= 1;
        let device = now as i64 * 1_000 + state.offset_ns;
        let first_draw = (device.div_euclid(self.frame_period_ns) + 1) * self.frame_period_ns;
        let record = json!({
            "type": "flip", "n": state.flip_n, "j": state.flip_j, "x": x, "y": y,
            "t_evt_dev_ns": device, "t_evt_resolution": "ns", "t_handled_dev_ns": device + 100_000,
            "counter": first_draw / self.frame_period_ns, "t_vsync_first_draw_dev_ns": first_draw,
            "t_drawn_dev_ns": first_draw + 4_000_000, "superseded_by": null, "null_reasons": {},
        });
        Self::push_event(&mut state, record);
        Self::push_vsync(&mut state, first_draw, self.frame_period_ns);
    }

    fn push_event(state: &mut FakeApkState, mut record: Value) {
        let seq = state.events.len() as u64 + 1;
        if let Value::Object(map) = &mut record {
            map.insert("stream".to_owned(), json!("event"));
            map.insert("seq".to_owned(), json!(seq));
        }
        state.events.push(record);
    }

    fn push_vsync(state: &mut FakeApkState, t_vsync: i64, period: i64) {
        let seq = state.vsync.len() as u64 + 1;
        let mode = state.mode.clone();
        state.vsync.push(json!({
            "stream": "vsync", "seq": seq, "type": "vsync", "counter": t_vsync / period,
            "mode": mode, "t_vsync_dev_ns": t_vsync, "t_drawn_dev_ns": t_vsync + 4_000_000,
            "null_reasons": {},
        }));
    }

    fn answer(&self, line: &str) -> Value {
        let now = self.clock.now_us();
        let mut words = line.split(' ');
        let verb = words.next().unwrap_or_default().to_owned();
        let arguments: Vec<&str> = words.collect();
        let mut state = lock(&self.state);
        let device = now as i64 * 1_000 + state.offset_ns;
        let period = self.frame_period_ns;
        let next_vsync = (device.div_euclid(period) + 1) * period;
        let error =
            |code: &str| json!({ "ok": false, "cmd": verb, "error": code, "detail": "fake" });
        match verb.as_str() {
            "HELLO" => json!({
                "ok": true, "cmd": "HELLO", "protocol": PROTOCOL,
                "record_format": "capture-bench-pattern-records/1", "timecode_version": 1,
                "package": PACKAGE, "instance": state.instance, "t_now_dev_ns": device,
                "clock": "CLOCK_MONOTONIC", "frame_metrics": true, "server_error": null,
            }),
            "PING" => {
                let token = arguments.first().copied().unwrap_or_default();
                let rx = device + state.ping_one_way_us * 1_000;
                json!({
                    "ok": true, "cmd": "PING", "token": token,
                    "t_rx_dev_ns": rx, "t_tx_dev_ns": rx + state.ping_hold_ns,
                })
            }
            "INFO" => {
                let (width, height) = if state.orientation == "portrait" {
                    (720, 1280)
                } else {
                    (1280, 720)
                };
                json!({
                    "ok": true, "cmd": "INFO", "instance": state.instance,
                    "display": { "rotation": 0, "real_width": width, "real_height": height, "refresh_hz": 60 },
                    "orientation": { "requested": state.orientation, "configuration": state.orientation },
                    "mode": state.mode, "mode_seq": state.mode_seq, "mode_pending": null,
                    "view": { "width": width, "height": height, "x": 0, "y": 0 },
                    "cell": state.cell, "layout": { "cell": state.cell },
                    "flip_n": state.flip_n, "step_k": state.step_k, "step_level": state.step_level,
                })
            }
            "MODE" => {
                let Some(mode) = arguments.first().map(|mode| (*mode).to_owned()) else {
                    return error("bad_arguments");
                };
                if !matches!(
                    mode.as_str(),
                    "pattern" | "flip" | "step" | "static" | "replay"
                ) {
                    return error("bad_arguments");
                }
                if mode == "static"
                    && let Some(slot) = arguments
                        .iter()
                        .find_map(|argument| argument.strip_prefix("slot="))
                    && !state.slots.contains_key(slot)
                {
                    return error("no_such_slot");
                }
                state.mode = mode.clone();
                state.mode_seq += 1;
                if mode == "step" {
                    state.step_level = arguments
                        .iter()
                        .find_map(|argument| argument.strip_prefix("level="))
                        .and_then(|level| level.parse::<u8>().ok())
                        .unwrap_or(0);
                }
                let mode_seq = state.mode_seq;
                Self::push_event(
                    &mut state,
                    json!({
                        "type": "mode", "mode_seq": mode_seq, "mode": mode,
                        "t_rx_dev_ns": device, "counter": next_vsync / period,
                        "t_vsync_dev_ns": next_vsync, "t_drawn_dev_ns": next_vsync + 4_000_000,
                        "superseded_by": null, "null_reasons": {},
                    }),
                );
                json!({ "ok": true, "cmd": "MODE", "mode_seq": mode_seq, "mode": mode })
            }
            "STEP" => {
                if state.mode != "step" {
                    return error("wrong_mode");
                }
                state.step_k += 1;
                state.step_level = arguments
                    .iter()
                    .find_map(|argument| argument.strip_prefix("level="))
                    .and_then(|level| level.parse::<u8>().ok())
                    .unwrap_or(state.step_level ^ 1);
                let (k, level) = (state.step_k, state.step_level);
                Self::push_event(
                    &mut state,
                    json!({
                        "type": "step", "k": k, "level": level, "t_rx_dev_ns": device,
                        "t_accepted_dev_ns": device + 50_000, "counter": next_vsync / period,
                        "t_vsync_dev_ns": next_vsync, "t_drawn_dev_ns": next_vsync + 4_000_000,
                        "superseded_by": null, "null_reasons": {},
                    }),
                );
                Self::push_vsync(&mut state, next_vsync, period);
                json!({ "ok": true, "cmd": "STEP", "k": k, "level": level })
            }
            "ORIENT" => {
                let Some(value) = arguments.first().map(|value| (*value).to_owned()) else {
                    return error("bad_arguments");
                };
                let previous = state.orientation.clone();
                state.orientation = value.clone();
                Self::push_event(
                    &mut state,
                    json!({ "type": "orient", "requested": value, "previous": previous, "t_rx_dev_ns": device, "t_applied_dev_ns": device }),
                );
                json!({ "ok": true, "cmd": "ORIENT", "requested": value, "previous": previous })
            }
            "RECORDS" => {
                let stream = arguments.first().copied().unwrap_or_default().to_owned();
                let after = arguments
                    .get(1)
                    .and_then(|after| after.parse::<u64>().ok())
                    .unwrap_or(0);
                let max = arguments
                    .get(2)
                    .and_then(|max| max.parse::<usize>().ok())
                    .unwrap_or(1_000);
                let list = match stream.as_str() {
                    "vsync" => &state.vsync,
                    "event" => &state.events,
                    _ => return error("bad_arguments"),
                };
                let newest = list.len() as u64;
                if after > newest {
                    return error("cursor_ahead");
                }
                let records: Vec<Value> = list
                    .iter()
                    .skip(after as usize)
                    .take(max)
                    .cloned()
                    .collect();
                let next = after + records.len() as u64;
                json!({
                    "ok": true, "cmd": "RECORDS", "stream": stream, "after": after, "next": next,
                    "oldest": u64::from(newest > 0), "newest": newest, "lost": 0,
                    "count": records.len(), "records": records,
                })
            }
            "BYE" => json!({ "ok": true, "cmd": "BYE" }),
            _ => error("unknown_command"),
        }
    }

    fn put_slot(&self, slot: &str, bytes: &[u8]) -> Value {
        let mut state = lock(&self.state);
        let device = self.clock.now_us() as i64 * 1_000 + state.offset_ns;
        state.slots.insert(slot.to_owned(), bytes.len());
        let sha = crate::safety::sha256_hex(bytes);
        Self::push_event(
            &mut state,
            json!({ "type": "put", "slot": slot, "bytes": bytes.len(), "sha256": sha, "t_done_dev_ns": device }),
        );
        json!({ "ok": true, "cmd": "PUT", "slot": slot, "bytes": bytes.len(), "sha256": sha })
    }
}

/// A connection to one fake APK process: when the process restarts, the connection is gone
/// (as the real TCP connection is), and every request fails with `apk_io`.
struct FakeApkLink {
    apk: Arc<FakeApk>,
    process: String,
}

impl FakeApkLink {
    fn alive(&self) -> Result<(), ApkError> {
        if lock(&self.apk.state).instance == self.process {
            Ok(())
        } else {
            Err(ApkError::new(
                "apk_io",
                "the APK process of this connection is gone",
            ))
        }
    }
}

impl ApkLink for FakeApkLink {
    fn request(&mut self, line: &str) -> Result<Value, ApkError> {
        self.alive()?;
        Ok(self.apk.answer(line))
    }

    fn put(&mut self, slot: &str, bytes: &[u8]) -> Result<Value, ApkError> {
        self.alive()?;
        Ok(self.apk.put_slot(slot, bytes))
    }

    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// The fake connector: every instance named in `apks` has its fake APK.
pub struct FakeApks {
    pub apks: BTreeMap<String, Arc<FakeApk>>,
}

impl ApkConnector for FakeApks {
    fn connect(&self, instance: &str) -> Result<Box<dyn ApkLink>, ApkError> {
        self.apks
            .get(instance)
            .map(|apk| {
                Box::new(FakeApkLink {
                    apk: Arc::clone(apk),
                    process: lock(&apk.state).instance.clone(),
                }) as Box<dyn ApkLink>
            })
            .ok_or_else(|| ApkError::new("apk_unknown_instance", instance.to_owned()))
    }
}
