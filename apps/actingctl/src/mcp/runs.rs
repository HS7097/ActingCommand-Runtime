// SPDX-License-Identifier: AGPL-3.0-only

//! Run tools (#338 §四 工具表, R1): ac_get_run and ac_diagnose. Every run is runtime-client's
//! `actingcommand.run-status.v1` projection, passed on unchanged; this module only picks which
//! runs to read, waits, and bounds the output. ac_diagnose is `actingctl diagnose`
//! (`crate::diagnose`, Workflow #389 MCP-7): the same reads and result, cut here to the output
//! budget. ac_overview's run per instance is read with `crate::overview` (Workflow #389 MCP-8).

use super::lab;
use super::runtime::Connected;
use super::tools::{
    self, Arguments, ToolContext, ToolError, ToolOutcome, ToolSuccess, invalid_argument,
};
use crate::codes::{ActingctlCode, ArgumentProblem};
use crate::diagnose::{self, DiagnoseRequest, Failure};
use crate::events::{CursorBinding, MAX_INTEGER};
use crate::overview::Warning;
use actingcommand_contract::{RequestId, RunId};
use actingcommand_runtime_client::{ContainedRunState, RunKey, RunStatusMode};
use serde_json::{Map, Value, json};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Workflow #389 MCP-7: ac_diagnose's parts, shared with `actingctl diagnose`.
pub(super) use crate::diagnose::Diagnosis;

const MAX_WAIT_S: u64 = 25;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Time kept back from the call budget for the last full run read of a wait.
const WAIT_READ_RESERVE: Duration = Duration::from_secs(5);
/// Time kept back from the call budget before another Runtime read starts.
const READ_RESERVE: Duration = Duration::from_secs(1);

/// Now, in Unix milliseconds; `clock_unavailable` when the system clock is before the Unix epoch
/// or out of range.
fn now_unix_ms() -> Result<u64, ToolError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| ToolError::new("runtime", ActingctlCode::ClockUnavailable))
}

