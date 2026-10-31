// SPDX-License-Identifier: AGPL-3.0-only

//! The plan executor (§3.2 item 3, §3.11): the start check, the cells and their segments, the
//! cell end (§3.11.2 A), the whole-run stop (§3.11.2 B), the results and exit codes (§3.11.3),
//! the error classes (§3.11.4) and the device-side close confirmation (§3.5.1).

use crate::apk::{ApkClient, ApkConnector, ApkError, ApkStreams, Calibration};
use crate::backend::{
    BackendFactory, BackendFailure, CheckFailure, DeviceSideCheck, FailureClass, Floor, GoneResult,
    IdentityMethod, IdentityTuple, MaterialFactory, OpenFailure, OpenFailureHandling, OpenOptions,
    PreviewBackend, StreamFacts, open_failure_handling,
};
use crate::clock::{Clock, Micros, Participant, blocking, ms, pause_until, spawn_participant};
use crate::hub::{
    BudgetConfig, DispatchOp, DispatchOutcome, FormalResult, Hub, MemorySink, SampleOp, aux_call,
    formal_capture, formal_tap, run_blocking,
};
use crate::journal::{Journal, JournalRecords, TeeRecords};
use crate::locks::LockOwner;
use crate::metrics::{CellProbes, MetricsSample, MetricsSource};
use crate::model::{
    BackendKind, CellResult, CloseOutcome, Disposition, Entry, GateKind, GlobalGate, PathKind,
    RunResult, StopCause,
};
use crate::plan::{CellPlan, Plan, SegmentMode, SegmentPlan};
use crate::safety::{BenchLock, Roots, StartChecks};
use crate::supervisor::{
    CloseInfo, Closer, Emergency, ExitHook, FixedLimits, Terminal, control_tick, heartbeat_tick,
    spawn_ticker,
};
use crate::timecode::BandGeometry;
use crate::viewer::{ViewerContext, ViewerHandle, ViewerLauncher};
use crate::views::ViewMessage;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything a run needs from outside: real in production, fake in the D1 rehearsal.
pub struct Env {
    pub clock: Arc<dyn Clock>,
    pub backends: Arc<dyn BackendFactory>,
    pub device_check: Arc<dyn DeviceSideCheck>,
    pub materials: Arc<dyn MaterialFactory>,
    pub metrics: Box<dyn MetricsSource>,
    pub viewers: Arc<dyn ViewerLauncher>,
    pub start_checks: Box<dyn StartChecks>,
    pub exit: Arc<dyn ExitHook>,
    pub limits: FixedLimits,
    pub roots: Roots,
    /// Extra header facts (the executable, the timing points, the scrcpy server pin).
    pub header_facts: Value,
    /// #402 C2b: the test-pattern APK's connector (C3); `None` without one.
    pub apk: Option<Arc<dyn ApkConnector>>,
    /// #402 C2b: the per-cell readings (the stdio capture files, the VMMHeadless handles).
    pub probes: Arc<dyn CellProbes>,
}

/// What a run ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub result: RunResult,
    pub results: Vec<RunResult>,
    pub exit_code: i32,
}

impl RunOutcome {
    fn refused() -> Self {
        Self {
            result: RunResult::RefusedStart,
            results: vec![RunResult::RefusedStart],
            exit_code: RunResult::RefusedStart.exit_code(),
        }
    }
}

/// The state the runner, the supervisor's emergency path and the heartbeat share.
pub struct RunShared {
    pub plan: Plan,
    pub clock: Arc<dyn Clock>,
    pub journal: Arc<Journal>,
    pub hub: Arc<Hub>,
    pub records: Arc<MemorySink>,
    pub closer: Arc<Closer>,
    pub terminal: Arc<Terminal>,
    pub limits: FixedLimits,
    backends: Arc<dyn BackendFactory>,
    device_check: Arc<dyn DeviceSideCheck>,
    materials: Arc<dyn MaterialFactory>,
    viewers: Arc<dyn ViewerLauncher>,
    status: Mutex<Value>,
    cells: Mutex<BTreeMap<String, BTreeMap<String, CellResult>>>,
    closes: Mutex<Vec<Value>>,
    unconfirmed: Mutex<Vec<String>>,
    disabled: Mutex<BTreeMap<(usize, PathKind), String>>,
    scrcpy_not_ready: Mutex<BTreeMap<usize, String>>,
    identity_methods: Mutex<BTreeMap<usize, IdentityMethod>>,
    rebooted: Mutex<BTreeSet<usize>>,
    stdio_unrestored: AtomicBool,
    safety_failed: AtomicBool,
    summary_written: AtomicBool,
    /// #402 C2b: one lock owner per thread (concurrent segments).
    owners: Mutex<std::collections::HashMap<std::thread::ThreadId, Arc<LockOwner>>>,
    started_us: Micros,
    started_unix_ms: u64,
    next_subscriber: AtomicU64,
    working_set: AtomicU64,
    /// `t_stop_new` and the sequence number of the whole-run stop gate, once it is set.
    run_stop: Mutex<Option<(Micros, u64)>>,
    /// The backends that failed fatally in the current cell: their other paths in the cell
    /// fail too (§3.11.4 "共用同一后端的路径").
    cell_fatal: Mutex<BTreeMap<(usize, BackendKind), String>>,
    /// The opportunity files' `seq` (unique in every file).
    next_record: AtomicU64,
    /// #402 C2b: the APK connector and each instance's APK state.
    apk: Option<Arc<dyn ApkConnector>>,
    apk_states: Vec<Mutex<ApkInstance>>,
    /// #402 C2b: the band geometry INFO reported, per instance.
    geometries: Mutex<BTreeMap<usize, BandGeometry>>,
    /// #402 C2b: what the bench knows of each instance's display (the taps are mapped on it).
    displays: Mutex<BTreeMap<usize, DisplayFacts>>,
    /// #402 C2b: the orientation after a C10 rotation, per instance.
    orientations: Mutex<BTreeMap<usize, crate::plan::Orientation>>,
    probes: Arc<dyn CellProbes>,
    /// #402 C2b: the lock-order and invariant violations already written as events.
    violations_written: Mutex<(usize, usize)>,
    ocr_header_written: AtomicBool,
}

fn key(plan: &Plan, instance: usize, path: PathKind) -> String {
    format!("{}/{}", plan.instances[instance].name, path.as_str())
}

/// Runs `plan_text` in `run_dir`. The process exit is the caller's (production) or the test's.
pub fn run(plan_text: &str, run_dir: &Path, env: Env) -> RunOutcome {
    let Env {
        clock,
        backends,
        device_check,
        materials,
        mut metrics,
        viewers,
        mut start_checks,
        exit,
        limits,
        roots,
        header_facts,
        apk,
        probes,
    } = env;

    // §3.3.4: the run directory is under the fixed runs root; otherwise nothing is written.
    if !roots.contains_run_dir(run_dir) {
        return RunOutcome::refused();
    }
    let journal = match Journal::open(run_dir, Arc::clone(&clock)) {
        Ok(journal) => journal,
        Err(_) => return RunOutcome::refused(),
    };
    let refuse = |reason: String| {
        journal.event("refused_start", json!({ "reason": reason }));
        let _ = journal.write_json(
            "summary.json",
            &json!({
                "result": "refused_start",
                "exit_code": RunResult::RefusedStart.exit_code(),
                "reason": reason,
            }),
        );
        journal.flush();
        RunOutcome::refused()
    };
    // §3.3.4: the single bench lock, held for the process's life.
    let bench_lock = match BenchLock::acquire(&roots.lock_path) {
        Ok(bench_lock) => bench_lock,
        Err(reason) => return refuse(reason),
    };
    let _ = std::fs::write(journal.path("plan.toml"), plan_text);
    let plan = match Plan::parse(plan_text).and_then(|plan| plan.validate().map(|()| plan)) {
        Ok(plan) => plan,
        Err(reason) => return refuse(reason),
    };
    if clock.unix_ms() >= plan.until_unix_ms {
        return refuse("until_unix_ms has passed".to_owned());
    }
    if let Err(reason) = start_checks.check(&plan, run_dir) {
        return refuse(reason);
    }
    let names: Vec<String> = plan
        .instances
        .iter()
        .map(|instance| instance.name.clone())
        .collect();
    let plan_sha256 = crate::safety::sha256_hex(plan_text.as_bytes());
    let identity = crate::safety::process_identity(run_dir, &plan.run_id, &plan_sha256);
    if let Err(reason) = bench_lock.write_identity(&identity) {
        return refuse(reason);
    }
    let header = json!({
        // The analysis schema's header fields (`rd-1031/exp402/analysis/schema.md` §2).
        "run_id": plan.run_id,
        "window": plan.window,
        "started_unix_ms": clock.unix_ms(),
        "backend": header_facts.get("backend").cloned().unwrap_or_else(|| json!("fake")),
        "instances": plan
            .instances
            .iter()
            .map(|instance| json!({ "id": instance.name, "orientation": orientation_code(instance.orientation) }))
            .collect::<Vec<_>>(),
        "target_fps": 5,
        "window_derivation": { "premise_verified": false },
        "identity": identity,
        "plan_sha256": plan_sha256,
        "limits": limits,
        "lockcheck": cfg!(feature = "bench-lockcheck"),
        "n_cache": {
            "state": "not_ready",
            "reason": "no vendor SDK contract for buffer_size; boundary probing not done (146 b)",
        },
        "start_checks": start_checks.facts(),
        "facts": header_facts,
    });
    if let Err(error) = journal.write_json("header.json", &header) {
        return refuse(format!("header.json not written: {error}"));
    }

    let records = Arc::new(MemorySink::default());
    let sink = Arc::new(TeeRecords {
        first: Arc::new(JournalRecords::new(Arc::clone(&journal), names.clone())),
        second: records.clone(),
    });
    let budget = BudgetConfig {
        max_samples_per_second: plan.budget.max_samples_per_second,
        max_bytes_per_second: plan.budget.max_bytes_per_second,
        max_inflight_global: plan.budget.max_inflight_global,
        max_buffer_bytes: plan.budget.max_buffer_bytes,
    };
    let epoch = clock.unix_ms();
    let hub = Arc::new(Hub::new(Arc::clone(&clock), &names, epoch, budget, sink));
    let terminal = Terminal::new();
    let closer = Closer::new(
        Arc::clone(&clock),
        Arc::clone(&journal),
        Arc::clone(&terminal),
        limits,
        Arc::clone(&exit),
    );
    let shared = Arc::new(RunShared {
        plan,
        clock: Arc::clone(&clock),
        journal: Arc::clone(&journal),
        hub,
        records,
        closer: Arc::clone(&closer),
        terminal: Arc::clone(&terminal),
        limits,
        backends,
        device_check,
        materials,
        viewers,
        status: Mutex::new(json!({})),
        cells: Mutex::new(BTreeMap::new()),
        closes: Mutex::new(Vec::new()),
        unconfirmed: Mutex::new(Vec::new()),
        disabled: Mutex::new(BTreeMap::new()),
        scrcpy_not_ready: Mutex::new(BTreeMap::new()),
        identity_methods: Mutex::new(BTreeMap::new()),
        rebooted: Mutex::new(BTreeSet::new()),
        stdio_unrestored: AtomicBool::new(false),
        safety_failed: AtomicBool::new(false),
        summary_written: AtomicBool::new(false),
        owners: Mutex::new(std::collections::HashMap::new()),
        started_us: clock.now_us(),
        started_unix_ms: clock.unix_ms(),
        next_subscriber: AtomicU64::new(1),
        working_set: AtomicU64::new(0),
        run_stop: Mutex::new(None),
        cell_fatal: Mutex::new(BTreeMap::new()),
        next_record: AtomicU64::new(1),
        apk_states: names
            .iter()
            .map(|_| Mutex::new(ApkInstance::new()))
            .collect(),
        apk,
        geometries: Mutex::new(BTreeMap::new()),
        displays: Mutex::new(BTreeMap::new()),
        orientations: Mutex::new(BTreeMap::new()),
        probes,
        violations_written: Mutex::new((0, 0)),
        ocr_header_written: AtomicBool::new(false),
    });
    closer.set_emergency(shared.clone());
    for (index, instance) in shared.plan.instances.iter().enumerate() {
        if let Some(method) = instance.identity_method {
            lock(&shared.identity_methods).insert(index, method);
        }
    }

    // The background threads: supervisor, heartbeat, control files, metrics.
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads: Vec<Participant<()>> = Vec::new();
    match closer.spawn_supervisor() {
        Ok(thread) => threads.push(thread),
        Err(error) => return refuse(format!("the supervisor did not start: {error}")),
    }
    {
        let heartbeat = Arc::clone(&shared);
        match spawn_ticker(
            &clock,
            "heartbeat",
            limits.heartbeat_period_us,
            Arc::clone(&stop),
            move || {
                let dynamic = lock(&heartbeat.status).clone();
                heartbeat_tick(
                    &heartbeat.journal,
                    &heartbeat.closer,
                    &heartbeat.terminal,
                    dynamic,
                );
            },
        ) {
            Ok(thread) => threads.push(thread),
            Err(error) => return refuse(format!("the heartbeat did not start: {error}")),
        }
    }
    {
        let control = Arc::clone(&shared);
        let mut seen = [false; 2];
        match spawn_ticker(
            &clock,
            "control",
            limits.control_period_us,
            Arc::clone(&stop),
            move || {
                control_tick(&control.journal, &control.terminal, &mut seen);
            },
        ) {
            Ok(thread) => threads.push(thread),
            Err(error) => return refuse(format!("the control watcher did not start: {error}")),
        }
    }
    {
        let watcher = Arc::clone(&shared);
        match spawn_ticker(
            &clock,
            "metrics",
            limits.metrics_period_us,
            Arc::clone(&stop),
            move || {
                let sample = metrics.sample();
                watcher.on_metrics(&sample);
            },
        ) {
            Ok(thread) => threads.push(thread),
            Err(error) => return refuse(format!("the metrics sampler did not start: {error}")),
        }
    }
    journal.event("run_started", json!({ "run_id": shared.plan.run_id }));

    let outcome = shared.execute();
    shared.apk_disconnect_all();

    stop.store(true, Ordering::Release);
    closer.stop_supervisor();
    for thread in threads {
        let _ = thread.join();
    }
    journal.flush();
    drop(bench_lock);
    outcome
}

/// The cell-local state the activities share with the controller.
struct SegmentShared {
    stop: AtomicBool,
    fatal: Mutex<Vec<(BackendKind, BackendFailure)>>,
    instance_fatal: Mutex<Option<String>>,
    disabled_backends: Mutex<BTreeSet<BackendKind>>,
    opportunities: AtomicU64,
    floor_checks: AtomicU64,
    dispatches: AtomicU64,
    busy_streak: AtomicU32,
    /// #402 C2b: when the segment stopped new work (0 until then), and the calls in flight at
    /// that time with their returns (the `drain` event).
    stop_at: AtomicU64,
    drained: Mutex<Vec<Value>>,
    /// #402 C2b: delivered frames and B_model's material (the `artifact_delta` line).
    delivered: AtomicU64,
    material_bytes: AtomicU64,
    material_events: AtomicU64,
}

impl SegmentShared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            stop_at: AtomicU64::new(0),
            drained: Mutex::new(Vec::new()),
            delivered: AtomicU64::new(0),
            material_bytes: AtomicU64::new(0),
            material_events: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            fatal: Mutex::new(Vec::new()),
            instance_fatal: Mutex::new(None),
            disabled_backends: Mutex::new(BTreeSet::new()),
            opportunities: AtomicU64::new(0),
            floor_checks: AtomicU64::new(0),
            dispatches: AtomicU64::new(0),
            busy_streak: AtomicU32::new(0),
        })
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn backend_disabled(&self, kind: BackendKind) -> bool {
        lock(&self.disabled_backends).contains(&kind)
    }

    fn stop_at(&self) -> Option<Micros> {
        let at = self.stop_at.load(Ordering::Acquire);
        (at > 0).then_some(at)
    }

    /// A call that was in flight when the segment stopped new work (§3.5 四个时刻).
    fn note_drained(
        &self,
        k: u64,
        start: Option<Micros>,
        caller_return: Option<Micros>,
        native_return: Option<Micros>,
        unconfirmed: bool,
    ) {
        if let Some(stop_at) = self.stop_at()
            && start.is_some_and(|start| start < stop_at)
            && caller_return.is_none_or(|end| end >= stop_at)
        {
            lock(&self.drained).push(json!({
                "k": k,
                "t_caller_return_us": caller_return,
                "t_native_return_us": native_return,
                "unconfirmed": unconfirmed,
            }));
        }
    }
}

/// How a segment ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SegmentEnd {
    /// Its time or quota ran out.
    Finished,
    /// The cell limit cut it.
    CellLimit,
    /// The whole run stops.
    Whole,
    /// A fatal error stopped its path.
    Fatal(String),
    /// Not run (the path is disabled or not ready).
    Skipped(CellResult),
}

/// Which analysis-schema record a dispatch writes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordShape {
    Dispatch {
        preview_state: String,
        record_path: String,
    },
    Admission {
        j: u32,
    },
    Poll {
        after_sequence: Option<u64>,
    },
}

/// A small deterministic generator (the plan's seed).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next() % bound }
    }
}

fn hash_text(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `L` or `P` (analysis schema §2).
fn orientation_code(orientation: crate::plan::Orientation) -> &'static str {
    match orientation {
        crate::plan::Orientation::Landscape => "L",
        crate::plan::Orientation::Portrait => "P",
    }
}

/// The segment fields every opportunity record carries (analysis schema §3.1).
#[derive(Debug, Clone)]
struct SegmentInfo {
    cell: String,
    name: String,
    label: String,
    round: u32,
    instance: String,
    orientation: &'static str,
    target_fps: f64,
}

impl SegmentInfo {
    fn new(
        plan: &Plan,
        cell: &CellPlan,
        segment: &SegmentPlan,
        instance: usize,
        round: u32,
        index: usize,
    ) -> Self {
        let mode = match segment.mode {
            SegmentMode::Preview => "preview",
            SegmentMode::TaskFloor => "floor",
            SegmentMode::PollSequence => "static_poll",
            SegmentMode::ColdStart => "cold_start",
            SegmentMode::Calibrate => "calibrate",
            SegmentMode::StepTrials => "step_trials",
            SegmentMode::Rotate => "rotate",
            SegmentMode::Reconnect => "reconnect",
            SegmentMode::StaticImages => "static_images",
        };
        let name = segment
            .name
            .clone()
            .unwrap_or_else(|| format!("{mode}-{index}"));
        Self {
            cell: cell.id.clone(),
            label: format!("{}-r{round}-{name}", cell.id),
            name,
            round,
            instance: plan.instances[instance].name.clone(),
            orientation: orientation_code(plan.instances[instance].orientation),
            target_fps: segment.fps,
        }
    }

    fn record(&self, kind: &str, path: &str, seq: u64, extra: Value) -> Value {
        let mut line = json!({
            "kind": kind,
            "cell": self.cell,
            "path": path,
            "orientation": self.orientation,
            "instance": self.instance,
            "seq": seq,
            "round": self.round,
            "segment": self.name,
            "target_fps": self.target_fps,
        });
        if let (Value::Object(map), Value::Object(extra)) = (&mut line, extra) {
            map.extend(extra);
        }
        line
    }
}

