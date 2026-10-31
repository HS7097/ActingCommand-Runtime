// SPDX-License-Identifier: AGPL-3.0-only

//! The plan executor (§3.2 item 3, §3.11): the start check, the cells and their segments, the
//! cell end (§3.11.2 A), the whole-run stop (§3.11.2 B), the results and exit codes (§3.11.3),
//! the error classes (§3.11.4) and the device-side close confirmation (§3.5.1).

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
use crate::metrics::{MetricsSample, MetricsSource};
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
    owner: Arc<LockOwner>,
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
        owner: LockOwner::new("runner"),
        started_us: clock.now_us(),
        started_unix_ms: clock.unix_ms(),
        next_subscriber: AtomicU64::new(1),
        working_set: AtomicU64::new(0),
        run_stop: Mutex::new(None),
        cell_fatal: Mutex::new(BTreeMap::new()),
        next_record: AtomicU64::new(1),
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
}

impl SegmentShared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
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
    Dispatch { preview_state: String },
    Admission { j: u32 },
    Poll { after_sequence: Option<u64> },
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

/// Why `content` is null: the timecode is not decoded by this slice (its layout is C3's).
const CONTENT_NULL_REASON: &str =
    "the bench does not decode the test-pattern timecode yet (the layout is C3's)";

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
        let seq = self.hub.set_global(&self.owner, GlobalGate::RunStop, true);
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
                    self.hub.bump_generation(&self.owner, instance);
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
                let info =
                    SegmentInfo::new(&self.plan, cell, segment, instance, round, segment_index);
                let end = self.run_segment(cell, segment, instance, &info, deadline, &mut rng);
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
        let gate_seq = self.hub.set_global(&self.owner, GlobalGate::CellEnd, true);
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
        self.hub.set_global(&self.owner, GlobalGate::CellEnd, false);
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
                &self.owner,
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
                    match self.positive_identity(instance, method, &stream) {
                        Ok(identity) => {
                            self.journal.event(
                                "scrcpy_identity",
                                json!({ "instance": name, "identity": identity }),
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
                &self.owner,
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
                &self.owner,
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
            .snapshot(&self.owner, |state| state.instances[instance].generation);
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
                &self.owner,
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
        self.hub.admission[instance].acquire(&self.owner);
        let taken = lock(&self.hub.backends[instance]).take(kind);
        self.hub.admission[instance].release(&self.owner);
        taken
    }

    fn put_backend(&self, instance: usize, backend: Box<dyn PreviewBackend>) {
        self.hub.admission[instance].acquire(&self.owner);
        lock(&self.hub.backends[instance]).put(backend);
        self.hub.admission[instance].release(&self.owner);
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
    fn run_segment(
        self: &Arc<Self>,
        cell: &CellPlan,
        segment: &SegmentPlan,
        instance: usize,
        info: &SegmentInfo,
        cell_deadline: Micros,
        rng: &mut Rng,
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
                    self.hub.admission[instance].acquire(&self.owner);
                    lock(&self.hub.backends[instance]).material = Some(writer);
                    self.hub.admission[instance].release(&self.owner);
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
            if let Ok(thread) =
                spawn_participant(&self.clock, &format!("preview-{label}"), move || {
                    run.preview_activity(&segment_shared, instance, path, fps, quota, &file, &info);
                })
            {
                activities.push(thread);
            }
        }
        if let Some(formal) = segment.formal.clone() {
            let run = Arc::clone(self);
            let segment_shared = Arc::clone(&shared);
            let preview_state = segment.path.map_or("off", PathKind::as_str);
            let file = format!("opps-{}-{preview_state}.jsonl", cell.id);
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
                        preview_state,
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
                self.hub.sweep(&self.owner, ms(segment.grace_ms));
                if self.hub.grace_expired(&self.owner, instance) == Some(true)
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
                SegmentMode::ColdStart => false,
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
        let segment_gate = self.hub.set_global(&self.owner, GlobalGate::CellEnd, true);
        self.journal.event(
            "segment_stop_new",
            json!({ "segment": label, "gate_seq": segment_gate, "end": format!("{end:?}") }),
        );
        shared.stop.store(true, Ordering::Release);
        for activity in activities {
            let _ = activity.join();
        }
        if self.terminal.finished_by_supervisor() {
            return SegmentEnd::Whole;
        }
        for mut viewer in viewers {
            viewer.stop(true);
        }
        if producer {
            self.hub.views.release_producer(instance);
        }
        for gate in set_gates {
            self.hub.set_gate(&self.owner, instance, gate, false);
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
        self.hub.admission[instance].acquire(&self.owner);
        let material = lock(&self.hub.backends[instance]).material.take();
        self.hub.admission[instance].release(&self.owner);
        drop(material);
        let confirmed = self.close_segment_backends(instance, &mut identities, "segment_end");
        self.hub.set_global(&self.owner, GlobalGate::CellEnd, false);
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
            let seq = self.hub.set_gate(&self.owner, instance, gate, true);
            set_gates.push(gate);
            self.journal.event(
                "gate_applied",
                json!({ "instance": self.instance_name(instance), "gate": gate, "seq": seq }),
            );
            if gate.is_hard() {
                self.hub.end_grace_hard(&self.owner, instance);
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
                    self.hub.set_gate(&self.owner, instance, cleared, false);
                }
            }
            let generation = self.hub.bump_generation(&self.owner, instance);
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
        while self.hub.inflight(&self.owner, instance) > 0 && self.now() < drain_deadline {
            pause_until(self.clock.as_ref(), self.now() + ms(10));
        }
        let drained = self.hub.inflight(&self.owner, instance) == 0;
        let open_after: Vec<&str> = {
            self.hub.admission[instance].acquire(&self.owner);
            let slots = lock(&self.hub.backends[instance]);
            let mut open = Vec::new();
            if slots.nemu.is_some() {
                open.push("nemu");
            }
            if slots.scrcpy.is_some() {
                open.push("scrcpy");
            }
            drop(slots);
            self.hub.admission[instance].release(&self.owner);
            open
        };
        let unconfirmed_now = !lock(&self.unconfirmed).is_empty();
        if drained && open_after.is_empty() && !unconfirmed_now {
            self.hub
                .set_gate(&self.owner, instance, GateKind::Holding, true);
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
        self.hub.admission[instance].acquire(&self.owner);
        let result = match lock(&self.hub.backends[instance]).get(kind) {
            Some(backend) => call(backend),
            None => Err(BackendFailure::new(
                "backend_closed",
                FailureClass::CellPath,
                "the backend is not open",
            )
            .without_native_call()),
        };
        self.hub.admission[instance].release(&self.owner);
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
            let mut op = SampleOp::new(instance, path, Arc::clone(&owner));
            let outcome = run_blocking(&self.hub, &mut op);
            let delivered = outcome.disposition.is_delivered();
            let mut null_reasons = json!({ "content": CONTENT_NULL_REASON });
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
                    "content": null,
                    "null_reasons": null_reasons,
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
            let work: crate::hub::FormalWork = Box::new(move |hub: &Hub, instance: usize| {
                formal_work(
                    hub,
                    instance,
                    capture_kind,
                    cell_index,
                    grid,
                    clock.as_ref(),
                    true,
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
            };
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
            RecordShape::Dispatch { preview_state } => preview_state.clone(),
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
                match shape {
                    RecordShape::Dispatch { preview_state } => json!({
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
                        "stdio_wait_us": [],
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
                        "tap": { "j": j, "t_input_done_us": work.tap_done_us },
                        "floor_check": work.floor_frame.as_ref().and_then(|frame| frame.floor_check),
                        "native_start_us": work.floor_frame.as_ref().and_then(|frame| frame.native.iter().find_map(|span| span.start_us)),
                        "t_recv_us": work.floor_frame.as_ref().and_then(|frame| frame.received_us),
                        "stream_seq": work.floor_frame.as_ref().and_then(|frame| frame.stream_sequence),
                        "known_source_us": work.floor_frame.as_ref().and_then(|frame| frame.known_source_us),
                        "admission_wait_us": wait,
                        "floor_capture_us": work.floor_capture_us,
                        "content": null,
                        "first_flip": null,
                        "null_reasons": { "content": CONTENT_NULL_REASON, "first_flip": CONTENT_NULL_REASON },
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
                "preview_state": match shape { RecordShape::Dispatch { preview_state } => Some(preview_state.clone()), _ => None },
            }),
            DispatchOutcome::Refused { gate } => json!({
                "trial": trial,
                "t_tick_us": tick,
                "disposition": format!("skipped_gate:{}", gate.as_str()),
                "contended": false,
                "preview_state": match shape { RecordShape::Dispatch { preview_state } => Some(preview_state.clone()), _ => None },
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
        let last_sequence = Arc::new(Mutex::new(None::<u64>));
        while !segment.stopped() {
            if quota.is_some_and(|quota| check >= quota) || segment.backend_disabled(kind) {
                break;
            }
            check += 1;
            let cell_index = rng.below(u64::from(grid * grid)) as u32;
            let clock = Arc::clone(&self.clock);
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
                    formal_work(hub, instance, kind, cell_index, grid, clock.as_ref(), false)
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
        let violations = self.hub.violations(&self.owner);
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
            .snapshot(&self.owner, |state| state.checker.clone());
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

/// P4's formal step: capture, tap a grid cell, capture with a floor after the tap.
fn formal_work(
    hub: &Hub,
    instance: usize,
    kind: BackendKind,
    cell_index: u32,
    grid: u32,
    clock: &dyn Clock,
    capture_first: bool,
) -> FormalResult {
    let mut result = FormalResult::default();
    if capture_first {
        let started = clock.now_us();
        if let Err(failure) = formal_capture(hub, instance, kind, None) {
            result.failure = Some(failure);
            return result;
        }
        result.capture_us = Some(clock.now_us().saturating_sub(started));
    }
    // The tap goes to the Nemu backend when it is open (the instance's touch path), else to S.
    let (width, height) = (1280_i32, 720_i32);
    let grid = grid.max(1) as i32;
    let column = (cell_index as i32) % grid;
    let row = (cell_index as i32) / grid;
    let x = column * width / grid + width / (2 * grid);
    let y = row * height / grid + height / (2 * grid);
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
