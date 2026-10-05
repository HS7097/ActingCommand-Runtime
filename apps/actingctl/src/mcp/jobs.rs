// SPDX-License-Identifier: AGPL-3.0-only

//! Background jobs (#338 §四 线程 / 预算, R2): at most eight run at a time. A run job's handle
//! is the Runtime request_id; the other jobs use the correlation of their write. Only the job
//! phase and its outcome live here, in memory: the authoritative state of a run always comes
//! from R1 (`ac_get_run`). A job runs to its end even after the call that started it returned;
//! at stdin EOF the server stops waiting for jobs and exits.

use super::lock;
use super::tools::ToolError;
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

pub(super) struct Jobs {
    table: Mutex<VecDeque<Arc<Job>>>,
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
    outcome: Option<Value>,
    warnings: Vec<Value>,
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

    /// Whether another job may start now.
    pub(super) fn has_capacity(&self) -> Result<(), ToolError> {
        let running = lock(&self.table)
            .iter()
            .filter(|job| !lock(&job.state).finished)
            .count();
        if running >= MAX_RUNNING {
            return Err(ToolError::new(
                "safety",
                "job_capacity_exhausted",
                format!("{MAX_RUNNING} background jobs are running; wait for one with ac_get_run"),
            ));
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
        self.has_capacity()?;
        let job = Arc::new(Job {
            handle,
            kind,
            instance_alias,
            state: Mutex::new(JobState {
                phase,
                finished: false,
                outcome: None,
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
                let outcome = body();
                let mut state = lock(&running.state);
                match outcome {
                    Ok(value) => {
                        state.phase = "done";
                        state.outcome = Some(value);
                    }
                    Err(error) => {
                        state.phase = "failed";
                        state.outcome = Some(json!({"error": error.into_value()}));
                    }
                }
                state.finished = true;
                running.changed.notify_all();
            })
            .map_err(|error| {
                ToolError::new(
                    "runtime",
                    "job_start_failed",
                    format!("cannot start the background job: {error}"),
                )
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

    /// The job's outcome once it ended.
    pub(super) fn outcome(&self) -> Option<Value> {
        lock(&self.state).outcome.clone()
    }

    pub(super) fn warnings(&self) -> Vec<Value> {
        lock(&self.state).warnings.clone()
    }

    /// `{phase, warnings}`, and the outcome once the job ended.
    pub(super) fn snapshot(&self) -> Value {
        let state = lock(&self.state);
        let mut snapshot = json!({
            "kind": self.kind,
            "phase": state.phase,
            "warnings": state.warnings,
        });
        if let Some(outcome) = &state.outcome {
            snapshot["outcome"] = outcome.clone();
        }
        snapshot
    }
}