/// ac_get_run: R1 Full by handle (request_id) or run_id; with wait_s it reads again every
/// second while the state is not_found, admitted or running.
pub(super) fn get_run(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["handle", "run_id", "wait_s"])?;
    let wait = Duration::from_secs(arguments.integer("wait_s", 0, MAX_WAIT_S)?.unwrap_or(0));
    let last_read = context
        .deadline
        .checked_sub(WAIT_READ_RESERVE)
        .unwrap_or(context.deadline);
    let wait_until = (Instant::now() + wait).min(last_read);
    let handle = arguments.string("handle")?;
    let job = handle.and_then(|handle| context.jobs.find(handle));
    // A job of this process that is not a run (a pause, a resume, an emulator control, a
    // stop, a Lab call) has no run status: its phase and, once it ended, its outcome.
    if let Some(job) = job.as_ref().filter(|job| job.kind != "run_pack") {
        // Exactly one of handle and run_id.
        if arguments.string("run_id")?.is_some() {
            return Err(invalid_argument("handle", ArgumentProblem::Conflicting));
        }
        job.wait_until(wait_until, context.cancelled);
        return Ok(ToolSuccess::new(
            json!({"handle": job.handle, "job": job.snapshot()}),
        ));
    }
    let key = match (handle, arguments.string("run_id")?) {
        (Some(handle), None) => RunKey::RequestId(
            serde_json::from_value::<RequestId>(json!(handle)).map_err(|_| {
                // A job handle this MCP process does not hold: the process that started it
                // ended, or it is another one's.
                if handle.starts_with("correlation_") || handle.starts_with(lab::HANDLE_PREFIX) {
                    ToolError::usage(ActingctlCode::HandleUnknown)
                } else {
                    // Neither a Runtime request_id nor a job handle.
                    invalid_argument("handle", ArgumentProblem::NotRecognised)
                }
            })?,
        ),
        (None, Some(run_id)) => RunKey::RunId(
            serde_json::from_value::<RunId>(json!(run_id))
                .map_err(|_| invalid_argument("run_id", ArgumentProblem::NotRecognised))?,
        ),
        // Exactly one of handle and run_id.
        (Some(_), Some(_)) => {
            return Err(invalid_argument("handle", ArgumentProblem::Conflicting));
        }
        (None, None) => return Err(invalid_argument("handle", ArgumentProblem::Required)),
    };
    let connected = context.runtime.connect()?;
    loop {
        let status = connected
            .client
            .contained_run_status(key, RunStatusMode::Full)
            .map_err(|error| context.runtime.failure(&connected, &error))?;
        let may_change = matches!(
            status.state,
            ContainedRunState::NotFound | ContainedRunState::Admitted | ContainedRunState::Running
        );
        // A submit job of this process that failed before admission leaves no run.
        let job_failed_first = job.as_ref().is_some_and(|job| job.finished())
            && status.state == ContainedRunState::NotFound;
        if !may_change
            || job_failed_first
            || context.cancelled.load(Ordering::SeqCst)
            || Instant::now() + POLL_INTERVAL >= wait_until
        {
            let mut result = json!(status);
            if let Some(job) = &job {
                result["job"] = job.snapshot();
            }
            return Ok(ToolSuccess::new(result));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// ac_diagnose: `actingctl diagnose` over MCP (Workflow #389 MCP-7). Its arguments are the CLI's
/// flags (R2); the server's `--root` stands for the CLI's, and the generation `actingd suspended`
/// reads is found again at this call. The result is the CLI's stdout JSON, cut to the output
/// budget (`render_within_budget`).
pub(super) fn diagnose(context: &ToolContext<'_>, arguments: &Map<String, Value>) -> ToolOutcome {
    let arguments = Arguments::new(arguments, &["instance", "task_id", "since_unix_ms"])?;
    let request = DiagnoseRequest {
        instance: arguments.string("instance")?.map(str::to_owned),
        task_id: arguments
            .string("task_id")?
            .map(|text| {
                diagnose::parse_task_id(text)
                    .ok_or_else(|| invalid_argument("task_id", ArgumentProblem::NotRecognised))
            })
            .transpose()?,
        since_unix_ms: arguments.integer("since_unix_ms", 0, MAX_INTEGER)?,
    };
    let since_unix_ms = request
        .window(now_unix_ms()?)
        .map_err(|failure| diagnose_error(context, None, failure))?;
    let deadline = context
        .deadline
        .checked_sub(READ_RESERVE)
        .unwrap_or(context.deadline);
    // The read-only `actingd suspended` runs beside the Runtime reads.
    let report = diagnose::SuspensionReport::start(context.runtime.requested_root(), deadline);
    let location = context.runtime.locate();
    let connected = context.runtime.connect()?;
    let (diagnosis, query) = diagnose::read(
        &connected.client,
        &request,
        since_unix_ms,
        report,
        location.state_root.as_deref().ok(),
        deadline,
    )
    .map_err(|failure| diagnose_error(context, Some(&connected), failure))?;
    let binding = CursorBinding {
        session: context.session,
        generation: connected.generation,
    };
    diagnosis.render_within_budget(|kept| {
        diagnosis
            .errors_page(&query, binding, kept)
            .map_err(|failure| diagnose_error(context, Some(&connected), failure))
    })
}

/// A diagnosis failure as a tool error, with the CLI's outcomes (an argument as
/// `arguments_invalid`).
fn diagnose_error(
    context: &ToolContext<'_>,
    connected: Option<&Connected>,
    failure: Failure,
) -> ToolError {
    match &failure {
        Failure::Argument { argument, problem } => invalid_argument(argument, *problem),
        Failure::SinceOutOfRange { .. } | Failure::Selector { .. } => {
            ToolError::outcome("usage", failure.outcome())
        }
        Failure::QueryInvalid(_) | Failure::CursorEncode(_) | Failure::Clock => {
            ToolError::outcome("runtime", failure.outcome())
        }
        Failure::Runtime(error) => match connected {
            Some(connected) => context.runtime.failure(connected, error),
            None => super::runtime::client_error(error),
        },
    }
}

impl Diagnosis {
    /// Renders with every list whole, then cuts until the result fits the output budget.
    /// `errors_page` renders the errors page with its first `kept` rows. The parts that could
    /// not be read (Workflow #386 SR-6b, MCP-19b) go beside the result as its warnings.
    pub(super) fn render_within_budget(
        &self,
        errors_page: impl Fn(usize) -> Result<Value, ToolError>,
    ) -> ToolOutcome {
        let warnings = self
            .warnings
            .iter()
            .map(|warning| match warning {
                Warning::Outcome(outcome) => tools::warning(outcome),
                Warning::Runtime(error) => super::runtime::client_error(error).into_warning(),
            })
            .collect::<Vec<_>>();
        let mut kept = self.kept_all();
        loop {
            let result = self.render(&kept, errors_page(kept.errors)?, &tools::envelope);
            // With warnings the envelope is `{ok, result, warnings}`; this wrapper is a little
            // larger, so an answer it lets through always fits.
            let fits = if warnings.is_empty() {
                tools::fits_success(&result)
            } else {
                tools::fits_success(&json!({"result": &result, "warnings": &warnings}))
            };
            if fits {
                return Ok(ToolSuccess { result, warnings });
            }
            // Workflow #389 MCP-7 (command table §4.8): the groups go first, the fewest-count
            // groups of each list first (each list is ordered by count, the most frequent
            // first): failures_grouped, then runs_grouped. Then the clock-slot rows, from the
            // end of their list (the owed rows, then the deferred and expired ones, then the
            // refused and superseded ones, each oldest first): the full list stays in actingd
            // suspended and clock_slots_total points there (#372 K3, K4). Then the deferred rows
            // of the latest evaluation, from the end of their list (Workflow #386 SR-6b, MCP-19b:
            // ac_schedule answers them all). Then what another tool can read again: the errors page
            // (ac_events), then the runs, oldest first (ac_get_run / ac_events). The other rows
            // of the actingd suspended report have no other MCP route, so they go last:
            // repeating, lift, suspended, the suspended task packages (Workflow #386 SR-3), the
            // stopped remedy rounds (Workflow #411 B15 R7), then the report's warnings.
            if kept.failures_grouped > 0 {
                kept.failures_grouped -= 1;
            } else if kept.runs_grouped > 0 {
                kept.runs_grouped -= 1;
            } else if kept.clock_slots > 0 {
                kept.clock_slots -= 1;
            } else if kept.not_run > 0 {
                kept.not_run -= 1;
            } else if kept.errors > 0 {
                kept.errors -= 1;
            } else if kept.runs > 0 {
                kept.runs -= 1;
            } else if kept.repeating > 0 {
                kept.repeating -= 1;
            } else if kept.lift > 0 {
                kept.lift -= 1;
            } else if kept.suspended > 0 {
                kept.suspended -= 1;
            } else if kept.packs > 0 {
                kept.packs -= 1;
            } else if kept.remedies > 0 {
                kept.remedies -= 1;
            } else if kept.report_warnings > 0 {
                kept.report_warnings -= 1;
            } else {
                // Nothing left to cut: the result answers output_budget_exceeded.
                return Ok(ToolSuccess { result, warnings });
            }
        }
    }
}
