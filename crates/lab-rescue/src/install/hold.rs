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
//!
//! Workflow #434 LAB-3 fix 1 and 2 (`contracts/install-cli.md`, "The Lab's rescue module"): a
//! hold of the Lab's rescue module never takes a pause it did not set, keeps a global pause, and
//! its release ([`release_own`]) resumes only the instances its pause step paused, each while its
//! pause still shows the hold's reason, in whatever owner epoch: the Runtime's restore numbers
//! every pause anew, so the revision does not survive a restart ([`OwnedHold`]).
//!
//! Workflow #421 INST-5b: the hold of the rescue module's `instance add` also sets, in its pause
//! step after the instance pauses, one global pause with the hold's reason ([`global_pause`]),
//! unless a global pause is in effect (then it sets none and leaves that one alone); the step
//! records it in its `own_pauses` (`global`), and [`release_own`] hands it back first, with the
//! hold's other pauses ([`own_global`]).

use super::Flags;
use super::host::Host;
use super::journal::S0;
use super::runtime::{ConfigSeen, Drained, PauseSeen, PlanSeen, Resumed, RuntimeLink, RuntimeSeen};
use crate::codes::{InstallCaveat, InstallLocation, InstallRefusal};
use actingcommand_contract::SchedulingPauseReason;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_runtime_config::IdleAction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// How often the hold reads the status while it waits for a busy instance (the readiness
/// probe's period, fixed by the contract).
const BUSY_POLL_MS: u64 = 500;

/// The hold's inputs: its reason and the other parties' pauses the person named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoldInputs {
    pub(crate) reason: SchedulingPauseReason,
    pub(crate) known: BTreeMap<String, String>,
    /// Workflow #434 LAB-3: the instances the hold may take, when it takes only some (the rescue
    /// module's `place` holds the instances bound to the bundle's game); `None` for every
    /// instance, as `install stop` and the composite update hold.
    pub(crate) only: Option<Vec<String>>,
    /// Workflow #421 INST-5b: the hold's pause step also sets one global pause with the hold's
    /// reason, unless a global pause is in effect ([`global_pause`]); only the rescue module's
    /// `instance add` asks for it, every other hold is `false`.
    pub(crate) global: bool,
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
    Ok(Some(HoldInputs {
        reason,
        known,
        only: None,
        global: false,
    }))
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

