// SPDX-License-Identifier: AGPL-3.0-only

//! The hold, the formal close and the release of the hold (Workflow #421 INST-2;
//! `contracts/install-cli.md`, "Formal stop, start and the hold"; Alice's decision 75 of 10-27
//! 14:2x): when the installer stops the Runtime it holds and records every action first. S0 (the
//! owner, the pauses, the run-time instructions) is journalled; what the hold defers is recorded
//! before the first pause (per instance its queued requests, the run in flight, the latest plan's
//! selected decision, its due tasks and its wake); every instance S0 shows unpaused (and every
//! one already paused with the hold's own reason) is paused with `--hold`, all at once (fix
//! round 1, review S2 M3: no instance keeps dispatching while another drains), its run in flight
//! let reach its end within `install.stop.drain_timeout_ms` and then its next checkpoint (the
//! Runtime's own drain: never cut mid-action, never killed), and how each drain ended is
//! recorded. A pause the Runtime refuses while the instance has queued requests or a lease (an
//! operator's request waiting) is tried again once after they cleared, within
//! `install.stop.busy_wait_ms` (review S2 M4). The status must then show the hold on every
//! instance it took and, on every other instance, the pause S0 had or none (its owner lifted it
//! meanwhile: review S2 L5). Another party's pause is kept, and allowed only when
//! `--known-pause` names it; a global pause refuses. The close is `request-shutdown` and its
//! wait. The release resumes only the instances paused with exactly the hold's reason, each by
//! the pause it saw, and hands the deferred work back: the Runtime evaluates the deferred
//! dispatches again from its next cycle; nothing the installer recorded is sent again by the
//! installer, so no operation already sent is replayed. A run the drain stopped at a checkpoint
//! is not continued mid-run (its task is evaluated again from its start: the caveat
//! `runs_not_continued`), and the owed idle wakes of a build without a read for them are not
//! recorded (the caveat `idle_wakes_rederived`): both are the follow-up's (#421 S2 report).

use super::Flags;
use super::host::Host;
use super::journal::S0;
use super::runtime::{ConfigSeen, Drained, PauseSeen, PlanSeen, RuntimeLink, RuntimeSeen};
use crate::codes::{InstallCaveat, InstallLocation, InstallRefusal};
use actingcommand_contract::SchedulingPauseReason;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_runtime_config::IdleAction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// How often the hold reads the status while it waits for a busy instance (the readiness
/// probe's period, fixed by the contract).
const BUSY_POLL_MS: u64 = 500;

/// The hold's inputs: its reason and the other parties' pauses the person named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoldInputs {
    pub(crate) reason: SchedulingPauseReason,
    pub(crate) known: BTreeMap<String, String>,
}

/// `--hold <reason>` and every `--known-pause ALIAS=REASON`: the reason and each named pause of
/// the form a pause reason and an instance alias take; `None` without `--hold`
/// (`--known-pause` then is `actingctl_usage_invalid`, `required` `--hold`).
pub(crate) fn parse_hold(flags: &Flags<'_>) -> Result<Option<HoldInputs>, Outcome> {
    let known_words = flags.list("--known-pause");
    let Some(given) = flags.value("--hold") else {
        if known_words.is_empty() {
            return Ok(None);
        }
        return Err(flags
            .required("--hold")
            .err()
            .unwrap_or_else(|| super::unusable("--known-pause")));
    };
    let reason = SchedulingPauseReason::new(given).map_err(|_| super::unusable("--hold"))?;
    let mut known = BTreeMap::new();
    for word in known_words {
        let (alias, named) = word
            .split_once('=')
            .ok_or_else(|| super::unusable("--known-pause"))?;
        if actingcommand_contract::validate_instance_alias(alias).is_err()
            || actingcommand_contract::validate_scheduling_pause_reason(named).is_err()
            || named == reason.as_str()
            || known.insert(alias.to_owned(), named.to_owned()).is_some()
        {
            return Err(super::unusable("--known-pause"));
        }
    }
    Ok(Some(HoldInputs { reason, known }))
}