/// The timing block of a sample (analysis schema §3.2).
fn timing_of(frame: Option<&crate::backend::SampledFrame>, extra: Value) -> Value {
    let mut timing = json!({});
    if let Some(frame) = frame {
        let native: Vec<Value> = frame
            .native
            .iter()
            .filter(|span| matches!(span.kind, "probe" | "pixels"))
            .map(|span| json!({ "call": span.kind, "start_us": span.start_us, "end_us": span.end_us, "code": span.code }))
            .collect();
        let stdio: Vec<Value> = frame
            .native
            .iter()
            .filter(|span| span.kind == "stdio_wait")
            .map(|span| {
                json!(
                    span.end_us
                        .zip(span.start_us)
                        .map(|(end, start)| end.saturating_sub(start))
                )
            })
            .collect();
        let convert = frame
            .native
            .iter()
            .find(|span| span.kind == "flip")
            .and_then(|span| {
                span.end_us
                    .zip(span.start_us)
                    .map(|(end, start)| end.saturating_sub(start))
            });
        timing = json!({
            "stdio_wait_us": stdio,
            "native": native,
            "t_built_us": frame.built_us,
            "t_built_unix_ms": frame.built_unix_ms,
            "t_recv_us": frame.received_us,
            "stream_seq": frame.stream_sequence,
            "device_pts_us": frame.device_pts_us,
            "decode_us": null,
            "convert_us": convert,
        });
    }
    if let (Value::Object(map), Value::Object(extra)) = (&mut timing, extra) {
        map.extend(extra);
    }
    timing
}

impl RunShared {
    fn next_seq(&self) -> u64 {
        self.next_record.fetch_add(1, Ordering::Relaxed)
    }

    fn now(&self) -> Micros {
        self.clock.now_us()
    }

    fn instance_name(&self, instance: usize) -> &str {
        &self.plan.instances[instance].name
    }

    fn set_status(&self, cell: &str, segment: Value) {
        let inflight: u32 = (0..self.hub.instance_count())
            .map(|instance| {
                self.hub
                    .inflight(&LockOwner::new("heartbeat-read"), instance)
            })
            .sum();
        *lock(&self.status) = json!({
            "cell": cell,
            "segment": segment,
            "inflight": inflight,
            "working_set_bytes": self.working_set.load(Ordering::Relaxed),
        });
    }

    fn on_metrics(&self, sample: &MetricsSample) {
        let now = self.now();
        let status = lock(&self.status).clone();
        let cell = status
            .get("cell")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_owned();
        let segment = &status["segment"];
        self.journal.append(
            &format!("host-{cell}.jsonl"),
            &json!({
                "type": "sample",
                "t_us": now,
                "unix_ms": self.clock.unix_ms(),
                "cell": cell,
                "segment": segment.get("path").cloned().filter(|path| !path.is_null()).unwrap_or_else(|| json!("off")),
                "segment_name": segment.get("segment"),
                "orientation": segment.get("orientation"),
                "round": segment.get("round"),
                "host_cpu_bp": sample.host_cpu_bp,
                "gpu_pct": sample.gpu_bp.map(|bp| f64::from(bp) / 100.0),
                "bench_ws_bytes": sample.own_working_set_bytes,
                "host_available_bytes": sample.host_available_bytes,
                "disk_free_bytes": sample.disk_free_bytes,
                "processes": sample.processes,
                "unavailable": sample.unavailable,
            }),
        );
        if let Some(bytes) = sample.own_working_set_bytes {
            self.working_set.store(bytes, Ordering::Relaxed);
            if bytes > self.limits.working_set_cap_bytes {
                self.terminal
                    .request(now, StopCause::WorkingSetCap { bytes });
            }
        }
        if let Some(bytes) = sample.disk_free_bytes
            && bytes < self.limits.disk_min_bytes
        {
            self.terminal.request(now, StopCause::DiskLow { bytes });
        }
        self.check_time();
    }

    fn check_time(&self) {
        let now = self.now();
        let elapsed_ms = now.saturating_sub(self.started_us) / 1_000;
        if self.clock.unix_ms() >= self.plan.until_unix_ms {
            self.terminal.request(now, StopCause::Deadline);
        } else if elapsed_ms >= self.plan.total_duration_ms {
            self.terminal.request(now, StopCause::TotalDuration);
        }
    }

    fn record_cell(&self, cell: &str, instance: usize, path: PathKind, result: CellResult) {
        let key = key(&self.plan, instance, path);
        let mut cells = lock(&self.cells);
        let entry = cells.entry(cell.to_owned()).or_default();
        // A failure is kept over anything; an incomplete over a completion.
        let keep_old = match (entry.get(&key), &result) {
            (Some(CellResult::CellFailed(_)), _) => true,
            (Some(CellResult::CellIncomplete(_)), CellResult::CellCompleted) => true,
            _ => false,
        };
        if !keep_old {
            entry.insert(key, result);
        }
    }

    fn unconfirmed(&self, what: String) {
        self.journal
            .event("resource_unconfirmed", json!({ "what": what }));
        lock(&self.unconfirmed).push(what.clone());
        self.terminal
            .request(self.now(), StopCause::Unconfirmed(what));
    }

    fn whole_stop_requested(&self) -> bool {
        let requested = self.terminal.requested() || self.terminal.finished_by_supervisor();
        if requested {
            self.stop_new();
        }
        requested
    }

    /// §3.11.2 B step 1: the whole-run stop gate, set once, before the drain and the closes.
    fn stop_new(&self) -> (Micros, u64) {
        let mut run_stop = lock(&self.run_stop);
        if let Some(set) = *run_stop {
            return set;
        }
        let at = self.now();
        let seq = self
            .hub
            .set_global(&self.owner(), GlobalGate::RunStop, true);
        self.hub.views.broadcast(None, ViewMessage::Ended);
        self.journal.event(
            "run_stop",
            json!({ "t_stop_new_us": at, "gate_seq": seq, "cause": self.terminal.first() }),
        );
        *run_stop = Some((at, seq));
        (at, seq)
    }

    /// §3.11.2: the cells in order, A between them, B at the end.
    fn execute(self: &Arc<Self>) -> RunOutcome {
        let cells = self.plan.cells.clone();
        let mut ran = Vec::new();
        for (index, cell) in cells.iter().enumerate() {
            self.check_time();
            if self.whole_stop_requested() {
                break;
            }
            // Transition check 3 (§3.11.2 A 6.3): the next cell must fit before the deadline.
            let now_unix = self.clock.unix_ms();
            let elapsed_ms = self.now().saturating_sub(self.started_us) / 1_000;
            if now_unix.saturating_add(cell.limit_ms) > self.plan.until_unix_ms
                || elapsed_ms.saturating_add(cell.limit_ms) > self.plan.total_duration_ms
            {
                self.terminal.request(self.now(), StopCause::Deadline);
                break;
            }
            if index > 0 {
                for instance in 0..self.hub.instance_count() {
                    self.hub.bump_generation(&self.owner(), instance);
                }
            }
            self.run_cell(cell);
            ran.push(cell.id.clone());
            if self.terminal.finished_by_supervisor() {
                return self.finished_by_supervisor_outcome();
            }
        }
        self.settle(&ran)
    }

    fn finished_by_supervisor_outcome(&self) -> RunOutcome {
        RunOutcome {
            result: RunResult::Unconfirmed,
            results: vec![RunResult::Unconfirmed],
            exit_code: RunResult::Unconfirmed.exit_code(),
        }
    }

    /// The paths a cell plans, per instance.
    fn planned_paths(cell: &CellPlan, plan: &Plan) -> BTreeSet<(usize, PathKind)> {
        cell.segments
            .iter()
            .filter_map(|segment| {
                let instance = plan.instance_index(&segment.instance)?;
                segment.path.map(|path| (instance, path))
            })
            .collect()
    }