/// A record as JSON: these records hold only names and numbers, which always serialize (Workflow
/// #421 INST-5 fix 1: and those of `instance add`).
pub(crate) fn record_value(record: impl Serialize) -> Value {
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

/// The pause step's intent. Workflow #421 INST-5b: with the member `global: true` when the hold
/// sets a global pause too ([`global_of`]); every other hold's intent is written as before.
pub(crate) fn pause_intent(hold: &HoldInputs, held: &[String], drain_timeout_ms: u64) -> Value {
    let mut intent = record_value(PauseIntent {
        reason: hold.reason.clone(),
        instances: held.to_vec(),
        known: hold.known.clone(),
        drain_timeout_ms,
    });
    if hold.global {
        intent[GLOBAL_INTENT] = Value::Bool(true);
    }
    intent
}

/// Workflow #421 INST-5b: the member of a pause step's intent that asks for the hold's global
/// pause.
const GLOBAL_INTENT: &str = "global";

/// Workflow #421 INST-5b: whether a pause step's intent asks for the hold's global pause.
pub(crate) fn global_of(intent: &Value) -> bool {
    intent
        .get(GLOBAL_INTENT)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Workflow #421 INST-5b (the coordinator's ruling of 11-11): the global pause of a pause step
/// whose intent asks for one, after its instance pauses and before the close, so that every
/// instance, one the start registers anew too, stays held across the restart (the Runtime
/// persists the global pause and restores it at its start before any instance is prepared). A
/// global pause in effect with another reason, anyone's, is kept as it is and the step sets none
/// (`None`); one in effect with the hold's own reason is the step's own, set by an earlier run of
/// it; otherwise the step sets one with the hold's reason now. The revision of the step's own, in
/// the owner epoch `seen` shows. `seen` is the status the step checked last: none (no Runtime
/// answers) refuses `runtime_not_running`, and a pause the Runtime refuses `runtime_stop_failed`
/// with its failure as the cause; a hold that asked for its global pause never goes on without it.
pub(crate) fn global_pause(
    runtime: &dyn RuntimeLink,
    state_root: &Path,
    operation: InstallLocation,
    intent: &PauseIntent,
    seen: Option<&RuntimeSeen>,
) -> Result<Option<u64>, Outcome> {
    let Some(seen) = seen else {
        return Err(super::refusal(operation, InstallRefusal::RuntimeNotRunning));
    };
    match &seen.global_pause {
        Some(pause) if pause.reason == intent.reason => Ok(Some(pause.revision)),
        Some(_) => Ok(None),
        None => runtime
            .pause_global(state_root, &intent.reason, intent.drain_timeout_ms)
            .map(Some)
            .map_err(|cause| stop_failed(operation, cause)),
    }
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

/// A release step's evidence: the hold's reason and what the step did. Workflow #421 INST-5b:
/// `global_resumed` when it resumed the hold's own global pause (absent otherwise). Workflow #453
/// S3: `held_again`, the instances the Runtime held again instead of resuming them ([`HeldAgain`];
/// absent when none), which `resumed` does not name.
#[derive(Serialize)]
pub(crate) struct ReleaseEvidence<'a> {
    pub(crate) reason: &'a SchedulingPauseReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skipped: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resumed: Option<&'a [String]>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) global_resumed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) held_again: Option<&'a [HeldAgain]>,
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
/// on every instance of the intent, and on every other instance the pause S0 had or none; a
/// global pause refuses, unless `keep_global` (Workflow #434 LAB-3 fix 1: the rescue module keeps
/// and lists it, contracts/lab-two-entries.md §6). A Runtime that no longer runs keeps its
/// persisted pauses: the step is then done as far as it got. The drains and the status the step
/// checked last (none when no Runtime answered); a refusal carries the drains recorded so far.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_pause(
    host: &dyn Host,
    state_root: &Path,
    operation: InstallLocation,
    intent: &PauseIntent,
    s0_pauses: &BTreeMap<String, String>,
    mut drained: BTreeMap<String, Drained>,
    busy_wait_ms: u64,
    keep_global: bool,
) -> Result<(BTreeMap<String, Drained>, Option<RuntimeSeen>), PauseRefused> {
    match pause_all(
        host,
        state_root,
        operation,
        intent,
        s0_pauses,
        &mut drained,
        busy_wait_ms,
        keep_global,
    ) {
        Ok(seen) => Ok((drained, seen)),
        Err(refusal) => Err(PauseRefused { refusal, drained }),
    }
}