/// The instances the hold takes: those S0 shows unpaused, and those already paused with the
/// hold's own reason (a hold a failed step kept), by alias.
pub(crate) fn held_instances(seen: &RuntimeSeen, hold: &SchedulingPauseReason) -> Vec<String> {
    seen.instances
        .iter()
        .filter(|instance| {
            instance
                .pause
                .as_ref()
                .is_none_or(|pause| &pause.reason == hold)
        })
        .map(|instance| instance.alias.clone())
        .collect()
}

/// S0 of a hold: the owner epoch, every pause (`{global, instances: [{alias, reason,
/// revision}]}`) and every run-time instruction (`[{config_key, value_json}]`); the ledger head is
/// read once the Runtime stopped.
pub(crate) fn s0_of(seen: &RuntimeSeen, config: &ConfigSeen) -> S0 {
    S0 {
        head: None,
        epoch: Some(seen.owner_epoch.clone()),
        pauses: Some(pauses_value(seen)),
        instructions: Some(instructions_value(&config.instructions)),
    }
}

/// One pause as S0 records it: the instance (none for the global pause), its reason (a pause
/// reason name) and its revision.
#[derive(Serialize)]
struct PauseRecord<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<&'a str>,
    reason: &'a SchedulingPauseReason,
    revision: u64,
}

impl<'a> PauseRecord<'a> {
    fn of(alias: Option<&'a str>, pause: &'a PauseSeen) -> Self {
        Self {
            alias,
            reason: &pause.reason,
            revision: pause.revision,
        }
    }
}

/// Every pause as S0 records it.
#[derive(Serialize)]
struct PausesRecord<'a> {
    global: Option<PauseRecord<'a>>,
    instances: Vec<PauseRecord<'a>>,
}

/// A record as JSON: these records hold only names and numbers, which always serialize.
fn record_value(record: impl Serialize) -> Value {
    serde_json::to_value(record).unwrap_or(Value::Null)
}

/// The pauses as S0 records them.
pub(crate) fn pauses_value(seen: &RuntimeSeen) -> Value {
    record_value(PausesRecord {
        global: seen
            .global_pause
            .as_ref()
            .map(|pause| PauseRecord::of(None, pause)),
        instances: seen
            .instances
            .iter()
            .filter_map(|instance| {
                Some(PauseRecord::of(
                    Some(instance.alias.as_str()),
                    instance.pause.as_ref()?,
                ))
            })
            .collect(),
    })
}

/// An instance pause of another party as a guard lists it.
pub(crate) fn foreign_pause_value(alias: &str, pause: &PauseSeen) -> Value {
    record_value(PauseRecord::of(Some(alias), pause))
}

/// The instructions as S0 records them.
pub(crate) fn instructions_value(instructions: &[(String, String)]) -> Value {
    Value::Array(
        instructions
            .iter()
            .map(|(key, value)| json!({"config_key": key, "value_json": value}))
            .collect(),
    )
}

/// The instructions S0 recorded, `(config_key, value_json)` in key order.
pub(crate) fn instructions_of(value: &Value) -> Vec<(String, String)> {
    let mut instructions = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get("config_key")?.as_str()?.to_owned(),
                row.get("value_json")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    instructions.sort();
    instructions
}