    fn run_cell(self: &Arc<Self>, cell: &CellPlan) {
        let started = self.now();
        let deadline = started.saturating_add(ms(cell.limit_ms));
        self.journal.event(
            "cell_start",
            json!({ "cell": cell.id, "limit_ms": cell.limit_ms }),
        );
        self.journal.append(
            "progress.jsonl",
            &json!({ "t_us": started, "event": "cell_start", "cell": cell.id }),
        );
        self.set_status(&cell.id, Value::Null);
        lock(&self.cell_fatal).clear();
        self.cell_probe(&cell.id, "start");
        let planned = Self::planned_paths(cell, &self.plan);
        let mut ends: BTreeMap<(usize, PathKind), Vec<SegmentEnd>> = BTreeMap::new();
        let mut rng = Rng::new(self.plan.seed ^ hash_text(&cell.id));
        'rounds: for round in 1..=cell.rounds {
            let mut order: Vec<usize> = (0..cell.segments.len()).collect();
            if cell.shuffle {
                for position in (1..order.len()).rev() {
                    let pick = rng.below(position as u64 + 1) as usize;
                    order.swap(position, pick);
                }
            }
            if cell.concurrent {
                // #402 C2b (C9): the round's segments run at the same time, one thread each.
                if self.whole_stop_requested() {
                    break 'rounds;
                }
                let mut running = Vec::new();
                for segment_index in order {
                    let segment = cell.segments[segment_index].clone();
                    let Some(instance) = self.plan.instance_index(&segment.instance) else {
                        continue;
                    };
                    let path = segment.path;
                    if self.now() >= deadline {
                        if let Some(path) = path {
                            ends.entry((instance, path))
                                .or_default()
                                .push(SegmentEnd::CellLimit);
                        }
                        continue;
                    }
                    let mut info = SegmentInfo::new(
                        &self.plan,
                        cell,
                        &segment,
                        instance,
                        round,
                        segment_index,
                    );
                    info.orientation = self.orientation_now(instance);
                    let run = Arc::clone(self);
                    let cell_plan = cell.clone();
                    let mut segment_rng = Rng::new(rng.next());
                    let name = format!("segment-{}", info.label);
                    let spawned = spawn_participant(&self.clock, &name, move || {
                        run.run_segment(
                            &cell_plan,
                            &segment,
                            instance,
                            &info,
                            deadline,
                            &mut segment_rng,
                            true,
                        )
                    });
                    match spawned {
                        Ok(thread) => running.push((instance, path, thread)),
                        Err(error) => {
                            self.journal.event(
                                "segment_spawn_failed",
                                json!({ "cell": cell.id, "detail": error.to_string() }),
                            );
                            if let Some(path) = path {
                                ends.entry((instance, path))
                                    .or_default()
                                    .push(SegmentEnd::Fatal("segment_spawn_failed".to_owned()));
                            }
                        }
                    }
                }
                let mut whole = false;
                for (instance, path, thread) in running {
                    let end = thread
                        .join()
                        .unwrap_or_else(|_| SegmentEnd::Fatal("segment_panicked".to_owned()));
                    whole |= end == SegmentEnd::Whole;
                    if let Some(path) = path {
                        ends.entry((instance, path)).or_default().push(end);
                    }
                }
                if whole {
                    break 'rounds;
                }
                continue;
            }
            for segment_index in order {
                let segment = &cell.segments[segment_index];
                let Some(instance) = self.plan.instance_index(&segment.instance) else {
                    continue;
                };
                if self.whole_stop_requested() {
                    break 'rounds;
                }
                if self.now() >= deadline {
                    if let Some(path) = segment.path {
                        ends.entry((instance, path))
                            .or_default()
                            .push(SegmentEnd::CellLimit);
                    }
                    continue;
                }
                let mut info =
                    SegmentInfo::new(&self.plan, cell, segment, instance, round, segment_index);
                info.orientation = self.orientation_now(instance);
                let end =
                    self.run_segment(cell, segment, instance, &info, deadline, &mut rng, false);
                if let Some(path) = segment.path {
                    ends.entry((instance, path)).or_default().push(end.clone());
                }
                if end == SegmentEnd::Whole {
                    break 'rounds;
                }
            }
        }
        // §3.11.2 A: the cell-end gate, the drain (the segments joined their activities and
        // closed their backends), the cell summary, then the gate is lifted.
        let gate_seq = self
            .hub
            .set_global(&self.owner(), GlobalGate::CellEnd, true);
        self.hub.views.broadcast(None, ViewMessage::CellEnded);
        let stop_reason = self.terminal.first().map(|cause| cause.label().to_owned());
        for (instance, path) in &planned {
            let result = self.cell_result(
                *instance,
                *path,
                ends.get(&(*instance, *path)),
                stop_reason.as_deref(),
            );
            self.record_cell(&cell.id, *instance, *path, result);
        }
        let results = lock(&self.cells).get(&cell.id).cloned().unwrap_or_default();
        let listed: Vec<Value> = results
            .iter()
            .map(|(key, result)| self.result_entry(&cell.id, key, result))
            .collect();
        let summary = json!({
            "cell": cell.id,
            "results": listed,
            "closes": *lock(&self.closes),
            "gate_seq": gate_seq,
            "started_us": started,
            "ended_us": self.now(),
            "limit_ms": cell.limit_ms,
        });
        let _ = self
            .journal
            .write_json(&format!("summary-{}.json", cell.id), &summary);
        self.journal.append(
            "progress.jsonl",
            &json!({ "t_us": self.now(), "event": "cell_end", "cell": cell.id, "results": results }),
        );
        self.hub
            .set_global(&self.owner(), GlobalGate::CellEnd, false);
        self.cell_probe(&cell.id, "end");
        self.write_hub_notes();
    }

    /// One `{path, orientation, result, reason, ...}` entry of a summary (analysis schema §10).
    fn result_entry(&self, cell: &str, key: &str, result: &CellResult) -> Value {
        let (instance, path) = key.split_once('/').unwrap_or((key, ""));
        let orientation = self
            .plan
            .instance_index(instance)
            .map(|index| orientation_code(self.plan.instances[index].orientation));
        let reason = match result {
            CellResult::CellCompleted => None,
            CellResult::CellIncomplete(reason)
            | CellResult::CellFailed(reason)
            | CellResult::Untested(reason)
            | CellResult::ConditionalSkip(reason) => Some(reason.clone()),
        };
        let status = match result {
            CellResult::Untested(_) => "not_measured",
            other => other.label(),
        };
        json!({
            "cell": cell,
            "instance": instance,
            "path": path,
            "orientation": orientation,
            "result": result.label(),
            "status": status,
            "reason": reason,
            "first_error": match result { CellResult::CellFailed(code) => Some(code.clone()), _ => None },
        })
    }

    fn cell_result(
        &self,
        instance: usize,
        path: PathKind,
        ends: Option<&Vec<SegmentEnd>>,
        stop_reason: Option<&str>,
    ) -> CellResult {
        let Some(ends) = ends.filter(|ends| !ends.is_empty()) else {
            return CellResult::Untested(
                stop_reason
                    .map(|reason| format!("not reached: {reason}"))
                    .unwrap_or_else(|| "not reached".to_owned()),
            );
        };
        if let Some(SegmentEnd::Fatal(code)) =
            ends.iter().find(|end| matches!(end, SegmentEnd::Fatal(_)))
        {
            return CellResult::CellFailed(code.clone());
        }
        if let Some(SegmentEnd::Skipped(result)) = ends
            .iter()
            .find(|end| matches!(end, SegmentEnd::Skipped(_)))
        {
            return result.clone();
        }
        if ends.contains(&SegmentEnd::Whole) {
            return CellResult::CellIncomplete(stop_reason.unwrap_or("stopped").to_owned());
        }
        if ends.contains(&SegmentEnd::CellLimit) {
            return CellResult::CellIncomplete(format!(
                "quota_short: {} of {} segments of {} done",
                ends.iter()
                    .filter(|end| **end == SegmentEnd::Finished)
                    .count(),
                ends.len(),
                key(&self.plan, instance, path)
            ));
        }
        CellResult::CellCompleted
    }

    /// The identity method of `instance` (§3.5.1 C0): the plan's, or the first candidate whose
    /// negative control succeeds. `None` means S is not ready on this E (nothing was started).
    fn identity_method(&self, instance: usize) -> Option<IdentityMethod> {
        if let Some(method) = lock(&self.identity_methods).get(&instance) {
            return Some(*method);
        }
        let name = self.instance_name(instance).to_owned();
        let mut tried = Vec::new();
        for method in IdentityMethod::CANDIDATES {
            let checked = aux_call(
                &self.hub,
                &self.owner(),
                instance,
                Entry::DeviceSideCheck,
                false,
                || self.device_check.negative_control(&name, method),
            );
            match checked {
                Ok(Ok(())) => {
                    lock(&self.identity_methods).insert(instance, method);
                    self.journal.event(
                        "identity_method_selected",
                        json!({ "instance": name, "method": method, "tried": tried }),
                    );
                    return Some(method);
                }
                Ok(Err(failure)) => tried.push(json!({ "method": method, "failure": failure })),
                Err(gate) => tried.push(json!({ "method": method, "refused_by": gate })),
            }
        }
        self.journal.event(
            "scrcpy_not_ready",
            json!({ "instance": name, "reason": "device_side_close_unprovable", "tried": tried }),
        );
        lock(&self.scrcpy_not_ready).insert(instance, "device_side_close_unprovable".to_owned());
        None
    }

    /// Opens one backend for a segment, with table B's reopen rules for S and the identity
    /// positive check. `Err` carries the segment end.
    fn open_backend(
        &self,
        instance: usize,
        kind: BackendKind,
        options: &OpenOptions,
        reopens: &mut u32,
        cold_start: bool,
    ) -> Result<(Box<dyn PreviewBackend>, Option<IdentityTuple>), SegmentEnd> {
        let name = self.instance_name(instance).to_owned();
        let method = if kind == BackendKind::Scrcpy {
            match self.identity_method(instance) {
                Some(method) => Some(method),
                None => {
                    return Err(SegmentEnd::Skipped(CellResult::ConditionalSkip(
                        "scrcpy not ready: device_side_close_unprovable".to_owned(),
                    )));
                }
            }
        } else {
            None
        };
        loop {
            if self.whole_stop_requested() {
                return Err(SegmentEnd::Whole);
            }
            // §3.5.1 正例 1: before each S open the same method finds no bench server.
            let mut negative_empty = None;
            if let Some(method) = method {
                let checked = aux_call(
                    &self.hub,
                    &self.owner(),
                    instance,
                    Entry::DeviceSideCheck,
                    false,
                    || self.device_check.negative_control(&name, method),
                );
                match checked {
                    Ok(Ok(())) => negative_empty = Some(true),
                    Ok(Err(CheckFailure::Residual(detail))) => {
                        self.unconfirmed(format!(
                            "{name}: a bench scrcpy server runs before the open: {detail}"
                        ));
                        return Err(SegmentEnd::Whole);
                    }
                    Ok(Err(CheckFailure::Unconfirmed(detail))) => {
                        self.journal.event(
                            "s_negative_control_failed",
                            json!({ "instance": name, "detail": detail }),
                        );
                        return Err(SegmentEnd::Fatal("s_negative_control_failed".to_owned()));
                    }
                    Err(gate) => {
                        return Err(SegmentEnd::Skipped(CellResult::Untested(format!(
                            "the negative control was refused by {}",
                            gate.as_str()
                        ))));
                    }
                }
            }
            let opened = self.backends.open(&name, kind, options);
            match opened {
                Ok(backend) => {
                    let Some(method) = method else {
                        return Ok((backend, None));
                    };
                    let Some(stream) = backend.stream() else {
                        self.close_backend(backend, instance, "identity_unavailable", None);
                        self.unconfirmed(format!("{name} scrcpy opened without stream facts"));
                        return Err(SegmentEnd::Whole);
                    };
                    self.journal.event(
                        "s_open",
                        json!({ "instance": name, "scid": stream.scid_text(), "local_port": stream.local_port }),
                    );
                    match self.positive_identity(instance, method, &stream) {
                        Ok(identity) => {
                            self.journal.event(
                                "scrcpy_identity",
                                json!({ "instance": name, "identity": identity }),
                            );
                            self.journal.event(
                                "identity_positive",
                                json!({
                                    "instance": name,
                                    "scid": stream.scid_text(),
                                    "method": method,
                                    "matches": 1,
                                    "argv_equal": identity.argv == stream.expected_argv(),
                                    "negative_control_empty": negative_empty,
                                    "output_complete": true,
                                }),
                            );
                            return Ok((backend, Some(identity)));
                        }
                        Err(failure) => {
                            // The secretary's constraint: S started and its identity could not
                            // be read, so its close cannot be confirmed: the run is
                            // `unconfirmed` as a whole and stops.
                            self.journal.event(
                                "scrcpy_identity_failed",
                                json!({ "instance": name, "failure": failure }),
                            );
                            self.close_backend(backend, instance, "identity_unconfirmed", None);
                            self.unconfirmed(format!(
                                "{name} scrcpy {} started without a confirmed identity: {}",
                                stream.scid_text(),
                                failure.detail()
                            ));
                            return Err(SegmentEnd::Whole);
                        }
                    }
                }
                Err(failure) => {
                    let handling = self.handle_open_failure(instance, kind, method, &failure);
                    self.journal.event(
                        "open_failed",
                        json!({ "instance": name, "backend": kind, "failure": failure, "handling": handling }),
                    );
                    match handling {
                        OpenFailureHandling::Reopen if !cold_start && *reopens < 2 => {
                            *reopens += 1;
                            continue;
                        }
                        OpenFailureHandling::WholeUnconfirmed => return Err(SegmentEnd::Whole),
                        OpenFailureHandling::PathNotReady => {
                            lock(&self.scrcpy_not_ready)
                                .insert(instance, failure.cause.as_str().to_owned());
                            return Err(SegmentEnd::Skipped(CellResult::ConditionalSkip(format!(
                                "scrcpy not ready: {}",
                                failure.cause.as_str()
                            ))));
                        }
                        OpenFailureHandling::CellInstance => {
                            for path in [PathKind::Nemu, PathKind::Scrcpy, PathKind::BModel] {
                                lock(&self.disabled).insert(
                                    (instance, path),
                                    format!("device_verify: {}", failure.detail),
                                );
                            }
                            return Err(SegmentEnd::Fatal(format!(
                                "open_{}",
                                failure.cause.as_str()
                            )));
                        }
                        _ => {
                            return Err(SegmentEnd::Fatal(format!(
                                "open_{}",
                                failure.cause.as_str()
                            )));
                        }
                    }
                }
            }
        }
    }

    /// Table B's preconditions: the open's own cleanup confirmed, and the server and the exact
    /// forward of the failed stream gone on the device. Either missing is `unconfirmed`.
    fn handle_open_failure(
        &self,
        instance: usize,
        kind: BackendKind,
        method: Option<IdentityMethod>,
        failure: &OpenFailure,
    ) -> OpenFailureHandling {
        let name = self.instance_name(instance).to_owned();
        if !failure.cleanup_confirmed {
            self.unconfirmed(format!(
                "{name} {} open failed ({}) and its cleanup is unconfirmed",
                kind.as_str(),
                failure.cause.as_str()
            ));
            return OpenFailureHandling::WholeUnconfirmed;
        }
        if let (Some(stream), Some(method)) = (&failure.stream, method) {
            let checked = aux_call(
                &self.hub,
                &self.owner(),
                instance,
                Entry::DeviceSideCheck,
                true,
                || self.device_check.stream_absent(&name, method, stream),
            );
            match checked {
                Ok(Ok(())) => {}
                Ok(Err(check)) => {
                    self.unconfirmed(format!(
                        "{name} scrcpy {} after a failed open: {}",
                        stream.scid_text(),
                        check.detail()
                    ));
                    return OpenFailureHandling::WholeUnconfirmed;
                }
                Err(gate) => {
                    self.unconfirmed(format!(
                        "{name} scrcpy {} after a failed open: the check was refused by {}",
                        stream.scid_text(),
                        gate.as_str()
                    ));
                    return OpenFailureHandling::WholeUnconfirmed;
                }
            }
        }
        open_failure_handling(failure.cause)
    }

    /// §3.5.1 正例: at most three reads (the first and two more).
    fn positive_identity(
        &self,
        instance: usize,
        method: IdentityMethod,
        stream: &StreamFacts,
    ) -> Result<IdentityTuple, CheckFailure> {
        let name = self.instance_name(instance).to_owned();
        let mut last = CheckFailure::Unconfirmed("no read".to_owned());
        for _ in 0..3 {
            let checked = aux_call(
                &self.hub,
                &self.owner(),
                instance,
                Entry::DeviceSideCheck,
                false,
                || self.device_check.positive(&name, method, stream),
            );
            match checked {
                Ok(Ok(identity)) => return Ok(identity),
                Ok(Err(failure)) => last = failure,
                Err(gate) => {
                    last = CheckFailure::Unconfirmed(format!("refused by {}", gate.as_str()))
                }
            }
        }
        Err(last)
    }

    /// Closes one backend through the supervised path and, for S, confirms on the device that
    /// the same identity is gone (§3.5, §3.5.1). Returns whether the close is confirmed.
    fn close_backend(
        &self,
        backend: Box<dyn PreviewBackend>,
        instance: usize,
        reason: &str,
        identity: Option<IdentityTuple>,
    ) -> bool {
        let name = self.instance_name(instance).to_owned();
        let kind = backend.kind();
        let generation = self
            .hub
            .snapshot(&self.owner(), |state| state.instances[instance].generation);
        let outcome = self.closer.close(
            backend,
            CloseInfo {
                reason: reason.to_owned(),
                instance: name.clone(),
                generation,
                identity: identity.clone(),
            },
        );
        if self.terminal.finished_by_supervisor() {
            return false;
        }
        let mut record = json!({
            "instance": name,
            "backend": kind,
            "reason": reason,
            "host_side": outcome,
        });
        let mut confirmed = outcome.is_confirmed();
        if !confirmed {
            self.unconfirmed(format!("{name} {} close: {outcome:?}", kind.as_str()));
        }
        if let Some(identity) = identity {
            let checked = aux_call(
                &self.hub,
                &self.owner(),
                instance,
                Entry::DeviceSideCheck,
                true,
                || self.device_check.gone(&name, &identity),
            );
            let device_side = match checked {
                Ok(Ok(GoneResult::Gone)) => json!({ "gone": true, "forward_removed": true }),
                Ok(Ok(GoneResult::Rebooted)) => {
                    self.journal
                        .event("device_rebooted", json!({ "instance": name }));
                    lock(&self.rebooted).insert(instance);
                    if let Some(state) = self.apk_states.get(instance) {
                        let client = {
                            let mut state = lock(state);
                            state.needs_calibration = true;
                            state.client.take()
                        };
                        if let Some(client) = client {
                            let _ = client.close();
                        }
                    }
                    for path in [PathKind::Nemu, PathKind::Scrcpy, PathKind::BModel] {
                        lock(&self.disabled)
                            .entry((instance, path))
                            .or_insert_with(|| "device_rebooted".to_owned());
                    }
                    json!({ "gone": true, "rebooted": true })
                }
                Ok(Err(failure)) => {
                    confirmed = false;
                    let label = match &failure {
                        CheckFailure::Unconfirmed(_) => "device_side_unconfirmed",
                        CheckFailure::Residual(_) => "device_side_residual",
                    };
                    self.unconfirmed(format!(
                        "{name} scrcpy {:08x}: {label}: {}",
                        identity.scid,
                        failure.detail()
                    ));
                    json!({ "gone": false, "failure": failure })
                }
                Err(gate) => {
                    confirmed = false;
                    self.unconfirmed(format!(
                        "{name} scrcpy {:08x}: the device-side check was refused by {}",
                        identity.scid,
                        gate.as_str()
                    ));
                    json!({ "gone": false, "refused_by": gate })
                }
            };
            if let Value::Object(map) = &mut record {
                map.insert("device_side".to_owned(), device_side);
            }
        }
        if let Value::Object(map) = &mut record {
            map.insert("confirmed".to_owned(), json!(confirmed));
        }
        lock(&self.closes).push(record);
        confirmed
    }

    /// Takes the backend of `kind` out of `instance`'s slots, waiting for admission (so no call
    /// is in flight on it).
    fn take_backend(&self, instance: usize, kind: BackendKind) -> Option<Box<dyn PreviewBackend>> {
        self.hub.admission[instance].acquire(&self.owner());
        let taken = lock(&self.hub.backends[instance]).take(kind);
        self.hub.admission[instance].release(&self.owner());
        taken
    }

    fn put_backend(&self, instance: usize, backend: Box<dyn PreviewBackend>) {
        self.hub.admission[instance].acquire(&self.owner());
        lock(&self.hub.backends[instance]).put(backend);
        self.hub.admission[instance].release(&self.owner());
    }

    /// What a backend failure does (§3.11.4): `true` when the activity must stop.
    fn on_failure(
        &self,
        segment: &SegmentShared,
        instance: usize,
        kind: BackendKind,
        failure: &BackendFailure,
    ) -> bool {
        let now = self.now();
        let name = self.instance_name(instance).to_owned();
        match failure.class {
            FailureClass::Measured => false,
            FailureClass::StdioBusy => {
                if self.stdio_unrestored.load(Ordering::Acquire) {
                    self.terminal.request(
                        now,
                        StopCause::StdioUnrestored(format!(
                            "{name}: stdio busy after an unconfirmed restore"
                        )),
                    );
                    return true;
                }
                let streak = segment.busy_streak.fetch_add(1, Ordering::AcqRel) + 1;
                if failure.native_started || streak > 3 {
                    lock(&segment.fatal).push((kind, failure.clone()));
                    lock(&segment.disabled_backends).insert(kind);
                    return true;
                }
                false
            }
            FailureClass::CellPath => {
                lock(&segment.fatal).push((kind, failure.clone()));
                lock(&segment.disabled_backends).insert(kind);
                true
            }
            FailureClass::CellInstance => {
                *lock(&segment.instance_fatal) = Some(failure.code.clone());
                lock(&segment.disabled_backends).insert(BackendKind::Nemu);
                lock(&segment.disabled_backends).insert(BackendKind::Scrcpy);
                true
            }
            FailureClass::Poisoned => {
                self.terminal.request(
                    now,
                    StopCause::Poisoned(format!("{name} {}: {}", kind.as_str(), failure.detail)),
                );
                true
            }
            FailureClass::Unconfirmed => {
                self.unconfirmed(format!("{name} {}: {}", kind.as_str(), failure.detail));
                true
            }
            FailureClass::StdioUnrestored => {
                self.stdio_unrestored.store(true, Ordering::Release);
                self.terminal.request(
                    now,
                    StopCause::StdioUnrestored(format!("{name}: {}", failure.detail)),
                );
                true
            }
        }
    }

    /// One segment: open what it needs, run its activities under the controller, close.
    #[allow(clippy::too_many_arguments)]
    fn run_segment(
        self: &Arc<Self>,
        cell: &CellPlan,
        segment: &SegmentPlan,
        instance: usize,
        info: &SegmentInfo,
        cell_deadline: Micros,
        rng: &mut Rng,
        concurrent: bool,
    ) -> SegmentEnd {
        let label = info.label.as_str();
        if let Some(path) = segment.path {
            if let Some(code) = lock(&self.cell_fatal)
                .get(&(instance, path.backend()))
                .cloned()
            {
                return SegmentEnd::Fatal(code);
            }
            if let Some(reason) = lock(&self.disabled).get(&(instance, path)).cloned() {
                return SegmentEnd::Skipped(CellResult::Untested(format!("prior fatal: {reason}")));
            }
            if path == PathKind::Scrcpy
                && let Some(reason) = lock(&self.scrcpy_not_ready).get(&instance).cloned()
            {
                return SegmentEnd::Skipped(CellResult::ConditionalSkip(format!(
                    "scrcpy not ready: {reason}"
                )));
            }
        }
        self.set_status(
            &cell.id,
            json!({
                "segment": info.name,
                "label": label,
                "round": info.round,
                "orientation": info.orientation,
                "instance": segment.instance,
                "path": segment.path,
                "mode": segment.mode,
            }),
        );
        self.journal.event(
            "segment_start",
            json!({ "cell": cell.id, "segment": label, "plan": segment }),
        );
        if segment.mode == SegmentMode::ColdStart {
            return self.run_cold_start(cell, segment, instance, info, cell_deadline);
        }
        if matches!(
            segment.mode,
            SegmentMode::Calibrate
                | SegmentMode::StepTrials
                | SegmentMode::Rotate
                | SegmentMode::Reconnect
                | SegmentMode::StaticImages
        ) {
            return self.run_procedure(cell, segment, instance, info, cell_deadline, rng);
        }
        self.apk_segment_start(instance, segment, &cell.id);
        let options = OpenOptions {
            scrcpy_bit_rate: segment.scrcpy.as_ref().and_then(|scrcpy| scrcpy.bit_rate),
            scrcpy_max_fps: segment.scrcpy.as_ref().and_then(|scrcpy| scrcpy.max_fps),
            scrcpy_encoder: segment
                .scrcpy
                .as_ref()
                .and_then(|scrcpy| scrcpy.encoder.clone()),
        };
        // The backends: the path's, and the formal loop's (Nemu, plus S for the C7 variant).
        let mut kinds = BTreeSet::new();
        if let Some(path) = segment.path {
            kinds.insert(path.backend());
        }
        if let Some(formal) = &segment.formal {
            kinds.insert(BackendKind::Nemu);
            if formal.capture_from_scrcpy {
                kinds.insert(BackendKind::Scrcpy);
            }
        }
        let mut identities: BTreeMap<BackendKind, IdentityTuple> = BTreeMap::new();
        let mut reopens = 0;
        for kind in kinds.iter().copied() {
            match self.open_backend(instance, kind, &options, &mut reopens, false) {
                Ok((backend, identity)) => {
                    if let Some(identity) = identity {
                        identities.insert(kind, identity);
                    }
                    self.put_backend(instance, backend);
                }
                Err(end) => {
                    self.close_segment_backends(instance, &mut identities, "open_failed");
                    return end;
                }
            }
        }
        if segment.path == Some(PathKind::BModel) {
            match self.materials.open(self.instance_name(instance), label) {
                Ok(writer) => {
                    self.hub.admission[instance].acquire(&self.owner());
                    lock(&self.hub.backends[instance]).material = Some(writer);
                    self.hub.admission[instance].release(&self.owner());
                }
                Err(detail) => {
                    self.journal.event(
                        "material_writer_failed",
                        json!({ "segment": label, "detail": detail }),
                    );
                    self.close_segment_backends(instance, &mut identities, "material_failed");
                    return SegmentEnd::Fatal("b_model_writer_open".to_owned());
                }
            }
        }

        for identity in identities.values() {
            self.sampling_start(instance, identity);
        }
        let shared = SegmentShared::new();
        let preview = segment.mode == SegmentMode::Preview && segment.path.is_some();
        let producer = preview && self.hub.views.claim_producer(instance);
        if preview && !producer {
            self.journal.event(
                "producer_refused",
                json!({ "segment": label, "reason": "one preview producer per instance" }),
            );
        }
        // The viewers and their leases (§3.4).
        let mut viewers: Vec<Box<dyn ViewerHandle>> = Vec::new();
        if segment.path.is_some() {
            for index in 0..segment.viewers.max(1) {
                let subscriber = self.next_subscriber.fetch_add(1, Ordering::Relaxed);
                let context = ViewerContext {
                    hub: Arc::clone(&self.hub),
                    journal: Arc::clone(&self.journal),
                    instance,
                    subscriber,
                    ttl_us: ms(segment.lease_ttl_ms),
                    slow_fps: if index == 0 {
                        segment.slow_viewer_fps
                    } else {
                        None
                    },
                    stop_renew_at_us: segment
                        .viewer_stop_renew_ms
                        .map(|offset| self.now() + ms(offset)),
                    unsubscribe_at_us: segment
                        .viewer_unsubscribe_ms
                        .map(|offset| self.now() + ms(offset)),
                    grace_us: ms(segment.grace_ms),
                    gdi: index == 0 && segment.gdi_viewer,
                };
                match self.viewers.start(context) {
                    Ok(viewer) => viewers.push(viewer),
                    Err(detail) => self.journal.event(
                        "viewer_failed",
                        json!({ "segment": label, "detail": detail }),
                    ),
                }
            }
        }

        if producer {
            self.journal.event(
                "producer_count",
                json!({
                    "instance": self.instance_name(instance),
                    "segment": label,
                    "count": self.hub.views.producers(instance),
                    "viewers": viewers.iter().map(|viewer| viewer.subscriber()).collect::<Vec<_>>(),
                }),
            );
        }
        // The activities.
        let mut activities: Vec<Participant<()>> = Vec::new();
        let seed = rng.next();
        if preview
            && producer
            && let Some(path) = segment.path
        {
            let run = Arc::clone(self);
            let segment_shared = Arc::clone(&shared);
            let fps = segment.fps;
            let quota = segment.quota;
            let file = format!("opps-{}-{}.jsonl", cell.id, path.as_str());
            let info = info.clone();
            let reconnect = segment.reconnect_each_sample;
            if let Ok(thread) =
                spawn_participant(&self.clock, &format!("preview-{label}"), move || {
                    run.preview_activity(
                        &segment_shared,
                        instance,
                        path,
                        fps,
                        quota,
                        &file,
                        &info,
                        reconnect,
                    );
                })
            {
                activities.push(thread);
            }
        }
        if let Some(formal) = segment.formal.clone() {
            let run = Arc::clone(self);
            let segment_shared = Arc::clone(&shared);
            let preview_state = formal
                .preview_state
                .clone()
                .unwrap_or_else(|| segment.path.map_or("off", PathKind::as_str).to_owned());
            let record_path = formal
                .record_path
                .clone()
                .unwrap_or_else(|| preview_state.clone());
            let file = format!("opps-{}-{record_path}.jsonl", cell.id);
            let info = info.clone();
            let grid = segment.tap_grid;
            if let Ok(thread) =
                spawn_participant(&self.clock, &format!("formal-{label}"), move || {
                    run.formal_activity(
                        &segment_shared,
                        instance,
                        &formal,
                        grid,
                        seed,
                        &file,
                        &info,
                        &preview_state,
                        &record_path,
                    );
                })
            {
                activities.push(thread);
            }
        }
        if matches!(
            segment.mode,
            SegmentMode::TaskFloor | SegmentMode::PollSequence
        ) && let Some(path) = segment.path
        {
            let run = Arc::clone(self);
            let segment_shared = Arc::clone(&shared);
            let file = format!("opps-{}-{}.jsonl", cell.id, path.as_str());
            let info = info.clone();
            let grid = segment.tap_grid;
            let quota = segment.quota;
            let poll = segment.mode == SegmentMode::PollSequence;
            if let Ok(thread) =
                spawn_participant(&self.clock, &format!("task-{label}"), move || {
                    run.task_activity(
                        &segment_shared,
                        instance,
                        path,
                        poll,
                        grid,
                        quota,
                        seed,
                        &file,
                        &info,
                    );
                })
            {
                activities.push(thread);
            }
        }

        // The controller.
        let started = self.now();
        let end_time = segment
            .duration_ms
            .map(|duration| started.saturating_add(ms(duration)))
            .unwrap_or(Micros::MAX)
            .min(cell_deadline);
        let mut gate_events: Vec<(Micros, GateKind, bool)> = Vec::new();
        for script in &segment.gates {
            gate_events.push((started + ms(script.at_ms), script.gate, true));
            if let Some(hold) = script.hold_ms {
                gate_events.push((started + ms(script.at_ms) + ms(hold), script.gate, false));
            }
        }
        gate_events.sort_by_key(|(at, _, set)| (*at, !*set));
        let mut next_sweep = started + ms(segment.lease_check_ms);
        let mut set_gates: Vec<GateKind> = Vec::new();
        let mut closed_by_gate: BTreeSet<BackendKind> = BTreeSet::new();
        let end = loop {
            self.apk_periodic(instance);
            let now = self.now();
            self.check_time();
            if self.whole_stop_requested() {
                break SegmentEnd::Whole;
            }
            if let Some(code) = lock(&shared.instance_fatal).clone() {
                for path in [PathKind::Nemu, PathKind::Scrcpy, PathKind::BModel] {
                    lock(&self.disabled).insert((instance, path), code.clone());
                }
                break SegmentEnd::Fatal(code);
            }
            let fatal_path = segment.path.and_then(|path| {
                lock(&shared.fatal)
                    .iter()
                    .find(|(kind, _)| *kind == path.backend())
                    .map(|(_, failure)| failure.code.clone())
            });
            if let Some(code) = fatal_path {
                break SegmentEnd::Fatal(code);
            }
            while let Some((at, gate, set)) = gate_events.first().copied() {
                if at > now {
                    break;
                }
                gate_events.remove(0);
                self.apply_gate(
                    instance,
                    gate,
                    set,
                    &mut set_gates,
                    &mut closed_by_gate,
                    &mut identities,
                    &options,
                );
            }
            if now >= next_sweep {
                next_sweep = now + ms(segment.lease_check_ms);
                self.hub.sweep(&self.owner(), ms(segment.grace_ms));
                if self.hub.grace_expired(&self.owner(), instance) == Some(true)
                    && let Some(backend) = self.take_backend(instance, BackendKind::Scrcpy)
                {
                    let identity = identities.remove(&BackendKind::Scrcpy);
                    self.journal
                        .event("grace_expired_closed", json!({ "segment": label }));
                    if !self.close_backend(backend, instance, "grace_expired", identity) {
                        break SegmentEnd::Whole;
                    }
                    closed_by_gate.insert(BackendKind::Scrcpy);
                }
            }
            for viewer in viewers.iter_mut() {
                viewer.tick(now);
            }
            let quota_done = match segment.mode {
                SegmentMode::Preview => segment
                    .quota
                    .is_some_and(|quota| shared.opportunities.load(Ordering::Acquire) >= quota),
                SegmentMode::TaskFloor | SegmentMode::PollSequence => segment
                    .quota
                    .is_some_and(|quota| shared.floor_checks.load(Ordering::Acquire) >= quota),
                // These modes end in their own procedures and never reach the controller.
                SegmentMode::ColdStart
                | SegmentMode::Calibrate
                | SegmentMode::StepTrials
                | SegmentMode::Rotate
                | SegmentMode::Reconnect
                | SegmentMode::StaticImages => false,
            } || segment
                .formal
                .as_ref()
                .and_then(|formal| formal.quota)
                .is_some_and(|quota| {
                    shared.dispatches.load(Ordering::Acquire) >= quota && segment.quota.is_none()
                });
            if quota_done || (segment.duration_ms.is_some() && now >= end_time) {
                break SegmentEnd::Finished;
            }
            if now >= cell_deadline {
                break SegmentEnd::CellLimit;
            }
            let next_gate = gate_events.first().map_or(Micros::MAX, |(at, _, _)| *at);
            let wake = (now + self.limits.controller_period_us)
                .min(next_gate)
                .min(next_sweep)
                .min(end_time.max(now + 1))
                .min(cell_deadline.max(now + 1));
            pause_until(self.clock.as_ref(), wake);
        };

        // §3.11.2 A/B step 1: no new grants (the cell-end gate; the whole-run stop gate is set
        // already when the run stops), then step 2: the activities stop at their next
        // checkpoint and are joined (the drain).
        // A concurrent segment ends its own instance only (C9): the instance's stop gate.
        let segment_gate = if concurrent {
            self.hub
                .set_gate(&self.owner(), instance, GateKind::StopClass, true)
        } else {
            self.hub
                .set_global(&self.owner(), GlobalGate::CellEnd, true)
        };
        self.journal.event(
            "segment_stop_new",
            json!({ "segment": label, "gate_seq": segment_gate, "end": format!("{end:?}") }),
        );
        let drain_start = self.now();
        shared.stop_at.store(drain_start.max(1), Ordering::Release);
        shared.stop.store(true, Ordering::Release);
        for activity in activities {
            let _ = activity.join();
        }
        self.journal.event(
            "drain",
            json!({
                "instance": self.instance_name(instance),
                "segment": label,
                "t_start_us": drain_start,
                "t_end_us": self.now(),
                "calls": *lock(&shared.drained),
            }),
        );
        if self.terminal.finished_by_supervisor() {
            return SegmentEnd::Whole;
        }
        for mut viewer in viewers {
            viewer.stop(true);
        }
        if producer {
            self.hub.views.release_producer(instance);
            self.journal.event(
                "producer_count",
                json!({
                    "instance": self.instance_name(instance),
                    "segment": label,
                    "count": self.hub.views.producers(instance),
                    "viewers": [],
                }),
            );
        }
        for gate in set_gates {
            self.hub.set_gate(&self.owner(), instance, gate, false);
        }
        // The fatal backends close first (no more samples on them), then the rest.
        let fatal: Vec<(BackendKind, BackendFailure)> = lock(&shared.fatal).clone();
        for (kind, failure) in &fatal {
            lock(&self.cell_fatal)
                .entry((instance, *kind))
                .or_insert_with(|| failure.code.clone());
            for path in kind.paths() {
                lock(&self.disabled)
                    .entry((instance, *path))
                    .or_insert_with(|| failure.code.clone());
            }
            self.journal.event(
                "cell_path_stopped",
                json!({ "segment": label, "backend": kind, "first_error": failure }),
            );
        }
        self.hub.admission[instance].acquire(&self.owner());
        let material = lock(&self.hub.backends[instance]).material.take();
        self.hub.admission[instance].release(&self.owner());
        drop(material);
        let confirmed = self.close_segment_backends(instance, &mut identities, "segment_end");
        if concurrent {
            self.hub
                .set_gate(&self.owner(), instance, GateKind::StopClass, false);
        } else {
            self.hub
                .set_global(&self.owner(), GlobalGate::CellEnd, false);
        }
        if let Some(path) = segment.path {
            let state_changes = 2 * kinds.len() as u64 + segment.gates.len() as u64;
            self.write_artifact_delta(info, path, &shared, state_changes);
        }
        self.apk_fetch_records(instance);
        self.write_hub_notes();
        if !confirmed && end != SegmentEnd::Whole {
            return SegmentEnd::Whole;
        }
        self.journal.event(
            "segment_end",
            json!({
                "segment": label,
                "end": format!("{end:?}"),
                "opportunities": shared.opportunities.load(Ordering::Acquire),
                "floor_checks": shared.floor_checks.load(Ordering::Acquire),
                "dispatches": shared.dispatches.load(Ordering::Acquire),
            }),
        );
        end
    }

    /// Closes every backend left in the instance's slots; S with its device-side check.
    fn close_segment_backends(
        &self,
        instance: usize,
        identities: &mut BTreeMap<BackendKind, IdentityTuple>,
        reason: &str,
    ) -> bool {
        let mut confirmed = true;
        for kind in [BackendKind::Scrcpy, BackendKind::Nemu] {
            if let Some(backend) = self.take_backend(instance, kind) {
                let identity = identities.remove(&kind);
                if !self.close_backend(backend, instance, reason, identity) {
                    confirmed = false;
                }
            }
        }
        confirmed
    }

    /// A scripted gate (§3.7.2): set it, then close what the table closes; a cleared gate moves
    /// the generation and reopens what the gate closed.
    #[allow(clippy::too_many_arguments)]
    fn apply_gate(
        &self,
        instance: usize,
        gate: GateKind,
        set: bool,
        set_gates: &mut Vec<GateKind>,
        closed_by_gate: &mut BTreeSet<BackendKind>,
        identities: &mut BTreeMap<BackendKind, IdentityTuple>,
        options: &OpenOptions,
    ) {
        if set {
            let seq = self.hub.set_gate(&self.owner(), instance, gate, true);
            set_gates.push(gate);
            self.journal.event(
                "gate_applied",
                json!({ "instance": self.instance_name(instance), "gate": gate, "seq": seq }),
            );
            if gate.is_hard() {
                self.hub.end_grace_hard(&self.owner(), instance);
            }
            for entry in [Entry::ScrcpyProducer, Entry::NemuSession] {
                self.hub.note_entry(instance, entry, &[gate], false);
            }
            for (entry, kind) in [
                (Entry::ScrcpyProducer, BackendKind::Scrcpy),
                (Entry::NemuSession, BackendKind::Nemu),
            ] {
                if crate::gates::reaction(entry, gate).closes()
                    && let Some(backend) = self.take_backend(instance, kind)
                {
                    let identity = identities.remove(&kind);
                    if self.close_backend(
                        backend,
                        instance,
                        &format!("gate:{}", gate.as_str()),
                        identity,
                    ) {
                        closed_by_gate.insert(kind);
                    }
                }
            }
            if gate == GateKind::HoldRequest {
                self.complete_hold(instance, set_gates);
            }
        } else {
            for cleared in [gate, GateKind::Holding] {
                if let Some(position) = set_gates.iter().position(|known| *known == cleared) {
                    set_gates.remove(position);
                    self.hub.set_gate(&self.owner(), instance, cleared, false);
                }
            }
            let generation = self.hub.bump_generation(&self.owner(), instance);
            self.journal.event(
                "gate_cleared",
                json!({ "instance": self.instance_name(instance), "gate": gate, "generation": generation }),
            );
            let reopen: Vec<BackendKind> = closed_by_gate.iter().copied().collect();
            for kind in reopen {
                let mut reopens = 0;
                match self.open_backend(instance, kind, options, &mut reopens, false) {
                    Ok((backend, identity)) => {
                        if let Some(identity) = identity {
                            identities.insert(kind, identity);
                        }
                        self.put_backend(instance, backend);
                        closed_by_gate.remove(&kind);
                    }
                    Err(end) => {
                        self.journal.event(
                            "reopen_after_gate_failed",
                            json!({ "instance": self.instance_name(instance), "backend": kind, "end": format!("{end:?}") }),
                        );
                    }
                }
            }
        }
    }

    /// §3.7.3: the hold is `held` only when the formal work is drained, the input is reset (Nemu
    /// input cannot reset: `not_supported`) and the shared session and S are closed and
    /// confirmed. The closes ran in [`Self::apply_gate`]; here the drain and the record.
    fn complete_hold(&self, instance: usize, set_gates: &mut Vec<GateKind>) {
        let name = self.instance_name(instance).to_owned();
        // Drain: wait for the in-flight table of the instance (the formal loop stops at its next
        // checkpoint because E7 refuses new work under the hold request).
        let drain_deadline = self.now() + self.limits.nemu_close_deadline_us;
        while self.hub.inflight(&self.owner(), instance) > 0 && self.now() < drain_deadline {
            pause_until(self.clock.as_ref(), self.now() + ms(10));
        }
        let drained = self.hub.inflight(&self.owner(), instance) == 0;
        let open_after: Vec<&str> = {
            self.hub.admission[instance].acquire(&self.owner());
            let slots = lock(&self.hub.backends[instance]);
            let mut open = Vec::new();
            if slots.nemu.is_some() {
                open.push("nemu");
            }
            if slots.scrcpy.is_some() {
                open.push("scrcpy");
            }
            drop(slots);
            self.hub.admission[instance].release(&self.owner());
            open
        };
        let unconfirmed_now = !lock(&self.unconfirmed).is_empty();
        if drained && open_after.is_empty() && !unconfirmed_now {
            self.hub
                .set_gate(&self.owner(), instance, GateKind::Holding, true);
            set_gates.push(GateKind::Holding);
            self.journal.event(
                "held",
                json!({
                    "instance": name,
                    "t_held_us": self.now(),
                    "drained": true,
                    "input_reset": match self.plan.instances[instance].touch {
                        crate::plan::TouchKind::NemuIpc => "not_supported",
                        _ => "no_contact_open",
                    },
                    "sessions_closed": true,
                }),
            );
        } else if unconfirmed_now {
            self.journal.event(
                "hold_unconfirmed",
                json!({ "instance": name, "drained": drained, "open": open_after }),
            );
        } else {
            self.journal.event(
                "hold_failed",
                json!({ "instance": name, "drained": drained, "open": open_after }),
            );
            for path in [PathKind::Nemu, PathKind::Scrcpy, PathKind::BModel] {
                lock(&self.disabled)
                    .entry((instance, path))
                    .or_insert_with(|| "hold_failed".to_owned());
            }
        }
    }

    /// C0 and C2: open to the first frame, close and confirm, `reps` times. A failed cold start
    /// counts as +∞ and is not reopened (§3.11.4).
    fn run_cold_start(
        self: &Arc<Self>,
        cell: &CellPlan,
        segment: &SegmentPlan,
        instance: usize,
        info: &SegmentInfo,
        cell_deadline: Micros,
    ) -> SegmentEnd {
        let Some(path) = segment.path else {
            return SegmentEnd::Finished;
        };
        let kind = path.backend();
        let file = format!("opps-{}-{}.jsonl", cell.id, path.as_str());
        let options = OpenOptions::default();
        for rep in 1..=segment.reps.unwrap_or(0) {
            self.check_time();
            if self.whole_stop_requested() {
                return SegmentEnd::Whole;
            }
            if self.now() >= cell_deadline {
                return SegmentEnd::CellLimit;
            }
            let started = self.now();
            let mut reopens = 2;
            let opened = self.open_backend(instance, kind, &options, &mut reopens, true);
            let ready = self.now();
            match opened {
                Ok((backend, identity)) => {
                    self.put_backend(instance, backend);
                    let first = self.with_backend(instance, kind, |backend| backend.sample(None));
                    let first_frame = self.now();
                    let backend = self.take_backend(instance, kind);
                    let closed = backend.is_some_and(|backend| {
                        self.close_backend(backend, instance, "cold_start", identity)
                    });
                    let disposition = match &first {
                        Ok(_) => "delivered".to_owned(),
                        Err(failure) => format!("failed:{}", failure.code),
                    };
                    let line = info.record(
                        "cold_start",
                        path.as_str(),
                        self.next_seq(),
                        json!({
                            "attempt": rep,
                            "t_tick_us": started,
                            "t_start_us": started,
                            "t_open_us": ready,
                            "t_first_frame_us": first.as_ref().ok().map(|_| first_frame),
                            "disposition": disposition,
                            "failure": first.as_ref().err(),
                            "close_confirmed": closed,
                            "null_reasons": if first.is_ok() { json!({}) } else { json!({ "t_first_frame_us": disposition }) },
                        }),
                    );
                    self.journal.append(&file, &line);
                    if !closed {
                        return SegmentEnd::Whole;
                    }
                }
                Err(SegmentEnd::Whole) => return SegmentEnd::Whole,
                Err(end) => {
                    let line = info.record(
                        "cold_start",
                        path.as_str(),
                        self.next_seq(),
                        json!({
                            "attempt": rep,
                            "t_tick_us": started,
                            "t_start_us": started,
                            "t_first_frame_us": null,
                            "disposition": format!("failed:open_{}", match &end {
                                SegmentEnd::Fatal(code) => code.clone(),
                                other => format!("{other:?}"),
                            }),
                            "counted_as": "infinity",
                            "null_reasons": { "t_first_frame_us": "the open failed" },
                        }),
                    );
                    self.journal.append(&file, &line);
                    if let SegmentEnd::Skipped(_) = end {
                        return end;
                    }
                }
            }
        }
        SegmentEnd::Finished
    }

    fn with_backend<T>(
        &self,
        instance: usize,
        kind: BackendKind,
        call: impl FnOnce(&mut Box<dyn PreviewBackend>) -> Result<T, BackendFailure>,
    ) -> Result<T, BackendFailure> {
        self.hub.admission[instance].acquire(&self.owner());
        let result = match lock(&self.hub.backends[instance]).get(kind) {
            Some(backend) => call(backend),
            None => Err(BackendFailure::new(
                "backend_closed",
                FailureClass::CellPath,
                "the backend is not open",
            )
            .without_native_call()),
        };
        self.hub.admission[instance].release(&self.owner());
        result
    }

    /// The preview sampler (P1 at `fps`): one opportunity per tick, each with its disposition
    /// (§5.4), as an analysis-schema `preview` record (§3.2).
    #[allow(clippy::too_many_arguments)]
    fn preview_activity(
        &self,
        segment: &SegmentShared,
        instance: usize,
        path: PathKind,
        fps: f64,
        quota: Option<u64>,
        file: &str,
        info: &SegmentInfo,
        reconnect: bool,
    ) {
        let owner = LockOwner::new(format!(
            "preview/{}/{}",
            self.instance_name(instance),
            path.as_str()
        ));
        let period = ((1_000_000.0 / fps).round() as Micros).max(1);
        let mut next = self.now();
        let mut opportunity = 0u64;
        while !segment.stopped() {
            if quota.is_some_and(|quota| opportunity >= quota)
                || segment.backend_disabled(path.backend())
            {
                break;
            }
            opportunity += 1;
            segment.opportunities.fetch_add(1, Ordering::AcqRel);
            let tick = self.now();
            let reconnected = (reconnect && path.backend() == BackendKind::Nemu).then(|| {
                let started = self.now();
                let reopened = self.reconnect_nemu_for_sample(instance);
                json!({
                    "t_start_us": started,
                    "t_end_us": self.now(),
                    "generation": reopened.as_ref().ok(),
                    "failure": reopened.as_ref().err(),
                })
            });
            let mut op = SampleOp::new(instance, path, Arc::clone(&owner));
            let outcome = run_blocking(&self.hub, &mut op);
            let delivered = outcome.disposition.is_delivered();
            if delivered {
                segment.delivered.fetch_add(1, Ordering::AcqRel);
            }
            if let Some(frame) = &outcome.frame {
                self.note_frame(instance, frame.width, frame.height);
            }
            if let Some(cost) = outcome.material {
                segment
                    .material_bytes
                    .fetch_add(cost.bytes, Ordering::AcqRel);
                segment
                    .material_events
                    .fetch_add(cost.events, Ordering::AcqRel);
            }
            let native_return = outcome
                .frame
                .as_ref()
                .and_then(|frame| frame.native.iter().filter_map(|span| span.end_us).max())
                .or(outcome.call_end_us);
            segment.note_drained(
                opportunity,
                outcome.call_start_us,
                outcome.call_end_us,
                native_return,
                outcome
                    .failure
                    .as_ref()
                    .is_some_and(|failure| !failure.native_returned),
            );
            let decoded = if delivered {
                self.hub
                    .views
                    .latest(instance)
                    .filter(|frame| Some(frame.preview_sequence) == outcome.preview_sequence)
                    .and_then(|frame| {
                        self.decode_frame(
                            instance,
                            &frame.pixels,
                            frame.width,
                            frame.height,
                            frame.layout,
                        )
                    })
            } else {
                None
            };
            let (content, mut null_reasons) = if delivered {
                self.content_of(decoded.as_ref())
            } else {
                (Value::Null, json!({ "content": "not delivered" }))
            };
            if !delivered && let Value::Object(map) = &mut null_reasons {
                map.insert(
                    "timing.t_pub_us".to_owned(),
                    json!(outcome.disposition.label()),
                );
            }
            let line = info.record(
                "preview",
                path.as_str(),
                self.next_seq(),
                json!({
                    "opportunity": opportunity,
                    "t_tick_us": tick,
                    "disposition": outcome.disposition.label(),
                    "generation": outcome.generation,
                    "grant_seq": outcome.grant_seq,
                    "preview_seq": outcome.preview_sequence,
                    "timing": timing_of(outcome.frame.as_ref(), json!({
                        "t_grant_us": outcome.grant_us,
                        "t_call_start_us": outcome.call_start_us,
                        "t_call_end_us": outcome.call_end_us,
                        "t_pub_us": outcome.published_us,
                        "pipeline_us": outcome.material.map(|_| outcome
                            .call_end_us
                            .zip(outcome.call_start_us)
                            .map(|(end, start)| end.saturating_sub(start))),
                    })),
                    "content": content,
                    "null_reasons": null_reasons,
                    "reconnect": reconnected,
                    "discard_reason": match &outcome.disposition {
                        Disposition::DiscardedStale(reason) => Some(reason.clone()),
                        _ => None,
                    },
                    "pumped_formal_grant": outcome.pumped,
                    "material": outcome.material,
                    "known_source_us": outcome.frame.as_ref().and_then(|frame| frame.known_source_us),
                    "failure": outcome.failure,
                }),
            );
            self.journal.append(file, &line);
            if let Some(failure) = &outcome.failure {
                if self.on_failure(segment, instance, path.backend(), failure) {
                    break;
                }
            } else if delivered {
                segment.busy_streak.store(0, Ordering::Release);
            }
            next = next.saturating_add(period).max(self.now());
            pause_until(self.clock.as_ref(), next);
        }
    }

    /// The simulated formal loop (§5.6): every period ± jitter, P2 → P3′ → P4 (capture, tap,
    /// floor capture), as analysis-schema `dispatch` records (§3.6).
    #[allow(clippy::too_many_arguments)]
    fn formal_activity(
        &self,
        segment: &SegmentShared,
        instance: usize,
        formal: &crate::plan::FormalPlan,
        grid: u32,
        seed: u64,
        file: &str,
        info: &SegmentInfo,
        preview_state: &str,
        record_path: &str,
    ) {
        let owner = LockOwner::new(format!("formal/{}", self.instance_name(instance)));
        let mut rng = Rng::new(seed ^ 0x5eed);
        let capture_kind = if formal.capture_from_scrcpy {
            BackendKind::Scrcpy
        } else {
            BackendKind::Nemu
        };
        let mut next = self.now();
        let mut dispatch = 0u64;
        while !segment.stopped() {
            if formal.quota.is_some_and(|quota| dispatch >= quota)
                || segment.backend_disabled(BackendKind::Nemu)
                || segment.backend_disabled(capture_kind)
            {
                break;
            }
            dispatch += 1;
            segment.dispatches.fetch_add(1, Ordering::AcqRel);
            let cell_index = rng.below(u64::from(grid * grid)) as u32;
            let clock = Arc::clone(&self.clock);
            let display = self.display_facts(instance);
            let work: crate::hub::FormalWork = Box::new(move |hub: &Hub, instance: usize| {
                formal_work(
                    hub,
                    instance,
                    capture_kind,
                    cell_index,
                    grid,
                    clock.as_ref(),
                    true,
                    display,
                    None,
                )
            });
            let tick = self.now();
            let mut op = DispatchOp::new(
                instance,
                Arc::clone(&owner),
                Arc::clone(&self.clock),
                Some(work),
            );
            let outcome = run_blocking(&self.hub, &mut op);
            let record = RecordShape::Dispatch {
                preview_state: preview_state.to_owned(),
                record_path: record_path.to_owned(),
            };
            segment.note_drained(dispatch, Some(tick), Some(self.now()), None, false);
            let stop = self.record_dispatch(
                segment,
                instance,
                capture_kind,
                &outcome,
                dispatch,
                tick,
                file,
                info,
                &record,
            );
            if stop {
                break;
            }
            let jitter = rng.below(2 * formal.jitter_ms + 1) as i64 - formal.jitter_ms as i64;
            let period = (formal.period_ms as i64 + jitter).max(1) as u64;
            next = next.saturating_add(ms(period)).max(self.now());
            pause_until(self.clock.as_ref(), next);
        }
    }

    /// One `dispatch`, `admission` or `poll` record (analysis schema §3.3, §3.4, §3.6).
    #[allow(clippy::too_many_arguments)]
    fn record_dispatch(
        &self,
        segment: &SegmentShared,
        instance: usize,
        kind: BackendKind,
        outcome: &DispatchOutcome,
        trial: u64,
        tick: Micros,
        file: &str,
        info: &SegmentInfo,
        shape: &RecordShape,
    ) -> bool {
        let path = match shape {
            RecordShape::Dispatch { record_path, .. } => record_path.clone(),
            _ => match kind {
                BackendKind::Nemu => "N".to_owned(),
                BackendKind::Scrcpy => "S".to_owned(),
            },
        };
        let extra = match outcome {
            DispatchOutcome::Granted {
                registration,
                formal_grant,
                registered_us,
                granted_us,
                by_pump,
                work,
            } => {
                let disposition = work.failure.as_ref().map_or_else(
                    || "delivered".to_owned(),
                    |failure| format!("failed:{}", failure.code),
                );
                let wait = granted_us.saturating_sub(*registered_us);
                let decoded = match shape {
                    RecordShape::Admission { .. } => work.floor_frame.as_ref().and_then(|frame| {
                        self.decode_frame(
                            instance,
                            &frame.pixels,
                            frame.width,
                            frame.height,
                            frame.layout,
                        )
                    }),
                    _ => None,
                };
                let (admission_content, mut admission_reasons) = match &work.floor_frame {
                    Some(_) => self.content_of(decoded.as_ref()),
                    None => (Value::Null, json!({ "content": "no frame was admitted" })),
                };
                if let Some(frame) = &work.floor_frame {
                    self.note_frame(instance, frame.width, frame.height);
                }
                let mut tap_record = work.tap_display.clone().unwrap_or_else(|| json!({}));
                let rotation_missing = tap_record["rotation"].is_null();
                if rotation_missing && let Value::Object(map) = &mut admission_reasons {
                    map.insert(
                        "tap.rotation".to_owned(),
                        json!("no APK INFO gave the display rotation"),
                    );
                }
                let tap_reasons = if rotation_missing {
                    json!({ "tap.rotation": "no APK INFO gave the display rotation" })
                } else {
                    json!({})
                };
                if work.first_flip.is_none()
                    && let Value::Object(map) = &mut admission_reasons
                {
                    map.insert(
                        "first_flip".to_owned(),
                        json!(if self.apk_enabled(instance) {
                            "no frame showed the tap's flip within the wait"
                        } else {
                            "no test-pattern APK on this instance"
                        }),
                    );
                }
                match shape {
                    RecordShape::Dispatch { preview_state, .. } => json!({
                        "trial": trial,
                        "t_tick_us": tick,
                        "disposition": disposition,
                        "preview_state": preview_state,
                        "contended": false,
                        "admission_wait_us": wait,
                        "inflight_wait_us": if *by_pump { wait } else { 0 },
                        "formal_captures": [
                            { "dur_us": work.capture_us, "ok": work.capture_us.is_some(), "code": work.failure.as_ref().filter(|_| work.capture_us.is_none()).map(|failure| failure.code.clone()) },
                            { "dur_us": work.floor_capture_us, "ok": work.floor_frame.is_some(), "code": work.failure.as_ref().filter(|_| work.floor_frame.is_none()).map(|failure| failure.code.clone()) },
                        ],
                        "tap_us": work.tap_us,
                        "tap": tap_record,
                        "null_reasons": tap_reasons,
                        "stdio_wait_us": work.stdio_wait_us,
                        "register_seq": registration,
                        "formal_grant_seq": formal_grant,
                        "by_pump": by_pump,
                        "t_input_done_us": work.tap_done_us,
                        "failure": work.failure,
                    }),
                    RecordShape::Admission { j } => json!({
                        "trial": trial,
                        "t_tick_us": tick,
                        "disposition": disposition,
                        "tap": admission_tap(*j, work.tap_done_us, &tap_record),
                        "floor_check": work.floor_frame.as_ref().and_then(|frame| frame.floor_check),
                        "native_start_us": work.floor_frame.as_ref().and_then(|frame| frame.native.iter().find_map(|span| span.start_us)),
                        "t_recv_us": work.floor_frame.as_ref().and_then(|frame| frame.received_us),
                        "stream_seq": work.floor_frame.as_ref().and_then(|frame| frame.stream_sequence),
                        "known_source_us": work.floor_frame.as_ref().and_then(|frame| frame.known_source_us),
                        "admission_wait_us": wait,
                        "floor_capture_us": work.floor_capture_us,
                        "content": admission_content,
                        "first_flip": work.first_flip,
                        "null_reasons": admission_reasons,
                        "failure": work.failure,
                    }),
                    RecordShape::Poll { after_sequence } => json!({
                        "trial": trial,
                        "t_tick_us": tick,
                        "disposition": disposition,
                        "after_sequence": after_sequence,
                        "sequence": work.floor_frame.as_ref().and_then(|frame| frame.stream_sequence),
                        "floor_capture_us": work.floor_capture_us,
                        "failure": work.failure,
                    }),
                }
            }
            DispatchOutcome::Contended { registration } => json!({
                "trial": trial,
                "t_tick_us": tick,
                "disposition": "skipped_busy",
                "contended": true,
                "register_seq": registration,
                "tap_us": null,
                "preview_state": match shape { RecordShape::Dispatch { preview_state, .. } => Some(preview_state.clone()), _ => None },
            }),
            DispatchOutcome::Refused { gate } => json!({
                "trial": trial,
                "t_tick_us": tick,
                "disposition": format!("skipped_gate:{}", gate.as_str()),
                "contended": false,
                "preview_state": match shape { RecordShape::Dispatch { preview_state, .. } => Some(preview_state.clone()), _ => None },
            }),
        };
        let kind_name = match shape {
            RecordShape::Dispatch { .. } => "dispatch",
            RecordShape::Admission { .. } => "admission",
            RecordShape::Poll { .. } => "poll",
        };
        let line = info.record(kind_name, &path, self.next_seq(), extra);
        self.journal.append(file, &line);
        if let DispatchOutcome::Granted { work, .. } = outcome
            && let Some(failure) = &work.failure
        {
            return self.on_failure(segment, instance, kind, failure);
        }
        false
    }

    /// The task modes (C6): tap a grid cell, then capture with a floor after the tap; or (S on a
    /// still picture) capture after the last delivered sequence, in a loop.
    #[allow(clippy::too_many_arguments)]
    fn task_activity(
        &self,
        segment: &SegmentShared,
        instance: usize,
        path: PathKind,
        poll: bool,
        grid: u32,
        quota: Option<u64>,
        seed: u64,
        file: &str,
        info: &SegmentInfo,
    ) {
        let owner = LockOwner::new(format!(
            "task/{}/{}",
            self.instance_name(instance),
            path.as_str()
        ));
        let mut rng = Rng::new(seed ^ 0x7a5c);
        let kind = path.backend();
        let mut check = 0u64;
        let flip = self
            .apk_enabled(instance)
            .then(|| (self.band_geometry(instance), ms(self.plan.apk.flip_wait_ms)));
        let last_sequence = Arc::new(Mutex::new(None::<u64>));
        while !segment.stopped() {
            if quota.is_some_and(|quota| check >= quota) || segment.backend_disabled(kind) {
                break;
            }
            check += 1;
            let cell_index = rng.below(u64::from(grid * grid)) as u32;
            let clock = Arc::clone(&self.clock);
            let display = self.display_facts(instance);
            let sequence = Arc::clone(&last_sequence);
            let after_sequence = *lock(&last_sequence);
            let work: crate::hub::FormalWork = Box::new(move |hub: &Hub, instance: usize| {
                if poll {
                    let floor = Floor {
                        after_us: None,
                        after_sequence: *lock(&sequence),
                    };
                    let started = clock.now_us();
                    let frame = formal_capture(hub, instance, kind, Some(floor));
                    let mut result = FormalResult {
                        floor_capture_us: Some(clock.now_us().saturating_sub(started)),
                        ..FormalResult::default()
                    };
                    match frame {
                        Ok(frame) => {
                            *lock(&sequence) = frame.stream_sequence;
                            result.floor_frame = Some(frame);
                        }
                        Err(failure) => result.failure = Some(failure),
                    }
                    result
                } else {
                    formal_work(
                        hub,
                        instance,
                        kind,
                        cell_index,
                        grid,
                        clock.as_ref(),
                        false,
                        display,
                        flip,
                    )
                }
            });
            let tick = self.now();
            let mut op = DispatchOp::new(
                instance,
                Arc::clone(&owner),
                Arc::clone(&self.clock),
                Some(work),
            );
            let outcome = run_blocking(&self.hub, &mut op);
            if matches!(outcome, DispatchOutcome::Granted { .. }) {
                segment.floor_checks.fetch_add(1, Ordering::AcqRel);
            }
            let shape = if poll {
                RecordShape::Poll { after_sequence }
            } else {
                RecordShape::Admission { j: cell_index }
            };
            if self.record_dispatch(
                segment, instance, kind, &outcome, check, tick, file, info, &shape,
            ) {
                break;
            }
            if !poll {
                let pause = 300 + rng.below(401);
                pause_until(self.clock.as_ref(), self.now() + ms(pause));
            }
        }
    }

    /// §3.11.2 B: the whole-run settlement.
    fn settle(&self, ran: &[String]) -> RunOutcome {
        // The last cell's settlement (§3.11.2 A, "最后一格结束以后进 B"): only step 5 is left
        // when nothing is open; the gate is set first all the same.
        let (t_stop_new, gate_seq) = self.stop_new();
        // Cells never started are untested with the stop reason.
        let stop_reason = self.terminal.first();
        for cell in &self.plan.cells {
            if ran.contains(&cell.id) {
                continue;
            }
            for (instance, path) in Self::planned_paths(cell, &self.plan) {
                self.record_cell(
                    &cell.id,
                    instance,
                    path,
                    CellResult::Untested(
                        stop_reason
                            .as_ref()
                            .map(|cause| format!("not started: {}", cause.label()))
                            .unwrap_or_else(|| "not started".to_owned()),
                    ),
                );
            }
        }
        // Anything still open closes now (every segment closes its own, so this is a guard).
        for instance in 0..self.hub.instance_count() {
            let mut identities = BTreeMap::new();
            self.close_segment_backends(instance, &mut identities, "run_end");
        }
        self.write_hub_notes();
        let results = self.results();
        let result = RunResult::decisive(&results);
        self.write_summary_inner(result, &results, t_stop_new, Some(gate_seq), None);
        self.journal.event(
            "terminal",
            json!({ "result": result.as_str(), "exit_code": result.exit_code(), "results": results }),
        );
        RunOutcome {
            result,
            results,
            exit_code: result.exit_code(),
        }
    }

    /// Every result that applies (§3.11.3).
    fn results(&self) -> Vec<RunResult> {
        let mut results = BTreeSet::new();
        for cell in lock(&self.cells).values() {
            for result in cell.values() {
                results.insert(result.run_result());
            }
        }
        for cause in self.terminal.all() {
            results.insert(cause.result());
        }
        if !lock(&self.unconfirmed).is_empty() {
            results.insert(RunResult::Unconfirmed);
        }
        if self.stdio_unrestored.load(Ordering::Acquire) {
            results.insert(RunResult::StdioUnrestored);
        }
        if self.safety_failed.load(Ordering::Acquire) {
            results.insert(RunResult::Failed);
        }
        if !lock(&self.rebooted).is_empty() {
            results.insert(RunResult::Incomplete);
        }
        let violations = self.hub.violations(&self.owner());
        if !violations.is_empty() {
            results.insert(RunResult::InternalInvariant);
        }
        if !self.journal.failures().is_empty() {
            results.insert(RunResult::InternalInvariant);
        }
        results.remove(&RunResult::Completed);
        if results.is_empty() {
            results.insert(RunResult::Completed);
        }
        results.into_iter().collect()
    }

    fn write_summary_inner(
        &self,
        result: RunResult,
        results: &[RunResult],
        t_stop_new: Micros,
        gate_seq: Option<u64>,
        forced: Option<&str>,
    ) {
        if self.summary_written.swap(true, Ordering::AcqRel) && forced.is_none() {
            // The supervisor wrote it already; the runner does not overwrite it.
            if self.terminal.finished_by_supervisor() {
                return;
            }
        }
        let checker = self
            .hub
            .snapshot(&self.owner(), |state| state.checker.clone());
        let summary = json!({
            "run_id": self.plan.run_id,
            "result": result.as_str(),
            "exit_code": result.exit_code(),
            "results": results.iter().map(|result| result.as_str()).collect::<Vec<_>>(),
            "forced_by": forced,
            "stop_cause": self.terminal.first(),
            "stop_causes": self.terminal.all(),
            "t_stop_new_us": t_stop_new,
            "run_stop_gate_seq": gate_seq,
            "started_unix_ms": self.started_unix_ms,
            "ended_unix_ms": self.clock.unix_ms(),
            "forced_stop": forced.is_some(),
            "cells": lock(&self.cells)
                .iter()
                .flat_map(|(cell, results)| {
                    results
                        .iter()
                        .map(|(key, result)| self.result_entry(cell, key, result))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
            "closes": *lock(&self.closes),
            "unconfirmed": *lock(&self.unconfirmed),
            "disabled": lock(&self.disabled).iter().map(|((instance, path), reason)| {
                json!({ "instance": self.instance_name(*instance), "path": path, "reason": reason })
            }).collect::<Vec<_>>(),
            "scrcpy_not_ready": lock(&self.scrcpy_not_ready).iter().map(|(instance, reason)| {
                json!({ "instance": self.instance_name(*instance), "reason": reason })
            }).collect::<Vec<_>>(),
            "invariants": {
                "violations": checker.violations,
                "lock_order_violations": self.hub.check.violations(),
                "grants": checker.grants,
                "publishes": checker.publishes,
                "discards": checker.discards,
                "inflight_before_register": checker.inflight_before_register,
            },
            "journal_failures": self.journal.failures(),
        });
        let _ = self.journal.write_json("summary.json", &summary);
        self.journal.flush();
    }

    /// The records of the run so far (tests).
    pub fn records(&self) -> Vec<crate::hub::SeqRecord> {
        self.records.records()
    }
}

/// #402 C2b: what the bench knows of an instance's display: the APK's view (INFO, with its
/// offset, in frame pixels), the APK's display size and rotation, and the size of the frames the
/// instance last delivered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DisplayFacts {
    view: Option<(u32, u32, u32, u32)>,
    display: Option<(u32, u32)>,
    rotation: Option<u64>,
    frame: Option<(u32, u32)>,
}

