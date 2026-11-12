// SPDX-License-Identifier: AGPL-3.0-only

//! The jobs of this server (Workflow #434 LAB-5G3; register §1.287, "Slice LAB-5G3";
//! `contracts/lab-two-entries.md` §9.3), copied from today's MCP server
//! (`apps/actingctl/src/mcp/jobs.rs`; copied, not shared, as LAB-5G2a copied its loop) and adapted
//! to the generated tools: every served call runs as a job, on a thread of its own, to its end,
//! also after the call that started it answered. Only a job's phase and its line live here, in
//! this process's memory; a run's state is always read from the Runtime (`ac_run_show`).
//!
//! - **Handles**: a run's job is named by the run's Runtime request id, which outlives this
//!   process; every other job by `mcp_job_<session>_<n>` ([`JOB_PREFIX`]), which only this process
//!   holds.
//! - **The caps** are the start options `--jobs` (the jobs that run at once) and `--kept-jobs` (the
//!   jobs kept), where today's server has the constants 8 and 64, and each does what today's does:
//!   a job past the first is refused `job_capacity_exhausted` with `limit_count`, and past the
//!   second the oldest finished job is dropped, its handle then unknown (`handle_unknown`).
//! - **What the MCP entry asks** ([`Jobs::outstanding`], `actingcommand/entryStatus`): a job counts
//!   while it runs and, once it ended, until its line was read, if an answer carrying its handle
//!   went out to the client; a run's job only while it submits the run.

use crate::calls::Finished;
use crate::lock;
use actingcommand_contract::outcome::{IoOp, Outcome};
use actingcommand_lab_ops::codes::ActingctlCode;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The prefix of the handle of a job only this process holds.
pub(crate) const JOB_PREFIX: &str = "mcp_job_";
/// A job's phase while its work runs.
const RUNNING: &str = "running";
/// A run's job's phase while it submits the run and waits for its receipt.
const SUBMITTING: &str = "submitting";
/// A job's phase once it ended with a line that exits zero, or non-zero.
const DONE: &str = "done";
const FAILED: &str = "failed";
/// How often a wait for a job looks at its call's cancellation.
const WAKE: Duration = Duration::from_millis(500);

/// The two caps of the start options: the jobs that run at once (`--jobs`) and the jobs kept
/// (`--kept-jobs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) running: usize,
    pub(crate) kept: usize,
}

/// The handles `actingcommand/entryStatus` reports.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Outstanding {
    pub(crate) handles: Vec<String>,
    pub(crate) kinds: Vec<String>,
}

/// Every job this process keeps, oldest first.
pub(crate) struct Jobs {
    table: Mutex<VecDeque<Arc<Job>>>,
    limits: Limits,
    /// This process's identity in its handles.
    session: u64,
    next: AtomicU64,
}

/// One job.
#[derive(Debug)]
pub(crate) struct Job {
    pub(crate) handle: String,
    /// The tool whose call started it.
    pub(crate) kind: String,
    /// A run's job: its handle is the run's Runtime request id.
    pub(crate) run: bool,
    state: Mutex<JobState>,
    changed: Condvar,
}

#[derive(Debug)]
struct JobState {
    phase: &'static str,
    /// Its line, once it ended.
    ended: Option<Finished>,
    /// Its line was handed to the client: in the answer of its call, or by `ac_run_show`.
    collected: bool,
    /// An answer carrying its handle went out to the client.
    issued: bool,
}

impl Jobs {
    pub(crate) fn new(limits: Limits, session: u64) -> Self {
        Self {
            table: Mutex::new(VecDeque::new()),
            limits,
            session,
            next: AtomicU64::new(0),
        }
    }

    /// A handle no job of this process had: `mcp_job_<session>_<n>`.
    pub(crate) fn handle(&self) -> String {
        let number = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        format!("{JOB_PREFIX}{:016x}_{number}", self.session)
    }

    /// The job kept under `handle`.
    pub(crate) fn find(&self, handle: &str) -> Option<Arc<Job>> {
        lock(&self.table)
            .iter()
            .find(|job| job.handle == handle)
            .cloned()
    }