/// The instance pauses S0 recorded, by alias: their reasons.
pub(crate) fn pause_reasons_of(value: &Value) -> BTreeMap<String, String> {
    value
        .get("instances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row.get("alias")?.as_str()?.to_owned(),
                row.get("reason")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// What the hold defers, recorded before its first pause: per instance it takes, its queued
/// lease requests, whether a run is in flight, and by the Runtime's latest plan its selected
/// decision, its due tasks and its earliest wake. The owed idle wakes have no read in the builds
/// this records from, so they are recorded as not read.
pub(crate) fn deferred_of(
    read_at_unix_ms: u64,
    seen: &RuntimeSeen,
    plan: Result<&PlanSeen, &Outcome>,
    held: &[String],
) -> Value {
    let rows = held
        .iter()
        .map(|alias| {
            let instance = seen.instance(alias);
            let entry = plan.ok().and_then(|plan| plan.row(alias));
            json!({
                "alias": alias,
                "queued_requests": instance.map_or(0, |instance| instance.queued_requests),
                "run_in_flight": instance.is_some_and(|instance| instance.lease_active),
                "selected": entry.and_then(|entry| entry.selected.clone()),
                "due_tasks": entry.map(|entry| entry.due_tasks.clone()).unwrap_or_default(),
                "next_wake_unix_ms": entry.and_then(|entry| entry.next_wake_unix_ms),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "read_at_unix_ms": read_at_unix_ms,
        "plan": match plan {
            Ok(plan) => json!({"evaluated_at_unix_ms": plan.evaluated_at_unix_ms}),
            Err(failure) => json!({"failure": crate::cli_line::warning(failure)}),
        },
        "instances": rows,
        "owed_idle_wakes": Value::Null,
        "owed_idle_wakes_read": "not_available",
    })
}

/// Whether a deferred record could not read the owed idle wakes.
pub(crate) fn idle_wakes_unread(deferred: Option<&Value>) -> bool {
    deferred
        .and_then(|deferred| deferred.get("owed_idle_wakes_read"))
        .and_then(Value::as_str)
        == Some("not_available")
}

/// How many of the `held` instances have an idle action other than `none` by the configuration
/// document `config`: the instance's own `idle.action`, else the section's, else the shipped
/// defaults file's (review S2 M6: their owed idle wakes are derived again, never silently).
pub(crate) fn idle_waking(config: &Value, held: &[String]) -> usize {
    let defaults =
        serde_json::from_str::<Value>(actingcommand_runtime_config::SHIPPED_DEFAULTS).ok();
    let action_of = |document: &Value| -> Option<String> {
        Some(document.get("idle")?.get("action")?.as_str()?.to_owned())
    };
    let section = action_of(config).or_else(|| defaults.as_ref().and_then(action_of));
    held.iter()
        .filter(|alias| {
            let own = config
                .get("instances")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|instance| {
                    instance.get("alias").and_then(Value::as_str) == Some(alias.as_str())
                })
                .and_then(action_of);
            own.or_else(|| section.clone())
                .is_some_and(|action| action != IdleAction::None.as_str())
        })
        .count()
}

/// The pause step's intent, written and read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PauseIntent {
    pub(crate) reason: SchedulingPauseReason,
    pub(crate) instances: Vec<String>,
    pub(crate) known: BTreeMap<String, String>,
    pub(crate) drain_timeout_ms: u64,
}

impl PauseIntent {
    pub(crate) fn of(intent: &Value) -> Option<Self> {
        serde_json::from_value(intent.clone()).ok()
    }
}

/// The pause step's intent.
pub(crate) fn pause_intent(hold: &HoldInputs, held: &[String], drain_timeout_ms: u64) -> Value {
    record_value(PauseIntent {
        reason: hold.reason.clone(),
        instances: held.to_vec(),
        known: hold.known.clone(),
        drain_timeout_ms,
    })
}

/// A release step's intent: the hold's reason, and whether an automatic rollback planned it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReleaseIntent {
    pub(crate) reason: SchedulingPauseReason,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) rollback: bool,
}

impl ReleaseIntent {
    pub(crate) fn value(reason: &SchedulingPauseReason, rollback: bool) -> Value {
        record_value(Self {
            reason: reason.clone(),
            rollback,
        })
    }
}

/// The hold as an answer reports it: its reason, whether it stays, and the release that lifted
/// it.
#[derive(Serialize)]
pub(crate) struct HoldReport<'a> {
    pub(crate) reason: &'a SchedulingPauseReason,
    pub(crate) kept: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) release: Option<&'a Value>,
}

impl HoldReport<'_> {
    pub(crate) fn value(&self) -> Value {
        record_value(self)
    }
}

/// A release step's evidence: the hold's reason and what the step did.
#[derive(Serialize)]
pub(crate) struct ReleaseEvidence<'a> {
    pub(crate) reason: &'a SchedulingPauseReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skipped: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resumed: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) handed_back: Option<&'a Value>,
}

impl ReleaseEvidence<'_> {
    pub(crate) fn value(&self) -> Value {
        record_value(self)
    }
}