/// The rectangle `(x, y, width, height)` in frame pixels the APK's tap grid covers, and where its
/// size came from: the APK's view when it lies inside the frames the bench sees (`apk_view`);
/// else the frame of this dispatch (`dispatch_frame`), the last delivered frame (`last_frame`),
/// or the APK's display size (`apk_display`).
fn tap_rect(
    facts: &DisplayFacts,
    seen: Option<(u32, u32)>,
) -> Option<((u32, u32, u32, u32), &'static str)> {
    let frame = seen.or(facts.frame);
    if let Some((x, y, width, height)) = facts.view
        && width > 0
        && height > 0
        && frame.is_none_or(|(frame_width, frame_height)| {
            x + width <= frame_width && y + height <= frame_height
        })
    {
        return Some(((x, y, width, height), "apk_view"));
    }
    if let Some((width, height)) = seen {
        return Some(((0, 0, width, height), "dispatch_frame"));
    }
    if let Some((width, height)) = facts.frame {
        return Some(((0, 0, width, height), "last_frame"));
    }
    facts
        .display
        .map(|(width, height)| ((0, 0, width, height), "apk_display"))
}

/// The centre of tap-grid cell `cell_index` (column `cell % grid`, row `cell / grid`) of
/// `rect`, in frame pixels: inside the APK's cell j of PROTOCOL §4.2 for any size.
fn tap_point(rect: (u32, u32, u32, u32), cell_index: u32, grid: u32) -> (i32, i32) {
    let grid = u64::from(grid.max(1));
    let column = u64::from(cell_index) % grid;
    let row = u64::from(cell_index) / grid;
    let (left, top, width, height) = rect;
    let x = u64::from(left) + (2 * column + 1) * u64::from(width) / (2 * grid);
    let y = u64::from(top) + (2 * row + 1) * u64::from(height) / (2 * grid);
    (
        i32::try_from(x).unwrap_or(i32::MAX),
        i32::try_from(y).unwrap_or(i32::MAX),
    )
}