#[allow(clippy::too_many_arguments)]
fn pause_all(
    host: &dyn Host,
    state_root: &Path,
    operation: InstallLocation,
    intent: &PauseIntent,
    s0_pauses: &BTreeMap<String, String>,
    drained: &mut BTreeMap<String, Drained>,
    busy_wait_ms: u64,
    keep_global: bool,
) -> Result<Option<RuntimeSeen>, Outcome> {
    let runtime = host.runtime();
    let Some(seen) = runtime
        .status(state_root)
        .map_err(|cause| stop_failed(operation, cause))?
    else {
        return Ok(None);
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
    if (seen.global_pause.is_some() && !keep_global) || !unexpected.is_empty() {
        return Err(super::refusal(operation, InstallRefusal::RuntimeStopFailed)
            .count(unexpected.len())
            .aliases(unexpected));
    }
    Ok(Some(seen))
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

/// The evidence member of a pause step that holds an [`OwnPauses`] record.
pub(crate) const OWN_PAUSES: &str = "own_pauses";

/// Workflow #434 LAB-3 fix 1 (contracts/lab-two-entries.md §5: `release-hold` hands back only the
/// operation's own hold): the pauses a hold of the Lab's rescue module set, as its pause step
/// records them once they are set: in the owner epoch `owner_epoch`, each instance it paused with
/// `reason` and the revision that pause showed then. Fix 2: the release matches the reason and the
/// alias only ([`release_own`]); the epoch and the revisions are the record's evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OwnPauses {
    pub(crate) reason: SchedulingPauseReason,
    pub(crate) owner_epoch: String,
    pub(crate) revisions: BTreeMap<String, u64>,
    /// Workflow #421 INST-5b: the revision of the global pause the step set with `reason` (or an
    /// earlier run of it set), in `owner_epoch`; absent when the step set none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) global: Option<u64>,
}

impl OwnPauses {
    /// The pauses with `reason` that `seen` shows on `instances`.
    pub(crate) fn seen(
        seen: &RuntimeSeen,
        reason: &SchedulingPauseReason,
        instances: &[String],
    ) -> Self {
        Self {
            reason: reason.clone(),
            owner_epoch: seen.owner_epoch.clone(),
            revisions: seen
                .instances
                .iter()
                .filter(|instance| instances.contains(&instance.alias))
                .filter_map(|instance| {
                    let pause = instance
                        .pause
                        .as_ref()
                        .filter(|pause| &pause.reason == reason)?;
                    Some((instance.alias.clone(), pause.revision))
                })
                .collect(),
            global: None,
        }
    }

    /// Workflow #421 INST-5b: the record with the revision of the global pause the step set.
    pub(crate) fn with_global(mut self, revision: Option<u64>) -> Self {
        self.global = revision;
        self
    }

    /// Workflow #421 INST-5b: the global pause the record holds as the step's own: the owner
    /// epoch it was recorded in and its revision then.
    pub(crate) fn recorded_global(&self) -> Option<(String, u64)> {
        Some((self.owner_epoch.clone(), self.global?))
    }

    pub(crate) fn value(&self) -> Value {
        record_value(self)
    }

    /// The record a step's evidence holds, if any.
    pub(crate) fn of(evidence: Option<&Value>) -> Option<Self> {
        serde_json::from_value(evidence?.get(OWN_PAUSES)?.clone()).ok()
    }
}

/// Workflow #434 LAB-3 fix 1 and 2: a hold of the rescue module as the module's journals in the
/// root record it, by the reason their pause steps took: the instances those steps paused
/// (`paused`: each pause the Runtime accepted, the step's `drained`, and each instance its
/// [`OwnPauses`] record shows paused with the reason) and the instances they were to pause
/// (`intended`, their intents). Nothing else, the owner epoch above all, decides what is the
/// module's: the real Runtime's restore numbers every pause anew at revision 1 (a start that is not
/// installer-held, which neither `actingctl install` nor the module's start is), so neither the
/// epoch nor the revision survives a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OwnedHold {
    pub(crate) paused: BTreeSet<String>,
    pub(crate) intended: BTreeSet<String>,
    /// Workflow #421 INST-5b: the global pause a pause step of the hold recorded as its own
    /// ([`OwnPauses`]'s `global`): the owner epoch it was recorded in and its revision then.
    pub(crate) global: Option<(String, u64)>,
}

/// Workflow #421 INST-5b: the global pause `seen` shows that is the hold `reason`'s own: one a
/// pause step of the hold recorded (`owned.global`) that shows the hold's reason now; in the owner
/// epoch it was recorded in only at the revision recorded then, in a later one (the Runtime's
/// restore numbers every pause anew, revision 1) by its reason. Never another party's.
pub(crate) fn own_global<'a>(
    seen: &'a RuntimeSeen,
    reason: &SchedulingPauseReason,
    owned: &OwnedHold,
) -> Option<&'a PauseSeen> {
    let (epoch, revision) = owned.global.as_ref()?;
    let pause = seen.global_pause.as_ref()?;
    let recorded = seen.owner_epoch != *epoch || pause.revision == *revision;
    (&pause.reason == reason && recorded).then_some(pause)
}

/// Workflow #453 S3 (the coordinator's ruling 4 on board #453): an instance whose pause the release
/// asked the Runtime to lift and that the Runtime held again instead, as a Runtime with Workflow
/// #453 S2 answers for an entry that declares `start_paused` and that its start left unapplied
/// (`contracts/scheduling-pause.md`, "Pauses held at a first start"): its hold's pause is gone and
/// it stays paused with `instance.added`, as a newly added instance waits for a person's resume.
/// Recorded as S0 records a pause: the alias, the reason and the revision the Runtime answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HeldAgain {
    pub(crate) alias: String,
    pub(crate) reason: SchedulingPauseReason,
    pub(crate) revision: u64,
}