/// `runtime_stop_failed` of `operation`, the step's own failure as its cause.
pub(crate) fn stop_failed(operation: InstallLocation, cause: Outcome) -> Outcome {
    super::refusal(operation, InstallRefusal::RuntimeStopFailed)
        .joined(CauseRelation::CausedBy, cause)
}

/// Waits until the instance `alias` has no queued request, at most `wait_ms` on the host's
/// clock; whether it cleared, and the queued requests it last showed. A lease is not waited for:
/// the pause's own drain waits for the run in flight (re-review S2 N2: the routine work the
/// scheduler dispatches right after an operator's run would otherwise outlast the wait).
fn wait_not_busy(
    host: &dyn Host,
    state_root: &Path,
    alias: &str,
    wait_ms: u64,
) -> Result<(bool, u32), Outcome> {
    let runtime = host.runtime();
    let deadline = host.now_unix_ms().saturating_add(wait_ms);
    loop {
        let seen = runtime.status(state_root)?;
        let Some(instance) = seen.as_ref().and_then(|seen| seen.instance(alias)) else {
            return Ok((false, 0));
        };
        if instance.queued_requests == 0 {
            return Ok((true, 0));
        }
        if host.now_unix_ms() >= deadline {
            return Ok((false, instance.queued_requests));
        }
        host.pause_ms(BUSY_POLL_MS);
    }
}

/// A pause step refused: its refusal, and how the drains that ran before it ended (their cut
/// runs are reported all the same: re-review S2 N3).
pub(crate) struct PauseRefused {
    pub(crate) refusal: Outcome,
    pub(crate) drained: BTreeMap<String, Drained>,
}

/// The pause step: every instance of the intent not yet paused with the hold's reason is paused,
/// all at once, and drained, how each drain ended recorded (`drained`, the earlier record kept
/// when the step runs again); a pause refused while its instance had a queued request is tried
/// again once after its queue cleared within `busy_wait_ms`. Then the status must show the hold
/// on every instance of the intent, and on every other instance the pause S0 had or none. A
/// Runtime that no longer runs keeps its persisted pauses: the step is then done as far as it
/// got. A refusal carries the drains recorded so far.
pub(crate) fn run_pause(
    host: &dyn Host,
    state_root: &Path,
    operation: InstallLocation,
    intent: &PauseIntent,
    s0_pauses: &BTreeMap<String, String>,
    mut drained: BTreeMap<String, Drained>,
    busy_wait_ms: u64,
) -> Result<BTreeMap<String, Drained>, PauseRefused> {
    match pause_all(
        host,
        state_root,
        operation,
        intent,
        s0_pauses,
        &mut drained,
        busy_wait_ms,
    ) {
        Ok(()) => Ok(drained),
        Err(refusal) => Err(PauseRefused { refusal, drained }),
    }
}