/// An admission record's `tap`: the grid cell, the input's end and the display it was mapped on.
fn admission_tap(j: u32, input_done: Option<Micros>, display: &Value) -> Value {
    let mut tap = json!({ "j": j, "t_input_done_us": input_done });
    if let (Value::Object(map), Value::Object(extra)) = (&mut tap, display.clone()) {
        map.extend(extra);
    }
    tap
}

/// The vendor stdio gate waits a frame's timing points reported.
fn stdio_waits(frame: &crate::backend::SampledFrame) -> impl Iterator<Item = Micros> + '_ {
    frame
        .native
        .iter()
        .filter(|span| span.kind == "stdio_wait")
        .filter_map(|span| span.end_us.zip(span.start_us))
        .map(|(end, start)| end.saturating_sub(start))
}

/// The flip number and grid cell a frame's timecode shows (flip mode), with the band geometry.
fn flip_shown(
    frame: &crate::backend::SampledFrame,
    geometry: BandGeometry,
) -> Option<crate::timecode::Payload> {
    crate::timecode::decode(
        &frame.pixels,
        frame.width,
        frame.height,
        bytes_per_pixel(frame.layout),
        geometry,
    )
    .payload
    .filter(|payload| payload.mode == 2)
}

/// P4's formal step: capture, tap a grid cell, capture with a floor after the tap. With `flip`
/// (C6 on the test pattern: the band geometry and the wait) a frame before the tap gives the
/// flip number n, and the first frame showing n + 1 is the tap's `first_flip`.
#[allow(clippy::too_many_arguments)]
fn formal_work(
    hub: &Hub,
    instance: usize,
    kind: BackendKind,
    cell_index: u32,
    grid: u32,
    clock: &dyn Clock,
    capture_first: bool,
    display: DisplayFacts,
    flip: Option<(BandGeometry, Micros)>,
) -> FormalResult {
    let mut result = FormalResult::default();
    // The size of a frame this dispatch took (frames are the display, unscaled).
    let mut seen: Option<(u32, u32)> = None;
    if capture_first {
        let started = clock.now_us();
        match formal_capture(hub, instance, kind, None) {
            Ok(frame) => {
                seen = Some((frame.width, frame.height));
                result.stdio_wait_us.extend(stdio_waits(&frame));
            }
            Err(failure) => {
                result.failure = Some(failure);
                return result;
            }
        }
        result.capture_us = Some(clock.now_us().saturating_sub(started));
    }
    let flip_before = flip.and_then(|(geometry, _)| {
        let frame = formal_capture(hub, instance, kind, None).ok()?;
        seen = Some((frame.width, frame.height));
        flip_shown(&frame, geometry).map(|payload| payload.event)
    });
    // The tap is mapped on the instance's actual display: the APK's view, or the frames' size;
    // without either, one frame is taken to learn it. Never a fixed size or orientation.
    let mut resolved = tap_rect(&display, seen);
    if resolved.is_none()
        && let Ok(frame) = formal_capture(hub, instance, kind, None)
    {
        resolved = tap_rect(&display, Some((frame.width, frame.height)));
    }
    let Some((rect, source)) = resolved else {
        result.failure = Some(
            BackendFailure::new(
                "tap_display_unknown",
                FailureClass::CellPath,
                "neither the APK nor a frame told the display size",
            )
            .without_native_call(),
        );
        return result;
    };
    let (x, y) = tap_point(rect, cell_index, grid);
    result.tap_display = Some(serde_json::json!({
        "display_w": rect.2,
        "display_h": rect.3,
        "view_x": rect.0,
        "view_y": rect.1,
        "rotation": display.rotation,
        "display_source": source,
        "x": x,
        "y": y,
    }));
    // The tap goes to the Nemu backend when it is open (the instance's touch path), else to S.
    let started = clock.now_us();
    let tapped = formal_tap(hub, instance, BackendKind::Nemu, x, y)
        .or_else(|_| formal_tap(hub, instance, BackendKind::Scrcpy, x, y));
    if let Err(failure) = tapped {
        result.failure = Some(failure);
        return result;
    }
    let done = clock.now_us();
    result.tap_us = Some(done.saturating_sub(started));
    result.tap_done_us = Some(done);
    let started = clock.now_us();
    let floor = Floor {
        after_us: Some(done),
        after_sequence: None,
    };
    match formal_capture(hub, instance, kind, Some(floor)) {
        Ok(frame) => {
            result.floor_capture_us = Some(clock.now_us().saturating_sub(started));
            result.stdio_wait_us.extend(stdio_waits(&frame));
            if let (Some((geometry, wait)), Some(before)) = (flip, flip_before) {
                let expected = (before + 1) & 0x00ff_ffff;
                let shows = |frame: &crate::backend::SampledFrame| {
                    flip_shown(frame, geometry).filter(|payload| payload.event == expected)
                };
                let first = |frame: &crate::backend::SampledFrame,
                             payload: crate::timecode::Payload,
                             floor_frame: bool| {
                    serde_json::json!({
                        "n": payload.event,
                        "j": payload.flip_j(),
                        "t_pub_us": frame.received_us.or(frame.built_us),
                        "t_capture_end_us": clock.now_us(),
                        "is_first_floor_frame": floor_frame,
                        "stream_seq": frame.stream_sequence,
                    })
                };
                if let Some(payload) = shows(&frame) {
                    result.first_flip = Some(first(&frame, payload, true));
                } else {
                    let deadline = clock.now_us().saturating_add(wait);
                    let mut after = frame.stream_sequence;
                    while clock.now_us() < deadline {
                        let next = formal_capture(
                            hub,
                            instance,
                            kind,
                            after.map(|sequence| Floor {
                                after_us: None,
                                after_sequence: Some(sequence),
                            }),
                        );
                        let Ok(next) = next else { break };
                        after = next.stream_sequence.or(after);
                        if let Some(payload) = shows(&next) {
                            result.first_flip = Some(first(&next, payload, false));
                            break;
                        }
                    }
                }
            }
            result.floor_frame = Some(frame);
        }
        Err(failure) => result.failure = Some(failure),
    }
    result
}

