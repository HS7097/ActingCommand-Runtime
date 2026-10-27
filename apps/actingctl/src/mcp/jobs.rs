// SPDX-License-Identifier: AGPL-3.0-only

//! Background jobs (#338 §四 线程 / 预算, R2): at most eight run at a time. A run job's handle
//! is the Runtime request_id; the other jobs use the correlation of their write, and a Lab
//! call's job a `lab_job_` handle of this process. Only the job
//! phase and its result live here, in memory: the authoritative state of a run always comes
//! from R1 (`ac_get_run`). A job runs to its end even after the call that started it returned;
//! at stdin EOF the server stops waiting for jobs and exits. Workflow #378 A5 (L6): a job's
//! result is `job_result` (the answer, or `{error {class, outcome}}`); `outcome` names only the
//! envelope.

use super::lock;
use super::tools::ToolError;
use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{IoOp, Outcome};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Background jobs running at the same time.
const MAX_RUNNING: usize = 8;
/// Finished jobs kept for `ac_get_run`; the oldest go first.
const MAX_KEPT: usize = 64;

/// How a noted job's body ends: its result and the warnings it found on the way.
pub(super) type JobEnd = (Result<Value, ToolError>, Vec<Value>);

pub(super) struct Jobs {
    table: Mutex<VecDeque<Arc<Job>>>,
}

/// The process-local handles `actingcommand/entryStatus` reports (Workflow #421 MCP-ENTRY D4).
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Outstanding {
    pub(super) handles: Vec<String>,
    pub(super) kinds: Vec<&'static str>,
}

pub(super) struct Job {
    pub(super) handle: String,
    pub(super) kind: &'static str,
    /// The instance alias a run job submitted to.
    pub(super) instance_alias: Option<String>,
    state: Mutex<JobState>,
    changed: Condvar,
}

struct JobState {
    phase: &'static str,
    finished: bool,
    /// The body's result once the job ended.
    result: Option<Result<Value, ToolError>>,
    /// Workflow #421 MCP-ENTRY (D4): the result was handed to the client (an answer or
    /// ac_get_run), so it no longer holds a switch of the MCP entry back.
    collected: bool,
    /// MCP-ENTRY fix round 1 (review M2): an answer carrying the handle went out to the client.
    /// A finished job whose handle never went out (its call was cancelled) holds nothing back.
    issued: bool,
    warnings: Vec<Value>,
}

/// The kind of a run job: its handle is the Runtime request_id, which survives any process.
const RUN_KIND: &str = "run_pack";