fn pause_all(
    host: &dyn Host,
    state_root: &Path,
    operation: InstallLocation,
    intent: &PauseIntent,
    s0_pauses: &BTreeMap<String, String>,
    drained: &mut BTreeMap<String, Drained>,
    busy_wait_ms: u64,
) -> Result<(), Outcome> {
    let runtime = host.runtime();
    let Some(seen) = runtime
        .status(state_root)
        .map_err(|cause| stop_failed(operation, cause))?
    else {
        return Ok(());
    };
    let pending = intent
        .instances
        .iter()
        .filter(|alias| {
            !seen
                .instance(alias)
                .and_then(|instance| instance.pause.as_ref())
                .is_some_and(|pause| pause.reason == intent.reason)
        })
        .cloned()
        .collect::<Vec<_>>();
    let refused_by = |alias: &str, cause: Outcome| {
        super::refusal(operation, InstallRefusal::RuntimeStopFailed)
            .alias(alias.to_owned())
            .joined(CauseRelation::CausedBy, cause)
    };
    let answers = runtime.pause_many(
        state_root,
        &pending,
        &intent.reason,
        intent.drain_timeout_ms,
    );
    // Every drain that ran is recorded before any refused pause is handled (re-review S2 N3).
    let mut refused = Vec::new();
    for (alias, answer) in answers {
        match answer {
            Ok(result) => {
                drained.insert(alias, result);
            }
            Err(cause) => refused.push((alias, cause)),
        }
    }
    for (alias, cause) in refused {
        // A queued operator request makes the Runtime refuse the pause (review S2 M4): wait for
        // the queue to clear, then pause it again once (re-review S2 N2).
        let (cleared, queued) = wait_not_busy(host, state_root, &alias, busy_wait_ms)
            .map_err(|status| stop_failed(operation, status))?;
        if !cleared {
            let refusal = refused_by(&alias, cause);
            return Err(if queued > 0 {
                refusal.count(queued)
            } else {
                refusal
            });
        }
        let result = runtime
            .pause(state_root, &alias, &intent.reason, intent.drain_timeout_ms)
            .map_err(|again| refused_by(&alias, again))?;
        drained.insert(alias, result);
    }
    let seen = runtime
        .status(state_root)
        .map_err(|cause| stop_failed(operation, cause))?
        .ok_or_else(|| super::refusal(operation, InstallRefusal::RuntimeNotRunning))?;
    let unexpected = seen
        .instances
        .iter()
        .filter(|instance| {
            let actual = instance.pause.as_ref().map(|pause| pause.reason.as_str());
            if intent.instances.contains(&instance.alias) {
                actual != Some(intent.reason.as_str())
            } else {
                // Another party's pause as S0 had it, or none: its owner may lift it meanwhile.
                actual.is_some() && actual != s0_pauses.get(&instance.alias).map(String::as_str)
            }
        })
        .map(|instance| instance.alias.clone())
        .collect::<Vec<_>>();
    if seen.global_pause.is_some() || !unexpected.is_empty() {
        return Err(super::refusal(operation, InstallRefusal::RuntimeStopFailed)
            .count(unexpected.len())
            .aliases(unexpected));
    }
    Ok(())
}

/// The drains as the pause step's evidence records them.
pub(crate) fn drained_value(drained: &BTreeMap<String, Drained>) -> Value {
    Value::Array(
        drained
            .iter()
            .map(|(alias, drained)| {
                json!({"alias": alias, "finished": drained.finished, "cancelled": drained.cancelled})
            })
            .collect(),
    )
}

/// The drains a pause step's evidence recorded.
pub(crate) fn drained_of(evidence: Option<&Value>) -> BTreeMap<String, Drained> {
    evidence
        .and_then(|evidence| evidence.get("drained"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let alias = row.get("alias")?.as_str()?.to_owned();
            let finished = u32::try_from(row.get("finished")?.as_u64()?).ok()?;
            let cancelled = u32::try_from(row.get("cancelled")?.as_u64()?).ok()?;
            Some((
                alias,
                Drained {
                    finished,
                    cancelled,
                },
            ))
        })
        .collect()
}

/// How many runs the drains stopped at a checkpoint.
pub(crate) fn runs_cut(drained: &BTreeMap<String, Drained>) -> u32 {
    drained.values().map(|drained| drained.cancelled).sum()
}

/// The close step: `request-shutdown` and its wait within `wait_ms`, nothing killed; done
/// already when no Runtime answers. Its evidence is what the wait observed and the closed
/// start's terminal outcome; a terminal outcome that is not a stop (its category neither `info`
/// nor `success`, as the watchdog judges it), or none found, is the caveat
/// `stop_terminal_unconfirmed` (the Runtime is gone all the same).
pub(crate) fn run_close(
    runtime: &dyn RuntimeLink,
    root: &Path,
    state_root: &Path,
    operation: InstallLocation,
    wait_ms: u64,
) -> Result<(Value, Option<Outcome>), Outcome> {
    let running = runtime
        .status(state_root)
        .map_err(|cause| stop_failed(operation, cause))?;
    if running.is_none() {
        return Ok((json!({"already_stopped": true}), None));
    }
    let closed = runtime
        .shutdown(root, state_root, wait_ms)
        .map_err(|cause| stop_failed(operation, cause))?;
    let caveat = match &closed.terminal {
        Some(_) if closed.stopped => None,
        Some((_, log)) => {
            Some(super::caveat(operation, InstallCaveat::StopTerminalUnconfirmed).path(log))
        }
        None => Some(super::caveat(
            operation,
            InstallCaveat::StopTerminalUnconfirmed,
        )),
    };
    Ok((
        json!({
            "wait": closed.wait,
            "terminal": closed.terminal.as_ref().map(|(code, _)| code),
            "log": closed.terminal.as_ref().map(|(_, log)| log),
        }),
        caveat,
    ))
}