impl Emergency for RunShared {
    fn close_remaining(&self) -> Vec<Value> {
        let mut closed = Vec::new();
        let owner = LockOwner::new("supervisor-emergency");
        for instance in 0..self.hub.instance_count() {
            // Never wait for admission: a stuck call holds it, and that backend stays open
            // (unconfirmed).
            if !self.hub.admission[instance].try_acquire(&owner) {
                closed.push(json!({
                    "instance": self.instance_name(instance),
                    "skipped": "admission held by a call in flight",
                }));
                continue;
            }
            let (scrcpy, nemu) = {
                let mut slots = lock(&self.hub.backends[instance]);
                (
                    slots.take(BackendKind::Scrcpy),
                    slots.take(BackendKind::Nemu),
                )
            };
            self.hub.admission[instance].release(&owner);
            for backend in [scrcpy, nemu].into_iter().flatten() {
                let kind = backend.kind();
                let outcome = self.closer.close(
                    backend,
                    CloseInfo {
                        reason: "emergency".to_owned(),
                        instance: self.instance_name(instance).to_owned(),
                        generation: 0,
                        identity: None,
                    },
                );
                closed.push(json!({
                    "instance": self.instance_name(instance),
                    "backend": kind,
                    "outcome": outcome,
                    "device_side": "not checked (the forced-stop checklist covers it)",
                }));
            }
        }
        closed
    }

    fn write_summary(&self, result: RunResult, reason: &str) {
        let mut results = self.results();
        if !results.contains(&result) {
            results.push(result);
        }
        self.write_summary_inner(result, &results, self.now(), None, Some(reason));
    }
}

/// #402 C2b: the APK state of one instance (C3's protocol): the client, the record cursors and
/// epochs, the calibrations of the current clock epoch.
struct ApkInstance {
    client: Option<ApkClient>,
    streams: ApkStreams,
    calibrations: Vec<Calibration>,
    last_calibration_us: Option<Micros>,
    last_fetch_us: Option<Micros>,
    needs_calibration: bool,
}

impl ApkInstance {
    fn new() -> Self {
        Self {
            client: None,
            streams: ApkStreams::default(),
            calibrations: Vec::new(),
            last_calibration_us: None,
            last_fetch_us: None,
            needs_calibration: true,
        }
    }

    fn client(&mut self) -> Result<&mut ApkClient, ApkError> {
        self.client
            .as_mut()
            .ok_or_else(|| ApkError::new("apk_not_connected", "no APK connection"))
    }
}

/// #402 C2b: quotes one CSV field when it needs it.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// #402 C2b: the columns of `ocr-coverage.csv` (analysis schema §9).
const OCR_COLUMNS: [&str; 20] = [
    "image",
    "image_kind",
    "target",
    "target_class",
    "path",
    "bitrate",
    "orientation",
    "frame",
    "command",
    "package",
    "package_sha256",
    "facts",
    "frame_count",
    "result_json",
    "truth_hit",
    "truth_text",
    "result_hit",
    "result_text",
    "verdict",
    "verdict_reason",
];

/// #402 C2b: a slot name the APK accepts (`[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}`).
fn slot_name(image: &str) -> String {
    let mut slot: String = image
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if slot.starts_with('.') || slot.is_empty() {
        slot.insert(0, 's');
    }
    slot
}

fn bytes_per_pixel(layout: crate::backend::PixelLayout) -> usize {
    match layout {
        crate::backend::PixelLayout::Rgba8 => 4,
        crate::backend::PixelLayout::Rgb8 => 3,
    }
}

impl RunShared {
    /// #402 C2b: the calling thread's lock owner (concurrent segments each have their own, so
    /// the lock-order check sees each thread apart).
    fn owner(&self) -> Arc<LockOwner> {
        let thread = std::thread::current();
        let mut owners = lock(&self.owners);
        Arc::clone(owners.entry(thread.id()).or_insert_with(|| {
            LockOwner::new(format!("runner/{}", thread.name().unwrap_or("thread")))
        }))
    }

    fn apk_enabled(&self, instance: usize) -> bool {
        self.apk.is_some() && self.plan.instances[instance].apk
    }

    fn calib_file(&self) -> String {
        format!(
            "calib-{}.jsonl",
            self.plan.window.as_deref().unwrap_or("run")
        )
    }

    /// The band geometry of `instance` (INFO's layout, or the plan's cell).
    fn band_geometry(&self, instance: usize) -> BandGeometry {
        lock(&self.geometries)
            .get(&instance)
            .copied()
            .unwrap_or(BandGeometry {
                cell: self.plan.apk.cell,
                offset_x: 0,
                offset_y: 0,
            })
    }

    /// What the bench knows of the instance's display now (the taps are mapped on it).
    fn display_facts(&self, instance: usize) -> DisplayFacts {
        lock(&self.displays)
            .get(&instance)
            .copied()
            .unwrap_or_default()
    }

    /// The size of a frame the instance delivered (frames are the display, unscaled).
    fn note_frame(&self, instance: usize, width: u32, height: u32) {
        if width > 0 && height > 0 {
            lock(&self.displays).entry(instance).or_default().frame = Some((width, height));
        }
    }

    /// An APK INFO: the band geometry, the view (with its offset, in frame pixels), the display
    /// size and its rotation.
    fn apply_info(&self, instance: usize, info: &Value) {
        if let Some(geometry) = crate::apk::geometry_of_info(info) {
            lock(&self.geometries).insert(instance, geometry);
        }
        let number = |value: &Value| value.as_u64().and_then(|value| u32::try_from(value).ok());
        let mut displays = lock(&self.displays);
        let facts = displays.entry(instance).or_default();
        if let (Some(width), Some(height)) = (
            number(&info["view"]["width"]),
            number(&info["view"]["height"]),
        ) {
            facts.view = Some((
                number(&info["view"]["x"]).unwrap_or(0),
                number(&info["view"]["y"]).unwrap_or(0),
                width,
                height,
            ));
        }
        if let (Some(width), Some(height)) = (
            number(&info["display"]["real_width"]),
            number(&info["display"]["real_height"]),
        ) {
            facts.display = Some((width, height));
        }
        facts.rotation = info["display"]["rotation"].as_u64();
    }