thread_local! {
    /// The handles the tool call running on this thread put in its answer (review M2); the
    /// call's worker marks them issued when that answer goes out.
    static OFFERED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Notes that the answer of the call on this thread carries `handle`.
pub(super) fn offer(handle: &str) {
    OFFERED.with(|offered| offered.borrow_mut().push(handle.to_owned()));
}

/// The handles offered on this thread since the last call, emptied.
pub(super) fn take_offered() -> Vec<String> {
    OFFERED.with(|offered| std::mem::take(&mut *offered.borrow_mut()))
}

impl Jobs {
    pub(super) fn new() -> Self {
        Self {
            table: Mutex::new(VecDeque::new()),
        }
    }

    pub(super) fn find(&self, handle: &str) -> Option<Arc<Job>> {
        lock(&self.table)
            .iter()
            .find(|job| job.handle == handle)
            .cloned()
    }

    /// Workflow #421 MCP-ENTRY (D4, `actingcommand/entryStatus`): the handles only this process
    /// can answer for, their kinds and the handles themselves. A job counts while it runs and,
    /// once ended, until its result was handed out, but only if its handle went out to the
    /// client (fix round 1, review M2). A run job counts only while its submission runs (review
    /// M3); after that its handle is a Runtime request_id.
    pub(super) fn outstanding(&self) -> Outstanding {
        let table = lock(&self.table);
        let mut outstanding = Outstanding::default();
        for job in table.iter() {
            let state = lock(&job.state);
            let counts = if job.kind == RUN_KIND {
                !state.finished
            } else {
                !state.finished || (state.issued && !state.collected)
            };
            if counts {
                outstanding.handles.push(job.handle.clone());
                if !outstanding.kinds.contains(&job.kind) {
                    outstanding.kinds.push(job.kind);
                }
            }
        }
        outstanding
    }

    /// Review M2: the answer carrying `handles` went out to the client.
    pub(super) fn issue(&self, handles: &[String]) {
        if handles.is_empty() {
            return;
        }
        for job in lock(&self.table).iter() {
            if handles.contains(&job.handle) {
                lock(&job.state).issued = true;
            }
        }
    }

    /// Whether another job may start now: `job_capacity_exhausted` (`limit_count`) while
    /// eight run; wait for one with ac_get_run.
    pub(super) fn has_capacity(&self) -> Result<(), ToolError> {
        let running = lock(&self.table)
            .iter()
            .filter(|job| !lock(&job.state).finished)
            .count();
        if running >= MAX_RUNNING {
            return Err(
                ToolError::new("safety", ActingctlCode::JobCapacityExhausted)
                    .with(|outcome| outcome.limit_count(MAX_RUNNING)),
            );
        }
        Ok(())
    }

    /// Registers a job and runs `body` on its own thread. `phase` names the work while it
    /// runs; the job ends `done` with `body`'s result or `failed` with its error.
    pub(super) fn start(
        &self,
        handle: String,
        kind: &'static str,
        instance_alias: Option<String>,
        phase: &'static str,
        warnings: Vec<Value>,
        body: impl FnOnce() -> Result<Value, ToolError> + Send + 'static,
    ) -> Result<Arc<Job>, ToolError> {
        self.start_noted(handle, kind, instance_alias, phase, warnings, move || {
            (body(), Vec::new())
        })
    }

    /// As `start`, for a body that also reports warnings found while it ran (a Lab call's
    /// client.action); they join the job's warnings before it ends. A thread the OS refuses is
    /// `job_start_failed`, caused by the OS error.
    pub(super) fn start_noted(
        &self,
        handle: String,
        kind: &'static str,
        instance_alias: Option<String>,
        phase: &'static str,
        warnings: Vec<Value>,
        body: impl FnOnce() -> JobEnd + Send + 'static,
    ) -> Result<Arc<Job>, ToolError> {
        self.has_capacity()?;
        let job = Arc::new(Job {
            handle,
            kind,
            instance_alias,
            state: Mutex::new(JobState {
                phase,
                finished: false,
                result: None,
                collected: false,
                issued: false,
                warnings,
            }),
            changed: Condvar::new(),
        });
        {
            let mut table = lock(&self.table);
            table.push_back(Arc::clone(&job));
            while table.len() > MAX_KEPT {
                let Some(position) = table.iter().position(|job| lock(&job.state).finished) else {
                    break;
                };
                table.remove(position);
            }
        }
        let running = Arc::clone(&job);
        thread::Builder::new()
            .name(format!("mcp-job-{kind}"))
            .spawn(move || {
                let (result, notes) = body();
                let mut state = lock(&running.state);
                state.warnings.extend(notes);
                state.phase = if result.is_ok() { "done" } else { "failed" };
                state.result = Some(result);
                state.finished = true;
                running.changed.notify_all();
            })
            .map_err(|error| {
                ToolError::new("runtime", ActingctlCode::JobStartFailed)
                    .caused_by(Outcome::from_io_error(&error, IoOp::Spawn))
            })?;
        Ok(job)
    }
}

impl Job {
    pub(super) fn finished(&self) -> bool {
        lock(&self.state).finished
    }

    pub(super) fn phase(&self) -> &'static str {
        lock(&self.state).phase
    }

    /// Waits until the job ends, `until` passes or the call is cancelled
    /// (`notifications/cancelled`, checked at every wake-up); true when it ended. A cancelled
    /// wait leaves the job running.
    pub(super) fn wait_until(&self, until: Instant, cancelled: &AtomicBool) -> bool {
        let mut state = lock(&self.state);
        while !state.finished {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || cancelled.load(Ordering::SeqCst) {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left.min(Duration::from_millis(500)))
                .map_or_else(|poisoned| poisoned.into_inner().0, |(state, _)| state);
        }
        true
    }

    /// The job's result once it ended (Workflow #378 A5 L6, formerly `outcome`): the body's
    /// answer, or its error as the body returned it.
    pub(super) fn job_result(&self) -> Option<Result<Value, ToolError>> {
        let mut state = lock(&self.state);
        state.collected |= state.result.is_some();
        state.result.clone()
    }

    pub(super) fn warnings(&self) -> Vec<Value> {
        lock(&self.state).warnings.clone()
    }

    /// `{kind, phase, warnings}`, and once the job ended its `job_result`: the answer, or
    /// `{error {class, outcome}}`.
    pub(super) fn snapshot(&self) -> Value {
        let mut state = lock(&self.state);
        state.collected |= state.result.is_some();
        let mut snapshot = json!({
            "kind": self.kind,
            "phase": state.phase,
            "warnings": state.warnings,
        });
        if let Some(result) = &state.result {
            snapshot["job_result"] = match result {
                Ok(answer) => answer.clone(),
                Err(error) => json!({"error": error.to_value()}),
            };
        }
        snapshot
    }
}
