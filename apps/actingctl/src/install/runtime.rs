// SPDX-License-Identifier: AGPL-3.0-only

//! What `actingctl install` reads from and asks of the root's Runtime (Workflow #421 INST-2;
//! `contracts/install-cli.md`, "Formal stop, start and the hold"), behind one seam: its status
//! (the owner, the pauses, what each instance holds), the configuration facts (the run-time
//! instructions in effect and where each parameter comes from), the latest evaluation's plan, a
//! pause with its drain, a resume of exactly the pause it saw, the formal close, the report of
//! its own start, and the watchdog's manual start with its readiness probe. Production is
//! [`SystemRuntime`] over `actingcommand-runtime-client` and the watchdog's start; the tests
//! (`gate_inst2_*`) use a fake Runtime.

use crate::watchdog::decide::{LiveOwner, OwnerLock};
use crate::watchdog::observe;
use crate::watchdog::start::{self, StartedProcess, Target, TickSeams};
use actingcommand_contract::outcome::{Category, IoOp, Outcome};
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, CONFIG_PARAMETERS_FACT_PREFIX, EventActor, EventSource, FactScalar,
    FactValue, OwnerEpoch, RuntimeResult, ScheduleCandidateEligibility, SchedulingPauseExpectation,
    SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::Path;
use std::time::Duration;

/// One scheduling pause as a status read shows it: its reason (a pause reason name, never a
/// code) and its revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PauseSeen {
    pub(crate) reason: SchedulingPauseReason,
    pub(crate) revision: u64,
}

/// One registered instance as a status read shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct InstanceSeen {
    pub(crate) alias: String,
    pub(crate) pause: Option<PauseSeen>,
    /// A lease is held: a run (or a client) uses the device now.
    pub(crate) lease_active: bool,
    /// Lease requests waiting in the instance's queue.
    pub(crate) queued_requests: u32,
}

/// The answering Runtime as a status read shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RuntimeSeen {
    pub(crate) owner_epoch: String,
    pub(crate) pid: u32,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) global_pause: Option<PauseSeen>,
    pub(crate) instances: Vec<InstanceSeen>,
}

impl RuntimeSeen {
    pub(crate) fn instance(&self, alias: &str) -> Option<&InstanceSeen> {
        self.instances
            .iter()
            .find(|instance| instance.alias == alias)
    }
}

/// The configuration facts of the Runtime: each run-time instruction in effect, `(config_key,
/// value_json)` in key order, and where each parameter's value comes from (`explicit`,
/// `default`, `discovered`, `instruction`), by key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConfigSeen {
    pub(crate) instructions: Vec<(String, String)>,
    pub(crate) sources: BTreeMap<String, String>,
}

/// One instance of the latest policy evaluation: its earliest wake, its selected decision and
/// the due tasks that did not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlanRow {
    pub(crate) alias: String,
    pub(crate) next_wake_unix_ms: Option<u64>,
    pub(crate) selected: Option<Value>,
    pub(crate) due_tasks: Vec<String>,
}

/// The latest policy evaluation's plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct PlanSeen {
    pub(crate) evaluated_at_unix_ms: Option<u64>,
    pub(crate) instances: Vec<PlanRow>,
}

impl PlanSeen {
    pub(crate) fn row(&self, alias: &str) -> Option<&PlanRow> {
        self.instances.iter().find(|row| row.alias == alias)
    }
}

/// The runs an instance pause drained: `finished` ended on their own, `cancelled` were asked to
/// stop at their next checkpoint after the drain timeout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Drained {
    pub(crate) finished: u32,
    pub(crate) cancelled: u32,
}

/// A formal close that completed: what its wait observed, and the terminal outcome of the
/// closed start's log with the log, when one was found, with whether it is a stop (its category
/// `info` or `success`, as the watchdog judges a terminal outcome by its category alone).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Closed {
    pub(crate) wait: Value,
    pub(crate) terminal: Option<(String, String)>,
    pub(crate) stopped: bool,
}