    fn orientation_now(&self, instance: usize) -> &'static str {
        let orientation = lock(&self.orientations)
            .get(&instance)
            .copied()
            .unwrap_or(self.plan.instances[instance].orientation);
        orientation_code(orientation)
    }

    /// The timecode of a frame (`None` on an instance without the APK).
    fn decode_frame(
        &self,
        instance: usize,
        pixels: &[u8],
        width: u32,
        height: u32,
        layout: crate::backend::PixelLayout,
    ) -> Option<crate::timecode::Decoded> {
        if !self.apk_enabled(instance) {
            return None;
        }
        Some(crate::timecode::decode(
            pixels,
            width,
            height,
            bytes_per_pixel(layout),
            self.band_geometry(instance),
        ))
    }

    /// The `content` and its null reasons for a delivered frame (analysis schema §3.2).
    fn content_of(&self, decoded: Option<&crate::timecode::Decoded>) -> (Value, Value) {
        match decoded {
            Some(decoded) => {
                let mut reasons = serde_json::Map::new();
                if decoded.is_ok() {
                    reasons.insert(
                        "content.t_vsync_next_dev_ns".to_owned(),
                        json!("in the APK vsync record with counter + 1"),
                    );
                }
                (
                    crate::timecode::content_json(decoded),
                    Value::Object(reasons),
                )
            }
            None => (
                Value::Null,
                json!({ "content": "no test-pattern APK on this instance" }),
            ),
        }
    }

    /// Runs `call` on the instance's APK (entry E5 or E6, P6: no bench lock during the call),
    /// connecting first. A HELLO that shows another APK process starts a new APK epoch (new
    /// record file, cursors from 0); a device time away from the calibration's prediction starts
    /// a new clock epoch; both need a new calibration. An error that ends the connection drops
    /// the client, so the next call reconnects.
    fn apk_call<T>(
        &self,
        instance: usize,
        entry: Entry,
        call: impl FnOnce(&RunShared, &mut ApkInstance) -> Result<T, ApkError>,
    ) -> Result<T, String> {
        let Some(connector) = self.apk.clone() else {
            return Err("apk_not_configured: the run has no APK connector".to_owned());
        };
        if !self.plan.instances[instance].apk {
            return Err(format!(
                "apk_not_configured: {} has no APK",
                self.instance_name(instance)
            ));
        }
        let name = self.instance_name(instance).to_owned();
        let owner = self.owner();
        let result = aux_call(
            &self.hub,
            &owner,
            instance,
            entry,
            false,
            || -> Result<T, ApkError> {
                let mut state = lock(&self.apk_states[instance]);
                // Every call starts with HELLO on the live connection, so a new APK process
                // (its connection is gone) or a reset device clock is seen before any record or
                // device time is used; a dead connection is replaced once.
                let alive = match state.client.as_mut().map(ApkClient::hello) {
                    Some(Ok(hello)) => Some(hello),
                    Some(Err(_)) => {
                        if let Some(client) = state.client.take() {
                            let _ = client.close();
                        }
                        None
                    }
                    None => None,
                };
                let (hello, connected) = match alive {
                    Some(hello) => (hello, false),
                    None => {
                        let link = connector.connect(&name)?;
                        let mut client = ApkClient::new(link, Arc::clone(&self.clock));
                        let hello = client.hello()?;
                        if let Ok(info) = client.info() {
                            self.apply_info(instance, &info);
                        }
                        state.client = Some(client);
                        (hello, true)
                    }
                };
                let host_us = self.now();
                let last = state.calibrations.last().copied();
                let change = state.streams.on_hello(&hello, host_us, last.as_ref());
                if change.apk_restarted {
                    self.journal.event(
                        "apk_restart",
                        json!({ "instance": name, "apk_instance": hello.instance, "apk_epoch": state.streams.epoch }),
                    );
                }
                if change.clock_reset {
                    state.calibrations.clear();
                    self.journal.event(
                        "device_clock_reset",
                        json!({ "instance": name, "clock_epoch": state.streams.clock_epoch, "t_now_dev_ns": hello.t_now_dev_ns }),
                    );
                }
                if change.apk_restarted || change.clock_reset {
                    state.needs_calibration = true;
                }
                if connected {
                    self.journal.event(
                        "apk_hello",
                        json!({ "instance": name, "hello": hello.raw, "marks": state.streams.marks() }),
                    );
                }
                let outcome = call(self, &mut state);
                if let Err(error) = &outcome
                    && error.closes_connection()
                    && let Some(client) = state.client.take()
                {
                    let _ = client.close();
                }
                outcome
            },
        );
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                self.journal.event(
                    "apk_error",
                    json!({ "instance": name, "entry": entry, "error": error }),
                );
                Err(format!("{}: {}", error.code, error.detail))
            }
            Err(gate) => Err(format!("skipped_gate:{}", gate.as_str())),
        }
    }

    /// Closes every APK connection (the bench's own forwards are removed).
    fn apk_disconnect_all(&self) {
        for (instance, state) in self.apk_states.iter().enumerate() {
            let client = lock(state).client.take();
            if let Some(client) = client
                && let Err(detail) = client.close()
            {
                self.journal.event(
                    "apk_close_failed",
                    json!({ "instance": self.instance_name(instance), "detail": detail }),
                );
            }
        }
    }

    /// One calibration (plan §5.1): `pings` PINGs in sequence; every round trip, the result and
    /// the drift go to `calib-<window>.jsonl`.
    fn calibrate(&self, instance: usize, source: &str, pings: u32) -> Result<Calibration, String> {
        let file = self.calib_file();
        let name = self.instance_name(instance).to_owned();
        let calibrated = self.apk_call(instance, Entry::Calibration, |run, state| {
            let mut samples = Vec::with_capacity(pings as usize);
            {
                let client = state.client()?;
                for _ in 0..pings.max(1) {
                    samples.push(client.ping()?);
                }
            }
            let calibration = crate::apk::calibrate(&samples)
                .ok_or_else(|| ApkError::new("apk_no_samples", "no PING sample"))?;
            let marks = state.streams.marks();
            for sample in &samples {
                run.journal.append(
                    &file,
                    &crate::apk::marked(
                        &json!({
                            "type": "roundtrip",
                            "instance": name,
                            "source": source,
                            "sent_us": sample.sent_us,
                            "received_us": sample.received_us,
                            "t_rx_dev_ns": sample.t_rx_dev_ns,
                            "t_tx_dev_ns": sample.t_tx_dev_ns,
                            "rtt_us": sample.rtt_us(),
                        }),
                        &marks,
                    ),
                );
            }
            let (residual, drift) =
                crate::apk::residual_and_drift(&state.calibrations, &calibration);
            run.journal.append(
                &file,
                &crate::apk::marked(
                    &json!({
                        "type": "calib",
                        "instance": name,
                        "source": source,
                        "t_us": run.now(),
                        "t_dev_us": calibration.t_dev_us,
                        "t_host_us": calibration.t_host_us,
                        "theta_us": calibration.theta_us,
                        "u_us": calibration.u_us,
                        "residual_us": residual,
                        "rtt_min_us": calibration.rtt_min_us,
                        "n": calibration.samples,
                    }),
                    &marks,
                ),
            );
            if let Some(drift) = drift {
                run.journal.append(
                    &file,
                    &crate::apk::marked(
                        &json!({ "type": "drift", "instance": name, "source": source, "ppm": drift }),
                        &marks,
                    ),
                );
            }
            state.calibrations.push(calibration);
            state.last_calibration_us = Some(run.now());
            state.needs_calibration = false;
            Ok(calibration)
        });
        if let Err(detail) = &calibrated {
            self.journal.event(
                "calibration_failed",
                json!({ "instance": name, "source": source, "detail": detail }),
            );
        }
        calibrated
    }

    /// Connects and calibrates `instance` when it has no calibration of its current APK
    /// process and clock (the device time restarts with the instance).
    fn ensure_calibrated(&self, instance: usize, source: &str) {
        if !self.apk_enabled(instance) {
            return;
        }
        let needed = self.apk_call(instance, Entry::Calibration, |_, state| {
            Ok(state.needs_calibration || state.calibrations.is_empty())
        });
        if needed == Ok(true) {
            let _ = self.calibrate(instance, source, self.plan.apk.pings);
        }
    }

    /// The periodic work of a segment's controller: recalibrate every `recalibrate_ms`, fetch
    /// the records every `records_period_ms`.
    fn apk_periodic(&self, instance: usize) {
        if !self.apk_enabled(instance) {
            return;
        }
        let now = self.now();
        let (calibrate, fetch) = {
            let state = lock(&self.apk_states[instance]);
            (
                state.client.is_some()
                    && state.last_calibration_us.is_none_or(|at| {
                        now.saturating_sub(at) >= ms(self.plan.apk.recalibrate_ms)
                    }),
                state.client.is_some()
                    && state.last_fetch_us.is_none_or(|at| {
                        now.saturating_sub(at) >= ms(self.plan.apk.records_period_ms)
                    }),
            )
        };
        if calibrate {
            let _ = self.calibrate(instance, "periodic", self.plan.apk.pings);
        }
        if fetch {
            self.apk_fetch_records(instance);
        }
    }

    /// Pages both record streams to their ends: `flip`, `step` and `vsync` records to the APK
    /// file of the current APK process, the others to `apkraw-<instance>.jsonl`.
    fn apk_fetch_records(&self, instance: usize) {
        if !self.apk_enabled(instance) {
            return;
        }
        let name = self.instance_name(instance).to_owned();
        let fetched = self.apk_call(instance, Entry::ApkControl, |run, state| {
            let mut written = 0u64;
            for stream in ["vsync", "event"] {
                loop {
                    let after = if stream == "vsync" {
                        state.streams.vsync_after
                    } else {
                        state.streams.event_after
                    };
                    let page = state.client()?.records(stream, after, 5_000)?;
                    let marks = state.streams.marks();
                    if page.lost > 0 {
                        run.journal.event(
                            "apk_records_lost",
                            json!({ "instance": name, "stream": stream, "lost": page.lost, "marks": marks }),
                        );
                    }
                    let file = crate::apk::apk_file_name(&name, state.streams.epoch);
                    let raw = crate::apk::apk_raw_file_name(&name);
                    for record in &page.records {
                        let line = crate::apk::marked(record, &marks);
                        if crate::apk::analysis_record(record) {
                            run.journal.append(&file, &line);
                        } else {
                            run.journal.append(&raw, &line);
                        }
                        written += 1;
                    }
                    if stream == "vsync" {
                        state.streams.vsync_after = page.next;
                    } else {
                        state.streams.event_after = page.next;
                    }
                    if page.records.is_empty() || page.next >= page.newest {
                        break;
                    }
                }
            }
            state.last_fetch_us = Some(run.now());
            Ok(written)
        });
        if let Err(detail) = fetched {
            self.journal.event(
                "apk_records_failed",
                json!({ "instance": name, "detail": detail }),
            );
        }
    }

    /// `MODE …`, then INFO until the APK shows the mode (it changes at the next vsync).
    fn apk_set_mode(
        &self,
        instance: usize,
        mode: &str,
        options: Vec<(&'static str, String)>,
    ) -> Result<Value, String> {
        let wait = ms(self.plan.apk.mode_wait_ms);
        let set = self.apk_call(instance, Entry::ApkControl, |run, state| {
            let client = state.client()?;
            let response = client.mode(mode, &options)?;
            let deadline = run.now().saturating_add(wait);
            loop {
                let info = client.info()?;
                if info["mode"] == json!(mode) && info["mode_pending"].is_null() {
                    run.apply_info(instance, &info);
                    return Ok(response);
                }
                if run.now() >= deadline {
                    return Err(ApkError::new(
                        "apk_mode_not_shown",
                        format!("the APK did not show mode {mode} within the wait"),
                    ));
                }
                pause_until(run.clock.as_ref(), run.now() + ms(20));
            }
        });
        self.journal.event(
            "apk_mode",
            json!({ "instance": self.instance_name(instance), "mode": mode, "options": options, "ok": set.is_ok(), "detail": set.as_ref().err() }),
        );
        set
    }

    /// The segment's APK set-up: connection and calibration, then its mode.
    fn apk_segment_start(&self, instance: usize, segment: &SegmentPlan, source: &str) {
        if !self.apk_enabled(instance) {
            return;
        }
        self.ensure_calibrated(instance, source);
        if let Some(mode) = &segment.apk_mode {
            let options = if mode == "step" {
                vec![("level", "0".to_owned())]
            } else {
                Vec::new()
            };
            let _ = self.apk_set_mode(instance, mode, options);
        }
    }

    /// The events the hub noted (entry × gate decisions, ended graces), and the invariant and
    /// lock-order violations not written yet; a new violation stops the run (§3.11.2 B).
    fn write_hub_notes(&self) {
        let notes = self.hub.take_notes();
        for ((instance, entry, gate, reaction, admitted), note) in notes.entry_gates {
            self.journal.event(
                "entry_gate",
                json!({
                    "instance": self.instance_name(instance),
                    "entry": crate::gates::entry_code(entry),
                    "entry_name": entry,
                    "gate": gate,
                    "reaction": reaction,
                    "admitted": admitted,
                    "count": note.count,
                    "t_first_us": note.t_first_us,
                }),
            );
        }
        for grace in notes.graces {
            self.journal.event(
                "grace",
                json!({
                    "instance": self.instance_name(grace.instance),
                    "t_enter_us": grace.t_enter_us,
                    "t_exit_us": grace.t_exit_us,
                    "planned_ms": grace.planned_ms,
                    "exit": grace.exit,
                }),
            );
        }
        let owner = self.owner();
        let lock_violations = self.hub.check.violations();
        let invariant_violations = self
            .hub
            .snapshot(&owner, |state| state.checker.violations.clone());
        let mut written = lock(&self.violations_written);
        let mut new = false;
        for violation in lock_violations.iter().skip(written.0) {
            new = true;
            self.journal
                .event("lock_order_violation", json!({ "detail": violation }));
        }
        for violation in invariant_violations.iter().skip(written.1) {
            new = true;
            let which = violation
                .split(':')
                .next()
                .unwrap_or("invariant")
                .trim()
                .to_owned();
            self.journal.event(
                "invariant_violation",
                json!({ "which": which, "detail": violation }),
            );
        }
        *written = (lock_violations.len(), invariant_violations.len());
        drop(written);
        if new {
            self.terminal.request(
                self.now(),
                StopCause::InternalInvariant("a lock-order or invariant violation".to_owned()),
            );
        }
    }

    /// The per-cell readings (§5.5, §2.2): the stdio capture files (`stdio_size`, in
    /// `host-<cell>.jsonl`) and the `VMMHeadless` handle counts (`vmm_handles`, in
    /// `events.jsonl`), at the cell's start and end.
    fn cell_probe(&self, cell: &str, phase: &str) {
        let file = format!("host-{cell}.jsonl");
        let line = match self.probes.stdio_capture() {
            Ok((bytes, files)) => json!({
                "type": "stdio_size",
                "t_us": self.now(),
                "cell": cell,
                "instance": null,
                "scope": "process",
                "phase": phase,
                "bytes": bytes,
                "files": files,
                "null_reasons": { "instance": "the vendor stdio redirect is process-wide (§1.2)" },
            }),
            Err(detail) => json!({
                "type": "stdio_size",
                "t_us": self.now(),
                "cell": cell,
                "instance": null,
                "scope": "process",
                "phase": phase,
                "bytes": null,
                "files": null,
                "null_reasons": { "instance": "the vendor stdio redirect is process-wide (§1.2)", "bytes": detail },
            }),
        };
        self.journal.append(&file, &line);
        match self.probes.vmm_handles() {
            Ok(processes) => self.journal.event(
                "vmm_handles",
                json!({ "cell": cell, "phase": phase, "processes": processes }),
            ),
            Err(detail) => self.journal.event(
                "vmm_handles",
                json!({ "cell": cell, "phase": phase, "processes": null, "null_reasons": { "processes": detail } }),
            ),
        }
    }

    /// One `artifact_delta` line (analysis schema §5, `host-<cell>.jsonl`): the material bytes
    /// and events a segment's path wrote (B_model's pipeline; N and S write none) and its state
    /// changes, each of which is one bounded diagnostic.
    fn write_artifact_delta(
        &self,
        info: &SegmentInfo,
        path: PathKind,
        shared: &SegmentShared,
        state_changes: u64,
    ) {
        let material_events = shared.material_events.load(Ordering::Acquire);
        self.journal.append(
            &format!("host-{}.jsonl", info.cell),
            &json!({
                "type": "artifact_delta",
                "t_us": self.now(),
                "cell": info.cell,
                "path": path.as_str(),
                "orientation": info.orientation,
                "round": info.round,
                "segment": info.name,
                "frames": shared.delivered.load(Ordering::Acquire),
                "bytes": shared.material_bytes.load(Ordering::Acquire),
                "events": material_events + state_changes,
                "state_changes": state_changes,
            }),
        );
    }

    /// The modes that drive the APK or a lifecycle step (C1, K, C8, C10).
    fn run_procedure(
        self: &Arc<Self>,
        cell: &CellPlan,
        segment: &SegmentPlan,
        instance: usize,
        info: &SegmentInfo,
        cell_deadline: Micros,
        rng: &mut Rng,
    ) -> SegmentEnd {
        let reps = segment.reps.unwrap_or(1).max(1);
        if segment.mode == SegmentMode::Calibrate {
            let pings = segment
                .quota
                .and_then(|quota| u32::try_from(quota).ok())
                .unwrap_or(self.plan.apk.pings);
            for _ in 0..reps {
                self.check_time();
                if self.whole_stop_requested() {
                    return SegmentEnd::Whole;
                }
                if self.now() >= cell_deadline {
                    return SegmentEnd::CellLimit;
                }
                if let Err(detail) = self.calibrate(instance, &cell.id, pings)
                    && !detail.starts_with("skipped_gate:")
                {
                    return SegmentEnd::Fatal("calibration_failed".to_owned());
                }
            }
            return SegmentEnd::Finished;
        }
        let Some(path) = segment.path else {
            return SegmentEnd::Finished;
        };
        let kind = path.backend();
        let options = OpenOptions {
            scrcpy_bit_rate: segment.scrcpy.as_ref().and_then(|scrcpy| scrcpy.bit_rate),
            scrcpy_max_fps: segment.scrcpy.as_ref().and_then(|scrcpy| scrcpy.max_fps),
            scrcpy_encoder: segment
                .scrcpy
                .as_ref()
                .and_then(|scrcpy| scrcpy.encoder.clone()),
        };
        self.apk_segment_start(instance, segment, &cell.id);
        let mut identities: BTreeMap<BackendKind, IdentityTuple> = BTreeMap::new();
        let mut reopens = 0;
        match self.open_backend(instance, kind, &options, &mut reopens, false) {
            Ok((backend, identity)) => {
                if let Some(identity) = identity {
                    self.sampling_start(instance, &identity);
                    identities.insert(kind, identity);
                }
                self.put_backend(instance, backend);
            }
            Err(end) => return end,
        }
        let end = match segment.mode {
            SegmentMode::StepTrials => {
                self.step_trials(cell, instance, kind, reps, info, cell_deadline, rng)
            }
            SegmentMode::Rotate => self.rotations(
                instance,
                kind,
                reps,
                &options,
                &mut identities,
                cell_deadline,
            ),
            SegmentMode::Reconnect => self.reconnects(
                instance,
                path,
                reps,
                &options,
                &mut identities,
                cell_deadline,
            ),
            SegmentMode::StaticImages => self.static_images(cell, segment, instance, path, info),
            _ => SegmentEnd::Finished,
        };
        self.apk_fetch_records(instance);
        let confirmed = self.close_segment_backends(instance, &mut identities, "segment_end");
        self.write_hub_notes();
        if !confirmed && end != SegmentEnd::Whole {
            return SegmentEnd::Whole;
        }
        end
    }

    /// `sampling_start` (analysis schema §8): the stream's identity is confirmed and its first
    /// sample may be taken.
    fn sampling_start(&self, instance: usize, identity: &IdentityTuple) {
        self.journal.event(
            "sampling_start",
            json!({ "instance": self.instance_name(instance), "scid": format!("{:08x}", identity.scid) }),
        );
    }

    /// C1/K step trials (plan §5.3): STEP k, then the first S frame whose timecode shows step k;
    /// its arrival is a `step_arrival` record of the calibration file. `T_vsync(k)` is the APK's
    /// `step` record.
    #[allow(clippy::too_many_arguments)]
    fn step_trials(
        &self,
        cell: &CellPlan,
        instance: usize,
        kind: BackendKind,
        reps: u32,
        info: &SegmentInfo,
        cell_deadline: Micros,
        rng: &mut Rng,
    ) -> SegmentEnd {
        let name = self.instance_name(instance).to_owned();
        let file = self.calib_file();
        if let Err(detail) = self.apk_set_mode(instance, "step", vec![("level", "0".to_owned())]) {
            self.journal.event(
                "step_trials_refused",
                json!({ "instance": name, "segment": info.label, "detail": detail }),
            );
            return SegmentEnd::Fatal("apk_step_mode".to_owned());
        }
        for trial in 1..=reps {
            self.check_time();
            if self.whole_stop_requested() {
                return SegmentEnd::Whole;
            }
            if self.now() >= cell_deadline {
                return SegmentEnd::CellLimit;
            }
            let sent = self.now();
            let stepped = self.apk_call(instance, Entry::ApkControl, |_, state| {
                state.client()?.step(None)
            });
            let k = match stepped {
                Ok(k) => k,
                Err(detail) => {
                    self.journal.event(
                        "step_failed",
                        json!({ "instance": name, "trial": trial, "detail": detail }),
                    );
                    return SegmentEnd::Fatal("apk_step".to_owned());
                }
            };
            let deadline = self.now().saturating_add(ms(self.plan.apk.step_wait_ms));
            let mut arrival: Option<(Option<Micros>, Option<u64>, u8)> = None;
            let mut last_sequence = None;
            while self.now() < deadline {
                let sampled = self.with_backend(instance, kind, |backend| {
                    backend.sample(last_sequence.map(|after| Floor {
                        after_us: None,
                        after_sequence: Some(after),
                    }))
                });
                match sampled {
                    Ok(frame) => {
                        last_sequence = frame.stream_sequence.or(last_sequence);
                        let decoded = self.decode_frame(
                            instance,
                            &frame.pixels,
                            frame.width,
                            frame.height,
                            frame.layout,
                        );
                        if let Some(payload) = decoded.and_then(|decoded| decoded.payload)
                            && payload.mode == 3
                            && u64::from(payload.event) == k & 0x00ff_ffff
                        {
                            arrival =
                                Some((frame.received_us, frame.stream_sequence, payload.level()));
                            break;
                        }
                    }
                    Err(failure) if failure.class == FailureClass::Measured => {}
                    Err(failure) => {
                        self.journal.event(
                            "step_trial_capture_failed",
                            json!({ "instance": name, "trial": trial, "failure": failure }),
                        );
                        return SegmentEnd::Fatal(failure.code);
                    }
                }
                pause_until(self.clock.as_ref(), self.now() + ms(5));
            }
            let mut line = json!({
                "type": "step_arrival",
                "instance": name,
                "k": k,
                "source": cell.id,
                "trial": trial,
                "t_step_sent_us": sent,
                "t_recv_us": arrival.and_then(|(received, _, _)| received),
                "stream_seq": arrival.and_then(|(_, sequence, _)| sequence),
                "level": arrival.map(|(_, _, level)| level),
                "null_reasons": if arrival.is_some() { json!({}) } else { json!({ "t_recv_us": "no S frame showed the step within the wait" }) },
            });
            let marks = lock(&self.apk_states[instance]).streams.marks();
            line = crate::apk::marked(&line, &marks);
            self.journal.append(&file, &line);
            let pause = 300 + rng.below(401);
            pause_until(self.clock.as_ref(), self.now() + ms(pause));
        }
        SegmentEnd::Finished
    }

    /// C10 item 11: each rotation sets the in-cell gate, drains, closes the backend and confirms
    /// it, lifts the gate, asks the APK for the other orientation, waits for the new geometry,
    /// moves the generation and reopens.
    fn rotations(
        &self,
        instance: usize,
        kind: BackendKind,
        reps: u32,
        options: &OpenOptions,
        identities: &mut BTreeMap<BackendKind, IdentityTuple>,
        cell_deadline: Micros,
    ) -> SegmentEnd {
        let name = self.instance_name(instance).to_owned();
        let owner = self.owner();
        for rep in 1..=reps {
            self.check_time();
            if self.whole_stop_requested() {
                return SegmentEnd::Whole;
            }
            if self.now() >= cell_deadline {
                return SegmentEnd::CellLimit;
            }
            let facts = self.display_facts(instance);
            let portrait_now = facts.display.or(facts.frame).map_or(
                self.plan.instances[instance].orientation == crate::plan::Orientation::Portrait,
                |(width, height)| height > width,
            );
            let target = if portrait_now {
                crate::plan::Orientation::Landscape
            } else {
                crate::plan::Orientation::Portrait
            };
            let target_word = match target {
                crate::plan::Orientation::Landscape => "landscape",
                crate::plan::Orientation::Portrait => "portrait",
            };
            let gate_seq = self
                .hub
                .set_gate(&owner, instance, GateKind::StopClass, true);
            let drain_deadline = self.now() + self.limits.nemu_close_deadline_us;
            while self.hub.inflight(&owner, instance) > 0 && self.now() < drain_deadline {
                pause_until(self.clock.as_ref(), self.now() + ms(10));
            }
            let generation_before = self
                .hub
                .snapshot(&owner, |state| state.instances[instance].generation);
            let confirmed = self.close_segment_backends(instance, identities, "rotation");
            self.hub
                .set_gate(&owner, instance, GateKind::StopClass, false);
            if !confirmed {
                return SegmentEnd::Whole;
            }
            let t_orient_us = self.now();
            let wait = ms(self.plan.apk.mode_wait_ms);
            let rotated = self.apk_call(instance, Entry::ApkControl, |run, state| {
                let client = state.client()?;
                client.orient(target_word)?;
                let deadline = run.now().saturating_add(wait);
                loop {
                    let info = client.info()?;
                    let width = info["display"]["real_width"].as_u64().unwrap_or(0);
                    let height = info["display"]["real_height"].as_u64().unwrap_or(0);
                    let portrait = height > width;
                    if width > 0 && portrait == (target == crate::plan::Orientation::Portrait) {
                        run.apply_info(instance, &info);
                        return Ok((width, height));
                    }
                    if run.now() >= deadline {
                        return Err(ApkError::new(
                            "apk_rotation_not_shown",
                            format!("the display did not turn {target_word} within the wait"),
                        ));
                    }
                    pause_until(run.clock.as_ref(), run.now() + ms(20));
                }
            });
            let t_stable_us = self.now();
            let display = match rotated {
                Ok(display) => display,
                Err(detail) => {
                    self.journal.event(
                        "rotation",
                        json!({ "instance": name, "rep": rep, "to": target_word, "ok": false, "detail": detail }),
                    );
                    return SegmentEnd::Fatal("rotation_failed".to_owned());
                }
            };
            lock(&self.orientations).insert(instance, target);
            lock(&self.displays).entry(instance).or_default().frame = None;
            let generation_after = self.hub.bump_generation(&owner, instance);
            let mut reopens = 0;
            let reopened = self.open_backend(instance, kind, options, &mut reopens, false);
            let t_reopen_us = self.now();
            match reopened {
                Ok((backend, identity)) => {
                    if let Some(identity) = identity {
                        self.sampling_start(instance, &identity);
                        identities.insert(kind, identity);
                    }
                    self.put_backend(instance, backend);
                }
                Err(end) => return end,
            }
            let first = self.with_backend(instance, kind, |backend| backend.sample(None));
            self.journal.event(
                "rotation",
                json!({
                    "instance": name,
                    "rep": rep,
                    "to": target_word,
                    "ok": first.is_ok(),
                    "gate_seq": gate_seq,
                    "generation_before": generation_before,
                    "generation_after": generation_after,
                    "close_confirmed": confirmed,
                    "display": [display.0, display.1],
                    "frame": first.as_ref().ok().map(|frame| [frame.width, frame.height]),
                    "failure": first.as_ref().err(),
                    "t_orient_us": t_orient_us,
                    "t_stable_us": t_stable_us,
                    "t_reopen_us": t_reopen_us,
                }),
            );
            if let Err(failure) = first
                && failure.class != FailureClass::Measured
            {
                return SegmentEnd::Fatal(failure.code);
            }
        }
        SegmentEnd::Finished
    }

    /// C10 item 10: N closes and reopens in-process; S stops this stream's server by its
    /// identity (only when the identity tuple still matches), waits for the stream to end,
    /// closes and confirms, and reopens with a new scid. Old frames never count as new: the
    /// generation moves and the new stream's sequence starts at 1.
    fn reconnects(
        &self,
        instance: usize,
        path: PathKind,
        reps: u32,
        options: &OpenOptions,
        identities: &mut BTreeMap<BackendKind, IdentityTuple>,
        cell_deadline: Micros,
    ) -> SegmentEnd {
        let name = self.instance_name(instance).to_owned();
        let owner = self.owner();
        let kind = path.backend();
        for rep in 1..=reps {
            self.check_time();
            if self.whole_stop_requested() {
                return SegmentEnd::Whole;
            }
            if self.now() >= cell_deadline {
                return SegmentEnd::CellLimit;
            }
            let generation_before = self
                .hub
                .snapshot(&owner, |state| state.instances[instance].generation);
            let before = self.with_backend(instance, kind, |backend| backend.sample(None));
            let sequence_before = before.as_ref().ok().and_then(|frame| frame.stream_sequence);
            let old_scid = identities.get(&kind).map(|identity| identity.scid);
            let mut stopped_pid = None;
            let mut stream_end_seen = None;
            if kind == BackendKind::Scrcpy {
                let Some(identity) = identities.get(&kind).cloned() else {
                    self.unconfirmed(format!("{name} scrcpy reconnect without an identity"));
                    return SegmentEnd::Whole;
                };
                let stopped = aux_call(
                    &self.hub,
                    &owner,
                    instance,
                    Entry::DeviceSideCheck,
                    false,
                    || self.device_check.stop_server(&name, &identity),
                );
                match stopped {
                    Ok(Ok(pid)) => stopped_pid = Some(pid),
                    Ok(Err(failure)) => {
                        self.journal.event(
                            "reconnect",
                            json!({ "instance": name, "path": path, "rep": rep, "ok": false, "stop_failure": failure }),
                        );
                        return SegmentEnd::Fatal("reconnect_stop_server".to_owned());
                    }
                    Err(gate) => {
                        self.journal.event(
                            "reconnect",
                            json!({ "instance": name, "path": path, "rep": rep, "ok": false, "refused_by": gate }),
                        );
                        return SegmentEnd::Fatal("reconnect_refused".to_owned());
                    }
                }
                let deadline = self.now() + ms(5_000);
                while self.now() < deadline {
                    match self.with_backend(instance, kind, |backend| backend.sample(None)) {
                        Err(failure) if failure.code.starts_with("scrcpy_stream_ended") => {
                            stream_end_seen = Some(self.now());
                            break;
                        }
                        _ => pause_until(self.clock.as_ref(), self.now() + ms(50)),
                    }
                }
            }
            let confirmed = self.close_segment_backends(instance, identities, "reconnect");
            if !confirmed {
                return SegmentEnd::Whole;
            }
            let generation_after = self.hub.bump_generation(&owner, instance);
            let mut reopens = 0;
            match self.open_backend(instance, kind, options, &mut reopens, false) {
                Ok((backend, identity)) => {
                    if let Some(identity) = identity {
                        self.sampling_start(instance, &identity);
                        identities.insert(kind, identity);
                    }
                    self.put_backend(instance, backend);
                }
                Err(end) => return end,
            }
            let after = self.with_backend(instance, kind, |backend| backend.sample(None));
            self.journal.event(
                "reconnect",
                json!({
                    "instance": name,
                    "path": path,
                    "rep": rep,
                    "ok": after.is_ok(),
                    "old_scid": old_scid.map(|scid| format!("{scid:08x}")),
                    "new_scid": identities.get(&kind).map(|identity| format!("{:08x}", identity.scid)),
                    "stopped_pid": stopped_pid,
                    "t_stream_end_us": stream_end_seen,
                    "close_confirmed": confirmed,
                    "generation_before": generation_before,
                    "generation_after": generation_after,
                    "sequence_before": sequence_before,
                    "first_sequence_after": after.as_ref().ok().and_then(|frame| frame.stream_sequence),
                    "failure": after.as_ref().err(),
                }),
            );
            if let Err(failure) = after
                && failure.class != FailureClass::Measured
            {
                return SegmentEnd::Fatal(failure.code);
            }
        }
        SegmentEnd::Finished
    }

    /// C8 (plan §5.8): each picture of `truth.json` goes to the APK (PUT, static mode), then
    /// `frames_per_image` frames of the segment's path are saved as PNG under `c8/`, each with a
    /// `c8_frame` record; `ocr-coverage.csv` gets one row per picture × target × path × frame
    /// (and the source row), with the offline `actinglab` command to run and the verdict
    /// `not_evaluated` until the coordinator runs it.
    fn static_images(
        &self,
        cell: &CellPlan,
        segment: &SegmentPlan,
        instance: usize,
        path: PathKind,
        info: &SegmentInfo,
    ) -> SegmentEnd {
        let name = self.instance_name(instance).to_owned();
        let Some(c8) = self.plan.c8.clone() else {
            return SegmentEnd::Fatal("c8_not_planned".to_owned());
        };
        let truth: Value = match std::fs::read_to_string(&c8.truth)
            .map_err(|error| error.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string()))
        {
            Ok(truth) => truth,
            Err(detail) => {
                self.journal.event(
                    "c8_truth_unreadable",
                    json!({ "truth": c8.truth, "detail": detail }),
                );
                return SegmentEnd::Fatal("c8_truth_unreadable".to_owned());
            }
        };
        let package = truth["package"].as_str().unwrap_or_default().to_owned();
        let package_sha = truth["package_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let facts = truth["facts"].as_str().unwrap_or_default().to_owned();
        let actinglab = c8
            .actinglab
            .clone()
            .unwrap_or_else(|| "actinglab".to_owned());
        let bitrate = if path == PathKind::Scrcpy {
            match segment.scrcpy.as_ref().and_then(|scrcpy| scrcpy.bit_rate) {
                Some(rate) if rate <= 4_000_000 => "4M".to_owned(),
                _ => "8M".to_owned(),
            }
        } else {
            String::new()
        };
        if !self.ocr_header_written.swap(true, Ordering::AcqRel) {
            self.journal
                .append_text("ocr-coverage.csv", &format!("{}\n", OCR_COLUMNS.join(",")));
        }
        let file = format!("opps-{}-{}.jsonl", cell.id, path.as_str());
        let images = truth["images"].as_array().cloned().unwrap_or_default();
        for image in images {
            self.check_time();
            if self.whole_stop_requested() {
                return SegmentEnd::Whole;
            }
            let image_name = image["image"].as_str().unwrap_or_default().to_owned();
            let image_kind = image["kind"].as_str().unwrap_or("truth").to_owned();
            let targets = image["targets"].as_array().cloned().unwrap_or_default();
            let source_path = std::path::Path::new(&c8.images_dir).join(&image_name);
            let bytes = match std::fs::read(&source_path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.journal.event(
                        "c8_image_unreadable",
                        json!({ "image": image_name, "detail": error.to_string() }),
                    );
                    return SegmentEnd::Fatal("c8_image_unreadable".to_owned());
                }
            };
            let slot = slot_name(&image_name);
            let put = self.apk_call(instance, Entry::ApkControl, |_, state| {
                state.client()?.put(&slot, &bytes)
            });
            let shown = put
                .and_then(|_| self.apk_set_mode(instance, "static", vec![("slot", slot.clone())]));
            if let Err(detail) = shown {
                self.journal.event(
                    "c8_image_not_shown",
                    json!({ "instance": name, "image": image_name, "detail": detail }),
                );
                return SegmentEnd::Fatal("c8_image_not_shown".to_owned());
            }
            let stem = slot.trim_end_matches(".png").to_owned();
            let row = |path_word: &str, frame: &str, saved: &str, target: &Value| {
                let class = target["class"].as_str().unwrap_or("template");
                let target_name = target["target"].as_str().unwrap_or_default();
                let verb = match class {
                    "visibility" => "is-visible",
                    "page" => "detect-page",
                    _ => "recognize",
                };
                let command = if class == "page" {
                    format!("{actinglab} --json {verb} --package {package} --scene {saved}")
                } else {
                    format!(
                        "{actinglab} --json {verb} --package {package} --target {target_name} --scene {saved}"
                    )
                };
                let result_json = format!(
                    "c8/results/{stem}-{}-{path_word}-{frame}.json",
                    slot_name(target_name)
                );
                let fields = [
                    image_name.clone(),
                    image_kind.clone(),
                    target_name.to_owned(),
                    class.to_owned(),
                    path_word.to_owned(),
                    if path_word == "S" {
                        bitrate.clone()
                    } else {
                        String::new()
                    },
                    info.orientation.to_owned(),
                    frame.to_owned(),
                    command,
                    package.clone(),
                    package_sha.clone(),
                    facts.clone(),
                    "1".to_owned(),
                    result_json,
                    target["hit"]
                        .as_bool()
                        .map(|hit| hit.to_string())
                        .unwrap_or_default(),
                    target["text"].as_str().unwrap_or_default().to_owned(),
                    String::new(),
                    String::new(),
                    "not_evaluated".to_owned(),
                    "pending_offline_actinglab".to_owned(),
                ];
                let line: Vec<String> = fields
                    .iter()
                    .map(|field| csv_field(field.as_str()))
                    .collect();
                self.journal
                    .append_text("ocr-coverage.csv", &format!("{}\n", line.join(",")));
            };
            if path == segment_first_path(cell, instance) {
                let source = source_path.display().to_string();
                for target in &targets {
                    row("source", "", &source, target);
                }
            }
            let mut last_sequence = None;
            for frame_idx in 0..c8.frames_per_image {
                let floor = last_sequence.map(|after| Floor {
                    after_us: None,
                    after_sequence: Some(after),
                });
                let sampled =
                    self.with_backend(instance, path.backend(), |backend| backend.sample(floor));
                let frame = match sampled {
                    Ok(frame) => frame,
                    Err(failure) => {
                        self.journal.event(
                            "c8_capture_failed",
                            json!({ "instance": name, "image": image_name, "path": path, "failure": failure }),
                        );
                        return SegmentEnd::Fatal(failure.code);
                    }
                };
                last_sequence = frame.stream_sequence.or(last_sequence);
                let format = match frame.layout {
                    crate::backend::PixelLayout::Rgba8 => actingcommand_device::PixelFormat::Rgba8,
                    crate::backend::PixelLayout::Rgb8 => actingcommand_device::PixelFormat::Rgb8,
                };
                let png = match actingcommand_device::encode_png_fast(
                    frame.width,
                    frame.height,
                    &frame.pixels,
                    format,
                ) {
                    Ok(png) => png,
                    Err(error) => {
                        self.journal.event(
                            "c8_encode_failed",
                            json!({ "image": image_name, "path": path, "detail": error.message() }),
                        );
                        return SegmentEnd::Fatal("c8_encode_failed".to_owned());
                    }
                };
                let path_word = if path == PathKind::Scrcpy && !bitrate.is_empty() {
                    format!("{}-{bitrate}", path.as_str())
                } else {
                    path.as_str().to_owned()
                };
                let saved = format!("c8/{stem}/{path_word}-{frame_idx}.png");
                if self.journal.write_bytes(&saved, &png).is_err() {
                    return SegmentEnd::Whole;
                }
                let line = info.record(
                    "c8_frame",
                    path.as_str(),
                    self.next_seq(),
                    json!({
                        "t_tick_us": self.now(),
                        "disposition": "delivered",
                        "image": image_name,
                        "image_kind": image_kind,
                        "frame_idx": frame_idx,
                        "frame_sha256": crate::safety::sha256_hex(&png),
                        "pixels_sha256": crate::safety::sha256_hex(&frame.pixels),
                        "saved_path": saved,
                        "bitrate": if path == PathKind::Scrcpy { Some(bitrate.clone()) } else { None },
                        "width": frame.width,
                        "height": frame.height,
                        "null_reasons": {},
                    }),
                );
                self.journal.append(&file, &line);
                let saved_path = self.journal.path(&saved).display().to_string();
                for target in &targets {
                    row(path.as_str(), &frame_idx.to_string(), &saved_path, target);
                }
            }
        }
        SegmentEnd::Finished
    }

    /// C9 (ii): N connects and disconnects around a sample, through P7 (the automatic reopen,
    /// E8): the old session closes and is confirmed, a new one opens, the generation moves.
    fn reconnect_nemu_for_sample(&self, instance: usize) -> Result<u64, String> {
        let owner = self.owner();
        let expected = self
            .hub
            .snapshot(&owner, |state| state.instances[instance].generation);
        let name = self.instance_name(instance).to_owned();
        let outcome = crate::hub::auto_reopen(&self.hub, &owner, instance, expected, |slots| {
            if let Some(backend) = slots.take(BackendKind::Nemu) {
                let closed = self.closer.close(
                    backend,
                    CloseInfo {
                        reason: "reconnect_each_sample".to_owned(),
                        instance: name.clone(),
                        generation: expected,
                        identity: None,
                    },
                );
                if !closed.is_confirmed() {
                    return Err(format!("the N close is unconfirmed: {closed:?}"));
                }
            }
            let backend = self
                .backends
                .open(&name, BackendKind::Nemu, &OpenOptions::default())
                .map_err(|failure| format!("open_{}", failure.cause.as_str()))?;
            slots.put(backend);
            Ok(())
        });
        match outcome {
            crate::hub::ReopenOutcome::Reopened { generation } => Ok(generation),
            crate::hub::ReopenOutcome::Failed(detail) => {
                if detail.contains("unconfirmed") {
                    self.unconfirmed(format!("{name} N reconnect: {detail}"));
                }
                Err(detail)
            }
            other => Err(format!("{other:?}")),
        }
    }
}

/// The first path a cell's segments plan on `instance` (C8 writes the source rows once).
fn segment_first_path(cell: &CellPlan, instance_index: usize) -> PathKind {
    let _ = instance_index;
    cell.segments
        .iter()
        .find_map(|segment| segment.path)
        .unwrap_or(PathKind::Nemu)
}

/// The paths of a plan's run directory (for the launcher's view).
pub fn run_files(run_dir: &Path) -> Vec<PathBuf> {
    [
        "header.json",
        "events.jsonl",
        "progress.jsonl",
        "heartbeat.json",
        "summary.json",
    ]
    .iter()
    .map(|name| run_dir.join(name))
    .collect()
}

/// Waits (a participant) for every participant thread in `threads`.
pub fn join_all(clock: &dyn Clock, threads: Vec<Participant<()>>) {
    blocking(clock, || {
        for thread in threads {
            let _ = thread.join();
        }
    });
}

/// The S close outcome as the device-side result requires (used by tests).
pub fn confirmed(outcome: &CloseOutcome) -> bool {
    outcome.is_confirmed()
}