/// The release of the hold: every instance paused with exactly `reason` resumed by the pause
/// it shows (its owner epoch and revision); another party's pause is never touched. The aliases
/// resumed, in alias order.
pub(crate) fn release(
    runtime: &dyn RuntimeLink,
    state_root: &Path,
    operation: InstallLocation,
    reason: &SchedulingPauseReason,
) -> Result<Vec<String>, Outcome> {
    let seen = runtime
        .status(state_root)
        .map_err(|cause| {
            super::refusal(operation, InstallRefusal::RuntimeNotRunning)
                .joined(CauseRelation::CausedBy, cause)
        })?
        .ok_or_else(|| super::refusal(operation, InstallRefusal::RuntimeNotRunning))?;
    let mut resumed = Vec::new();
    for instance in &seen.instances {
        let Some(pause) = instance
            .pause
            .as_ref()
            .filter(|pause| &pause.reason == reason)
        else {
            continue;
        };
        runtime
            .resume(
                state_root,
                &instance.alias,
                &seen.owner_epoch,
                pause.revision,
            )
            .map_err(|cause| {
                super::refusal(operation, InstallRefusal::RuntimeNotRunning)
                    .alias(instance.alias.clone())
                    .joined(CauseRelation::CausedBy, cause)
            })?;
        resumed.push(instance.alias.clone());
    }
    Ok(resumed)
}

/// The hand-back of the deferred work after the release at `released_at_unix_ms`: per
/// instance the hold recorded, whether it was resumed, each due task and the selected decision
/// it deferred with whether the Runtime's latest plan was evaluated since the release
/// (`re_evaluated`) or not yet (`pending_evaluation`), and the runs its drain stopped at a
/// checkpoint (`not_continued`: their tasks are evaluated again from their start).
pub(crate) fn hand_back(
    deferred: Option<&Value>,
    drained: &BTreeMap<String, Drained>,
    resumed: &[String],
    plan: Result<&PlanSeen, &Outcome>,
    released_at_unix_ms: u64,
) -> Value {
    let evaluated_since = plan.ok().is_some_and(|plan| {
        plan.evaluated_at_unix_ms
            .is_some_and(|evaluated| evaluated >= released_at_unix_ms)
    });
    let state = if evaluated_since {
        "re_evaluated"
    } else {
        "pending_evaluation"
    };
    let rows = deferred
        .and_then(|deferred| deferred.get("instances"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let alias = row.get("alias")?.as_str()?;
            let cut = drained.get(alias).map_or(0, |drained| drained.cancelled);
            let mut tasks = row
                .get("due_tasks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if let Some(task) = row
                .get("selected")
                .and_then(|selected| selected.get("task_id"))
                .and_then(Value::as_str)
                && !tasks.iter().any(|known| known == task)
            {
                tasks.push(task.to_owned());
            }
            Some(json!({
                "alias": alias,
                "resumed": resumed.iter().any(|known| known == alias),
                "tasks": tasks
                    .iter()
                    .map(|task| json!({"task_id": task, "state": state}))
                    .collect::<Vec<_>>(),
                "queued_requests": row.get("queued_requests"),
                "runs_stopped_at_checkpoint": cut,
                "runs": if cut > 0 { "not_continued" } else { "none_cut" },
            }))
        })
        .collect::<Vec<_>>();
    json!({
        "released_at_unix_ms": released_at_unix_ms,
        "plan": match plan {
            Ok(plan) => json!({"evaluated_at_unix_ms": plan.evaluated_at_unix_ms}),
            Err(failure) => json!({"failure": crate::cli_line::warning(failure)}),
        },
        "instances": rows,
        "owed_idle_wakes": "runtime_rederives",
    })
}