/// What the root's Runtime is asked. Every method answers for the Runtime of `state_root`; a
/// failure is the client's or the watchdog's own outcome.
pub(crate) trait RuntimeLink {
    /// The answering Runtime; `None` when no Runtime holds the state root's owner lock.
    fn status(&self, state_root: &Path) -> Result<Option<RuntimeSeen>, Outcome>;
    /// The configuration facts.
    fn config(&self, state_root: &Path) -> Result<ConfigSeen, Outcome>;
    /// The latest policy evaluation's plan, with its due tasks.
    fn plan(&self, state_root: &Path) -> Result<PlanSeen, Outcome>;
    /// Pauses the instance `alias` with `reason`, its in-flight runs drained within
    /// `drain_timeout_ms` (then asked to stop at their next checkpoint, never killed).
    fn pause(
        &self,
        state_root: &Path,
        alias: &str,
        reason: &SchedulingPauseReason,
        drain_timeout_ms: u64,
    ) -> Result<Drained, Outcome>;
    /// INST-2 fix round 1 (review S2 M3): pauses every instance of `aliases` with `reason` at
    /// once, so no instance keeps dispatching while another drains; each answer by alias, in the
    /// order given. The default pauses one after the other (the fake Runtime overrides it to
    /// record the batch); the machine's Runtime pauses them in parallel.
    fn pause_many(
        &self,
        state_root: &Path,
        aliases: &[String],
        reason: &SchedulingPauseReason,
        drain_timeout_ms: u64,
    ) -> Vec<(String, Result<Drained, Outcome>)> {
        aliases
            .iter()
            .map(|alias| {
                (
                    alias.clone(),
                    self.pause(state_root, alias, reason, drain_timeout_ms),
                )
            })
            .collect()
    }
    /// Lifts exactly the pause of `alias` the caller saw: in `owner_epoch` at `revision`.
    fn resume(
        &self,
        state_root: &Path,
        alias: &str,
        owner_epoch: &str,
        revision: u64,
    ) -> Result<(), Outcome>;
    /// The formal close (`request-shutdown`), then the wait until the owner record closed and
    /// the process exited, at most `wait_ms`; nothing is killed. The terminal outcome is read
    /// from the logs of `root`.
    fn shutdown(&self, root: &Path, state_root: &Path, wait_ms: u64) -> Result<Closed, Outcome>;
    /// The Runtime's report of its own start; `None` from a build that does not know the read.
    fn start_report(&self, state_root: &Path) -> Result<Option<Value>, Outcome>;
    /// The watchdog's manual start of `target`, its output in `log` (created here).
    fn spawn(&self, target: &Target<'_>, log: &Path) -> Result<Box<dyn StartedProcess>, Outcome>;
    /// The readiness probe: the owner that holds the owner lock of `state_root` and answers.
    fn answering_owner(&self, state_root: &Path) -> Option<LiveOwner>;
}

/// The machine's Runtime.
pub(crate) struct SystemRuntime;

fn connect(state_root: &Path) -> Result<RuntimeClient, Outcome> {
    RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .map_err(|error| error.outcome())
}

/// An owner epoch's text, as runtime-info and the status spell it.
fn epoch_text(epoch: OwnerEpoch) -> String {
    match serde_json::to_value(epoch) {
        Ok(Value::String(text)) => text,
        _ => String::new(),
    }
}

fn scalar_text(value: Option<&FactScalar>) -> Option<String> {
    match value {
        Some(FactScalar::String(text)) => Some(text.clone()),
        _ => None,
    }
}

impl RuntimeLink for SystemRuntime {
    fn status(&self, state_root: &Path) -> Result<Option<RuntimeSeen>, Outcome> {
        let (lock, _) = observe::owner_journal(state_root)?;
        if lock != OwnerLock::Locked {
            return Ok(None);
        }
        let client = connect(state_root)?;
        let status = client.status().map_err(|error| error.outcome())?;
        let info = client.runtime_info();
        Ok(Some(RuntimeSeen {
            owner_epoch: epoch_text(status.owner_epoch()),
            pid: info.pid(),
            started_at_unix_ms: info.started_at_unix_ms(),
            global_pause: status.scheduling_pause().map(|pause| PauseSeen {
                reason: pause.pause_reason.clone(),
                revision: pause.revision,
            }),
            instances: status
                .instances()
                .iter()
                .map(|instance| InstanceSeen {
                    alias: instance.instance_alias().to_owned(),
                    pause: instance.pause().map(|pause| PauseSeen {
                        reason: pause.pause_reason.clone(),
                        revision: pause.revision,
                    }),
                    lease_active: instance.lease_active(),
                    queued_requests: instance.queued_request_count(),
                })
                .collect(),
        }))
    }

