// SPDX-License-Identifier: AGPL-3.0-only

//! The D1 rehearsal's harness (plan v2.1 §6.2): the fake backend, the virtual clock (no test
//! reads a real clock or sleeps), a run directory under the test target's temporary directory.

#![allow(dead_code)]

use actingcommand_capture_bench::clock::{Clock, Participant, VirtualClock, spawn_participant};
use actingcommand_capture_bench::fake::{
    FakeCheckScript, FakeDevice, FakeMaterials, FakeScript, FaultAt, RecordedExit,
};
use actingcommand_capture_bench::metrics::{MetricsSample, ScriptedMetrics};
use actingcommand_capture_bench::model::BackendKind;
use actingcommand_capture_bench::runner::{Env, RunOutcome, run};
use actingcommand_capture_bench::safety::{AcceptStartChecks, Roots, StartChecks};
use actingcommand_capture_bench::supervisor::FixedLimits;
use actingcommand_capture_bench::viewer::InProcessViewers;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The virtual clock's wall-clock origin.
pub const UNIX_ORIGIN_MS: u64 = 1_800_000_000_000;

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

/// The production limits with the close deadlines shortened (§6.2 D1 item 7: 测试里把期限调短).
pub fn limits() -> FixedLimits {
    let mut limits = FixedLimits::production();
    limits.scrcpy_close_deadline_us = 3_000_000;
    limits.nemu_close_deadline_us = 3_000_000;
    limits.device_check_deadline_us = 3_000_000;
    limits
}

/// A plan with one instance `E1` and the given `[[cells]]` tables.
pub fn plan(run_id: &str, until_offset_ms: u64, cells: &str) -> String {
    format!(
        r#"run_id = "{run_id}"
until_unix_ms = {until}
total_duration_ms = 36000000
window = "D1"

[safety]
adb_path = 'C:\fake\adb.exe'
adb_version = "37.0.1-15733141"
mumu_folder = 'C:\fake\MuMu'
priority_class = "normal"
serial_whitelist = ["127.0.0.1:16544", "127.0.0.1:16576"]
nemu_whitelist = [5, 6]

[[instances]]
name = "E1"
serial = "127.0.0.1:16544"
nemu_instance = 5
orientation = "landscape"
touch = "adb_input"

{cells}
"#,
        until = UNIX_ORIGIN_MS + until_offset_ms,
    )
}

/// A cell with one segment.
pub fn cell(id: &str, limit_ms: u64, segments: &[&str]) -> String {
    let mut text = format!("[[cells]]\nid = \"{id}\"\nlimit_ms = {limit_ms}\n\n");
    for segment in segments {
        text.push_str("[[cells.segments]]\n");
        text.push_str(segment);
        text.push_str("\n\n");
    }
    text
}

pub fn preview(path: &str, duration_ms: u64) -> String {
    format!("instance = \"E1\"\npath = \"{path}\"\nmode = \"preview\"\nduration_ms = {duration_ms}")
}

pub fn at(kind: BackendKind, from_call: u64, count: u64) -> FaultAt {
    FaultAt {
        instance: "E1".to_owned(),
        kind,
        from_call,
        count,
    }
}

pub struct Harness {
    pub clock: Arc<VirtualClock>,
    pub device: Arc<FakeDevice>,
    pub exit: Arc<RecordedExit>,
    pub root: PathBuf,
    pub run_dir: PathBuf,
    pub roots: Roots,
}

pub fn harness(name: &str, run_id: &str, script: FakeScript, check: FakeCheckScript) -> Harness {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "capture-bench-d1/{name}-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let roots = Roots {
        lock_path: root.join("bench.lock"),
        runs_root: root.join("runs"),
    };
    let run_dir = roots.runs_root.join(run_id);
    let clock = Arc::new(VirtualClock::new(UNIX_ORIGIN_MS));
    let dyn_clock: Arc<dyn Clock> = clock.clone();
    let device = FakeDevice::new(dyn_clock, script, check);
    Harness {
        clock,
        device,
        exit: RecordedExit::new(),
        root,
        run_dir,
        roots,
    }
}

impl Harness {
    pub fn dyn_clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    pub fn env(
        &self,
        limits: FixedLimits,
        metrics: Vec<MetricsSample>,
        checks: Box<dyn StartChecks>,
    ) -> Env {
        Env {
            clock: self.dyn_clock(),
            backends: Arc::new(Arc::clone(&self.device)),
            device_check: Arc::new(Arc::clone(&self.device)),
            materials: Arc::new(Arc::new(FakeMaterials {
                writes: AtomicU64::new(0),
            })),
            metrics: Box::new(ScriptedMetrics {
                samples: metrics,
                next: 0,
            }),
            viewers: Arc::new(InProcessViewers),
            start_checks: checks,
            exit: self.exit.clone(),
            limits,
            roots: self.roots.clone(),
            header_facts: json!({ "backend": "fake" }),
        }
    }

    /// Starts the run on a participant thread of the virtual clock.
    pub fn spawn(
        &self,
        plan: String,
        limits: FixedLimits,
        metrics: Vec<MetricsSample>,
    ) -> Participant<RunOutcome> {
        self.spawn_with(plan, limits, metrics, Box::new(AcceptStartChecks))
    }

    pub fn spawn_with(
        &self,
        plan: String,
        limits: FixedLimits,
        metrics: Vec<MetricsSample>,
        checks: Box<dyn StartChecks>,
    ) -> Participant<RunOutcome> {
        let env = self.env(limits, metrics, checks);
        let run_dir = self.run_dir.clone();
        spawn_participant(&self.dyn_clock(), "d1-run", move || {
            run(&plan, &run_dir, env)
        })
        .expect("the run thread starts")
    }

    pub fn run(&self, plan: String) -> RunOutcome {
        self.spawn(plan, limits(), Vec::new())
            .join()
            .expect("the run does not panic")
    }

    pub fn lines(&self, name: &str) -> Vec<Value> {
        let text = std::fs::read_to_string(self.run_dir.join(name)).unwrap_or_default();
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    pub fn events(&self) -> Vec<Value> {
        self.lines("events.jsonl")
    }

    pub fn json(&self, name: &str) -> Value {
        let text = std::fs::read_to_string(self.run_dir.join(name))
            .unwrap_or_else(|error| panic!("read {name}: {error}"));
        serde_json::from_str(&text).expect("a JSON file")
    }
}

/// The position of the first event of `kind` matching `filter`.
pub fn position(events: &[Value], kind: &str, filter: impl Fn(&Value) -> bool) -> Option<usize> {
    events
        .iter()
        .position(|event| event["type"] == kind && filter(event))
}

/// The result of `instance/path` in a cell summary.
pub fn cell_result(summary: &Value, path: &str) -> (String, String) {
    let entry = summary["results"]
        .as_array()
        .expect("summary results")
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap_or_else(|| panic!("no result for {path} in {summary}"));
    (
        entry["result"].as_str().unwrap_or_default().to_owned(),
        entry["reason"].as_str().unwrap_or_default().to_owned(),
    )
}

/// The result of `path` in `cell` from `summary.json`.
pub fn run_cell_result(summary: &Value, cell: &str, path: &str) -> (String, String) {
    let entry = summary["cells"]
        .as_array()
        .expect("summary cells")
        .iter()
        .find(|entry| entry["cell"] == cell && entry["path"] == path)
        .unwrap_or_else(|| panic!("no result for {cell} {path} in {summary}"));
    (
        entry["result"].as_str().unwrap_or_default().to_owned(),
        entry["reason"].as_str().unwrap_or_default().to_owned(),
    )
}