/// What a release of the rescue module's own hold did: the instances it resumed, and the
/// instances the hold was to pause that show its reason though its pause step did not pause them,
/// which are not the module's pauses and are left as they are. Workflow #421 INST-5b: and whether
/// it resumed the hold's own global pause (`global`, absent when not). Workflow #453 S3: and the
/// instances the Runtime held again instead of resuming them (`held_again`, absent when none),
/// which `resumed` does not name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct OwnRelease {
    pub(crate) resumed: Vec<String>,
    pub(crate) left: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) global: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) held_again: Vec<HeldAgain>,
}

/// Workflow #434 LAB-3 fix 1 and 2: the release of the rescue module's own hold `reason`, in the
/// running Runtime's owner epoch whatever the epoch the hold was set in. Each instance the hold's
/// pause steps paused (`owned.paused`) is resumed, by the pause it shows now (that epoch, that
/// revision), only while that pause has `reason`; one the hold was to pause that shows `reason`
/// though its step did not pause it is left (`left`). No other instance and no other pause, an
/// instance whose pause shows another reason, a global pause or another party's above all, is
/// ever touched. Workflow #421 INST-5b: first the hold's own global pause ([`own_global`]), by the
/// owner epoch and the revision the Runtime shows; no other global pause. Workflow #453 S3: an
/// instance the Runtime holds again at its resume ([`Resumed::HeldAgain`]) is recorded in
/// `held_again` with `instance.added` and the revision it answered, not in `resumed`. A failure as
/// [`release`]'s.
pub(crate) fn release_own(
    runtime: &dyn RuntimeLink,
    state_root: &Path,
    operation: InstallLocation,
    reason: &SchedulingPauseReason,
    owned: &OwnedHold,
) -> Result<OwnRelease, Outcome> {
    let seen = runtime
        .status(state_root)
        .map_err(|cause| {
            super::refusal(operation, InstallRefusal::RuntimeNotRunning)
                .joined(CauseRelation::CausedBy, cause)
        })?
        .ok_or_else(|| super::refusal(operation, InstallRefusal::RuntimeNotRunning))?;
    let mut released = OwnRelease::default();
    if let Some(pause) = own_global(&seen, reason, owned) {
        runtime
            .resume_global(state_root, &seen.owner_epoch, pause.revision)
            .map_err(|cause| {
                super::refusal(operation, InstallRefusal::RuntimeNotRunning)
                    .joined(CauseRelation::CausedBy, cause)
            })?;
        released.global = true;
    }
    for instance in &seen.instances {
        let Some(pause) = instance
            .pause
            .as_ref()
            .filter(|pause| &pause.reason == reason)
        else {
            continue;
        };
        if !owned.paused.contains(&instance.alias) {
            if owned.intended.contains(&instance.alias) {
                released.left.push(instance.alias.clone());
            }
            continue;
        }
        let resumed = runtime
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
        match resumed {
            Resumed::Lifted => released.resumed.push(instance.alias.clone()),
            Resumed::HeldAgain { revision } => released.held_again.push(HeldAgain {
                alias: instance.alias.clone(),
                reason: super::instance_add::pause_reason()?,
                revision,
            }),
        }
    }
    Ok(released)
}

/// Workflow #434 LAB-3 fix 1: the refusal `hold_not_owned` of `operation`: the hold `reason` is
/// not the rescue module's own (no journal of the module recorded it in its pause step), or the
/// instances `aliases` show it already (`down`), or show it though the module's pause step did
/// not pause them (`release-hold`, fix 2). Nothing was resumed or paused by it.
pub(crate) fn not_owned(
    operation: InstallLocation,
    reason: &SchedulingPauseReason,
    aliases: Vec<String>,
) -> Outcome {
    let refusal = super::refusal(operation, InstallRefusal::HoldNotOwned)
        .hold_reason(reason.as_str().to_owned());
    if aliases.is_empty() {
        refusal
    } else {
        refusal.count(aliases.len()).aliases(aliases)
    }
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