    fn config(&self, state_root: &Path) -> Result<ConfigSeen, Outcome> {
        let client = connect(state_root)?;
        let snapshot = client
            .runtime_fact_snapshot()
            .map_err(|error| error.outcome())?;
        let mut seen = ConfigSeen::default();
        for record in &snapshot.records {
            let FactValue::RecordList(rows) = &record.value else {
                continue;
            };
            if record.key == CONFIG_OVERRIDES_FACT_KEY {
                for row in rows {
                    if let (Some(key), Some(value)) = (
                        scalar_text(row.get("config_key")),
                        scalar_text(row.get("value_json")),
                    ) {
                        seen.instructions.push((key, value));
                    }
                }
            } else if record.key.starts_with(CONFIG_PARAMETERS_FACT_PREFIX) {
                for row in rows {
                    if let (Some(key), Some(source)) =
                        (scalar_text(row.get("key")), scalar_text(row.get("source")))
                    {
                        seen.sources.insert(key, source);
                    }
                }
            }
        }
        seen.instructions.sort();
        Ok(seen)
    }

    fn plan(&self, state_root: &Path) -> Result<PlanSeen, Outcome> {
        let client = connect(state_root)?;
        let explanation = client
            .schedule_explanation(None, true)
            .map_err(|error| error.outcome())?;
        Ok(PlanSeen {
            evaluated_at_unix_ms: explanation.evaluated_at_unix_ms,
            instances: explanation
                .instances
                .iter()
                .map(|entry| PlanRow {
                    alias: entry.instance_alias.clone(),
                    next_wake_unix_ms: entry.next_wake_unix_ms,
                    selected: entry.selected.as_ref().map(|selected| {
                        json!({
                            "task_id": selected.task_id,
                            "decision_id": selected.decision_id,
                            "dispatch_state": selected.dispatch_state,
                        })
                    }),
                    due_tasks: entry
                        .candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.eligibility == ScheduleCandidateEligibility::True
                        })
                        .map(|candidate| candidate.task_id.clone())
                        .collect(),
                })
                .collect(),
        })
    }

    fn pause_many(
        &self,
        state_root: &Path,
        aliases: &[String],
        reason: &SchedulingPauseReason,
        drain_timeout_ms: u64,
    ) -> Vec<(String, Result<Drained, Outcome>)> {
        // One connection and one pause per instance, all at once: each gate closes at once and
        // the drains run side by side. A panic in one is the installer's own defect and is
        // raised again (loud), never turned into a result.
        std::thread::scope(|scope| {
            let pending = aliases
                .iter()
                .map(|alias| {
                    let handle = scope
                        .spawn(move || self.pause(state_root, alias, reason, drain_timeout_ms));
                    (alias.clone(), handle)
                })
                .collect::<Vec<_>>();
            pending
                .into_iter()
                .map(|(alias, handle)| match handle.join() {
                    Ok(result) => (alias, result),
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        })
    }

    fn pause(
        &self,
        state_root: &Path,
        alias: &str,
        reason: &SchedulingPauseReason,
        drain_timeout_ms: u64,
    ) -> Result<Drained, Outcome> {
        let client = connect(state_root)?;
        match client
            .pause_scheduling(
                SchedulingPauseScope::Instance {
                    instance_alias: alias.to_owned(),
                },
                reason.clone(),
                drain_timeout_ms,
            )
            .map_err(|error| error.outcome())?
        {
            RuntimeResult::SchedulingPaused { drained, .. } => {
                Ok(drained.map_or_else(Drained::default, |summary| Drained {
                    finished: summary.finished,
                    cancelled: summary.cancelled,
                }))
            }
            _ => Ok(Drained::default()),
        }
    }

    fn resume(
        &self,
        state_root: &Path,
        alias: &str,
        owner_epoch: &str,
        revision: u64,
    ) -> Result<(), Outcome> {
        let owner_epoch =
            serde_json::from_value::<OwnerEpoch>(Value::String(owner_epoch.to_owned()))
                .map_err(|error| super::files::json_failure(&error))?;
        let client = connect(state_root)?;
        client
            .resume_scheduling_expected(
                SchedulingPauseScope::Instance {
                    instance_alias: alias.to_owned(),
                },
                SchedulingPauseExpectation {
                    owner_epoch,
                    revision,
                },
            )
            .map(drop)
            .map_err(|error| error.outcome())
    }

    fn shutdown(&self, root: &Path, state_root: &Path, wait_ms: u64) -> Result<Closed, Outcome> {
        let client = connect(state_root)?;
        let since = client.runtime_info().started_at_unix_ms();
        client.request_shutdown().map_err(|error| error.outcome())?;
        let target = client.runtime_info().shutdown_target();
        // Close the connection so the waiting installer leaves nothing for the Runtime to drain.
        drop(client);
        let wait = crate::shutdown_wait::wait_for_shutdown(
            state_root,
            &target,
            Duration::from_millis(wait_ms),
        )
        .map_err(|error| error.outcome())?;
        let mut directories = vec![root.to_path_buf(), root.join("watchdog")];
        directories.extend(observe::console_log_directory());
        let terminal = observe::terminal_after(&directories, since, Some(root))?;
        let stopped = terminal.as_ref().is_some_and(|terminal| {
            matches!(
                terminal.outcome.category(),
                Category::Info | Category::Success
            )
        });
        let terminal =
            terminal.map(|terminal| (terminal.outcome.code().as_str().to_owned(), terminal.log));
        Ok(Closed {
            wait,
            terminal,
            stopped,
        })
    }

    fn start_report(&self, state_root: &Path) -> Result<Option<Value>, Outcome> {
        let client = connect(state_root)?;
        match client.start_report() {
            Ok(report) => serde_json::to_value(report)
                .map(Some)
                .map_err(|error| super::files::json_failure(&error)),
            Err(error)
                if error
                    .outcome()
                    .is(actingcommand_runtime_client::ClientCode::RuntimeOperationUnsupported) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.outcome()),
        }
    }

    fn spawn(&self, target: &Target<'_>, log: &Path) -> Result<Box<dyn StartedProcess>, Outcome> {
        start::preflight(target)?;
        let entry = target.entry();
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(log)
            .map_err(|error| {
                crate::watchdog::io_failure(
                    crate::codes::ActingctlCode::LogCreateFailed,
                    log,
                    &error,
                    IoOp::Create,
                )
            })?;
        match start::system().spawn(target, &entry, file, log, false) {
            Ok((_, started)) => Ok(started),
            Err(attempt) => Err(attempt.result.err().unwrap_or_else(|| {
                Outcome::new(crate::codes::ActingctlCode::SpawnFailed).path(&entry)
            })),
        }
    }

    fn answering_owner(&self, state_root: &Path) -> Option<LiveOwner> {
        start::system().answering_owner(state_root)
    }
}