    /// The handles only this process can answer for, and their kinds (the tools of their calls),
    /// as today's server counts them: a job while it runs and, once it ended, until its line was
    /// read, if its handle went out; a run's job only while it submits, since its handle, the
    /// run's request id, is read from the Runtime by any process.
    pub(crate) fn outstanding(&self) -> Outstanding {
        let table = lock(&self.table);
        let mut outstanding = Outstanding::default();
        for job in table.iter() {
            let state = lock(&job.state);
            let running = state.ended.is_none();
            let counts = if job.run {
                running
            } else {
                running || (state.issued && !state.collected)
            };
            if counts {
                outstanding.handles.push(job.handle.clone());
                if !outstanding.kinds.contains(&job.kind) {
                    outstanding.kinds.push(job.kind.clone());
                }
            }
        }
        outstanding
    }

    /// An answer carrying `handle` went out to the client.
    pub(crate) fn issue(&self, handle: &str) {
        if let Some(job) = self.find(handle) {
            lock(&job.state).issued = true;
        }
    }

    /// Whether another job may start now: `job_capacity_exhausted` with `limit_count` while as
    /// many run as `--jobs` allows (today's server: 8).
    pub(crate) fn has_capacity(&self) -> Result<(), Outcome> {
        let running = lock(&self.table)
            .iter()
            .filter(|job| !job.finished())
            .count();
        if running >= self.limits.running {
            return Err(
                Outcome::new(ActingctlCode::JobCapacityExhausted).limit_count(self.limits.running)
            );
        }
        Ok(())
    }

    /// Keeps a job under `handle` and runs `body` on its own thread; it ends `done` or `failed`
    /// with `body`'s line. A run's job (`run`) is `submitting` while it runs, any other `running`.
    /// Past `--kept-jobs` the oldest finished job is dropped. A job past `--jobs` is refused
    /// ([`Self::has_capacity`]); a thread the OS refuses is `job_start_failed`, caused by the OS
    /// error, and nothing is kept.
    pub(crate) fn start(
        &self,
        handle: String,
        kind: &str,
        run: bool,
        body: impl FnOnce() -> Finished + Send + 'static,
    ) -> Result<Arc<Job>, Outcome> {
        self.has_capacity()?;
        let job = Arc::new(Job {
            handle,
            kind: kind.to_owned(),
            run,
            state: Mutex::new(JobState {
                phase: if run { SUBMITTING } else { RUNNING },
                ended: None,
                collected: false,
                issued: false,
            }),
            changed: Condvar::new(),
        });
        {
            let mut table = lock(&self.table);
            table.push_back(Arc::clone(&job));
            while table.len() > self.limits.kept {
                let Some(position) = table.iter().position(|kept| kept.finished()) else {
                    break;
                };
                table.remove(position);
            }
        }
        let running = Arc::clone(&job);
        let spawned = thread::Builder::new()
            .name(format!("mcp-job-{kind}"))
            .spawn(move || {
                let ended = body();
                let mut state = lock(&running.state);
                state.phase = if ended.is_error { FAILED } else { DONE };
                state.ended = Some(ended);
                running.changed.notify_all();
            });
        if let Err(error) = spawned {
            lock(&self.table).retain(|kept| !Arc::ptr_eq(kept, &job));
            return Err(Outcome::new(ActingctlCode::JobStartFailed)
                .caused_by(Outcome::from_io_error(&error, IoOp::Spawn)));
        }
        Ok(job)
    }
}

impl Job {
    pub(crate) fn phase(&self) -> &'static str {
        lock(&self.state).phase
    }

    pub(crate) fn finished(&self) -> bool {
        lock(&self.state).ended.is_some()
    }

    /// Waits until the job ends, `until` passes or `cancelled` is set (`notifications/cancelled`,
    /// looked at every half second); true when it ended. A wait that stops leaves the job running.
    pub(crate) fn wait_until(&self, until: Instant, cancelled: &AtomicBool) -> bool {
        let mut state = lock(&self.state);
        while state.ended.is_none() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || cancelled.load(Ordering::SeqCst) {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left.min(WAKE))
                .map_or_else(|poisoned| poisoned.into_inner().0, |(state, _)| state);
        }
        true
    }

    /// Its line once it ended; [`Self::collect`] says it was handed out.
    pub(crate) fn ended(&self) -> Option<Finished> {
        lock(&self.state).ended.clone()
    }

    /// Its line was handed to the client, so it no longer holds a switch of the MCP entry back.
    pub(crate) fn collect(&self) {
        lock(&self.state).collected = true;
    }
}
