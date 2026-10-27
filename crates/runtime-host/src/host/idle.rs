// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24: when an instance has no task, close the game inside the
//! emulator, and the startup package relaunches it when a task is due; her ruling of 10-25:
//! 「没有任务的时候直接关闭模拟器……任务结束或者接下来一段时间比如30分钟内没有任务的时候关闭应用程序」, two
//! options of one function; Workflow #386 comment 6393646823; IDLE-2, the owed wake): the host
//! side of the idle action.
//!
//! - **When.** At every policy cycle, right after its evaluation and before actingd admits its
//!   dispatches, and at every lease sweep, the idle check reads, for every registered physical
//!   instance, its idle keys in effect (`idle.*`, the instance's own or the section's), what it
//!   shows (its lease, its queue, a keyless hold of its admission guard, the scheduling pauses
//!   and a #403 fence through its routine gate, its emulator state, its startup package and
//!   configured emulator start) and the scheduler's latest plan for it: the latest evaluation's
//!   not-run explanation (`PolicyHost::schedule_explanation`, the evaluator's own due candidates
//!   and next wake of the instance; no other estimate). The pure planner (`crate::idle_action`)
//!   decides; this module carries its steps out. Nothing here blocks the policy cycle: the
//!   action runs on the instance's worker, the wake only queues claims.
//! - **The action** runs on an emulator-control claim of the instance's key (no new claim kind,
//!   as #411 A38's recheck reuses the self-check claim), which no dispatch can pass. On the key
//!   the instance is checked once more: a pause or a queued request that came meanwhile leaves it
//!   alone. `close_app` reads the application id the game's standard pack root declares for the
//!   instance's server (`<standard_packs.root>/<game>/applications.json`,
//!   `servers.<server>.application_id`; no game word in the code), checks that it is the
//!   instance's assigned application (the read-only foreground query), and stops it through the
//!   existing application lifecycle path on the claim's key (`command.received`,
//!   `command.validated`, `application.intent`, `application.completed`), as the ladder's
//!   application-restart rung does. `stop_emulator` is an emulator Stop on the claim's key
//!   through the existing emulator-control path (the fence on the key, the retained session's
//!   close, the provider Stop through the configured or internal control, never `api`, the
//!   endpoint back to pending), so the next start finds no dead retained session.
//! - **The wake** at the next due time minus the action's wake lead, or at once for an earlier
//!   due task (a fact-refresh reading too) or a switched action: after `close_app` the startup
//!   package is queued (`startup_package_scheduled`); after `stop_emulator` the configured
//!   emulator start (emu-watch's autostart claim, with its `emulator_autostart_queued`), whose
//!   start hands the key to the startup package as every configured start does. Queued at the
//!   cycle, the wake's claim holds the instance before the cycle's dispatch is admitted. An
//!   instance the action left owes its wake until it is queued: what holds it in between (a
//!   claim, a queued request, its admission guard held by the emulator watch's status read or a
//!   keyless observe, the action's own claim) neither cancels the wake nor lets the action run
//!   again before it, and a wake due while the instance is held is queued all the same, its
//!   claims waiting behind the holder. While an instance's emulator is held stopped, the
//!   emulator watch does not start it again.
//! - **Records.** One `instance_idle_action` record per action and per wake (`operation`
//!   `run_idle_action` or `queue_idle_wake`, `idle_action` naming the action), a
//!   `device_state_observed` record under the instance and the same report line. An instance
//!   whose action in effect cannot bring it back (no startup package, or for `stop_emulator` no
//!   configured emulator start) never acts and gets one `instance_idle_action_unsupported`
//!   warning (`member` naming what it lacks) per action in effect, not a silent skip.
//! - **Blackouts** (Workflow #386 SR-4a3, `scheduling-blackout.md`, "Complete silence"): a
//!   blackout that covers the instance holds its action as a scheduling pause does, so the
//!   Runtime closes or stops nothing inside it. A wake due inside it queues its claims as usual;
//!   the routine gate holds the startup package and the configured emulator start until the
//!   blackout's end, when the sweep's pump grants them.
//! - **Fail loud, one instance (#419).** A failed close or stop is recorded (the existing
//!   failure records, or `runtime.failed` with the failure) and goes through the one decision
//!   point (`contain_failure`), so it fails only this instance's action. Only a failure that is
//!   the Runtime's as a whole latches it. What the planner is told (`IdleEnd`):
//!   - a `close_app` that failed before its stop request (the declared application, the check
//!     that it is the assigned one, the foreground query, the request itself) and every failed
//!     `stop_emulator` were not performed: the instance is left as it is, no wake follows, and
//!     its next due task goes through the normal path (a stopped emulator is started again by
//!     the emulator watch);
//!   - a `close_app` whose stop request failed (`application_control` or after it) may have
//!     closed the application, so it is indeterminate: the instance owes its wake as after a
//!     close that succeeded, and no `instance_idle_action` record says the action ran.
//! - **Across a start** (IDLE-1P, #405; 10-27: a restart at 17:32 forgot a game closed since 14:35
//!   and its 17:54 wake, and the 18:01 task met the closed game): every change of the closed
//!   instances is recorded as the runtime fact `host.idle_state`, tagged so the next start loads
//!   it, and the start restores the planner from it before any instance is prepared, as the
//!   scheduling pauses are restored (`HostShared::restore_idle_state`). A closed instance whose
//!   wake has come, or whose plan shows a task due, is woken at the start's first check; one the
//!   start knows nothing about is restored as closed, never as open, and so is every instance
//!   when the record names another writer than the owner before this start (review M-2: an
//!   older build ran in between and its idle action was not recorded). The configured emulator
//!   start at start leaves an emulator the action holds stopped to its wake.
//! - **The scheduler's fact refresh** (IDLE-1P): the closed instances are an input of every
//!   evaluation (`HostShared::idle_closed_for_policy`); a reading of one waits for its next wake
//!   unless a task that needs it is due, and the plan the check reads keeps the reading apart
//!   from the tasks, so a reading alone never wakes a closed game.

use super::failure_scope::Contained;
use super::host_claims::{HostClaimWork, HostKey};
use super::input::ApplicationKey;
use super::lease::HostClaim;
use super::resource_fence::UnconfirmedResources;
use super::*;
use crate::codes::{HostCode, IdleCode, IdleLocation};
use crate::idle_action::{
    IdleEnd, IdlePlan, IdlePlanner, IdleRequirement, IdleStep, IdleView, KeptIdle, WakeWork,
};
use actingcommand_contract::codes::{SanitizationCode, SharedCode};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{EmulatorRunningState, OwnerEpoch};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_runtime_config::IdleAction;

/// The root file of a game's standard pack that declares its application per server.
const APPLICATIONS_FILE: &str = "applications.json";
/// How often the lease sweep runs the idle check between policy cycles (a wake's timer). Every
/// policy cycle runs it too.
const IDLE_SWEEP_INTERVAL_MS: u64 = 1_000;
/// IDLE-1P (#405): the runtime fact (scope Runtime, a record list, tagged so a start loads it)
/// that keeps the instances the idle action closed or is closing. Its first row names the writer,
/// `owner_epoch` alone (review M-2); every further row is one instance, `instance_alias`,
/// `action` (`close_app` or `stop_emulator`) and, when one is planned, `wake_at_unix_ms`. The idle
/// planner's changes record it; a start restores the planner from it when the owner before it
/// wrote it, and records it again.
const IDLE_STATE_FACT_KEY: &str = "host.idle_state";
/// The member of a `host.idle_state` writer row.
const KEPT_OWNER: &str = "owner_epoch";
/// The members of a `host.idle_state` instance row.
const KEPT_ALIAS: &str = "instance_alias";
const KEPT_ACTION: &str = "action";
const KEPT_WAKE: &str = "wake_at_unix_ms";

/// One instance's idle action, run on its worker under its emulator-control claim.
pub(super) struct PendingIdleAction {
    pub(super) instance_id: InstanceId,
    pub(super) instance_alias: String,
    action: IdleAction,
    next_due_unix_ms: Option<u64>,
    wake_at_unix_ms: Option<u64>,
    /// The game and the server the instance's policy identity names, when one does.
    identity: Option<(String, String)>,
    /// Shared by the claim's request and the action's own requests.
    causation_id: actingcommand_contract::IssuedCausationId,
}

/// IDLE-1P (#405): the owner epoch that wrote a `host.idle_state` record (review M-2) and the
/// closed instances it keeps, by alias; none when the record is not of the shape this build
/// writes (not the Runtime's own record list, no writer row first, a row without its alias, an
/// action other than `close_app` or `stop_emulator`, a wake of another type).
fn kept_idle(record: &RuntimeFactRecord) -> Option<(String, BTreeMap<String, KeptIdle>)> {
    if record.source != OriginModule::Runtime {
        return None;
    }
    let ContractFactValue::RecordList(rows) = &record.value else {
        return None;
    };
    // Review M-2: the writer row first, naming the owner epoch that wrote the record.
    let (writer, rows) = rows.split_first()?;
    let writer = match writer.get(KEPT_OWNER) {
        Some(FactScalar::String(epoch)) if writer.len() == 1 => epoch.clone(),
        _ => return None,
    };
    let mut kept = BTreeMap::new();
    for row in rows {
        let Some(FactScalar::String(alias)) = row.get(KEPT_ALIAS) else {
            return None;
        };
        let Some(FactScalar::String(token)) = row.get(KEPT_ACTION) else {
            return None;
        };
        let action = [IdleAction::CloseApp, IdleAction::StopEmulator]
            .into_iter()
            .find(|action| action.as_str() == token.as_str())?;
        let wake_at_unix_ms = match row.get(KEPT_WAKE) {
            None => None,
            Some(FactScalar::TimestampMs(at)) => Some(*at),
            Some(_) => return None,
        };
        kept.insert(
            alias.clone(),
            KeptIdle {
                action,
                wake_at_unix_ms,
            },
        );
    }
    Some((writer, kept))
}

/// Review M-2: `owner_epoch` as `host.idle_state` names it (its canonical text).
fn idle_epoch_text(owner_epoch: OwnerEpoch, location: IdleLocation) -> RuntimeHostResult<String> {
    match serde_json::to_value(owner_epoch) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(RuntimeHostError::fatal(
            SharedCode::RuntimeIdentifierIssueFailed,
            location,
            RuntimeErrorCode::RuntimeHalted,
        )),
    }
}

/// `instance_idle_action` for `alias` and `action` at `operation`.
fn idle_outcome(
    alias: &str,
    action: IdleAction,
    operation: IdleLocation,
    next_due_unix_ms: Option<u64>,
    wake_at_unix_ms: Option<u64>,
) -> Outcome {
    let outcome = Outcome::new(IdleCode::Taken)
        .operation(operation)
        .alias(alias)
        .idle_action(action.as_str());
    let outcome = match next_due_unix_ms {
        Some(due) => outcome.next_occurrence_unix_ms(due),
        None => outcome,
    };
    match wake_at_unix_ms {
        Some(at) => outcome.at_unix_ms(at),
        None => outcome,
    }
}

/// A failure of one instance's idle action, `code` at `operation`, with `detail` as its foreign
/// cause.
fn idle_error(
    code: HostCode,
    operation: IdleLocation,
    instance_id: InstanceId,
    detail: String,
) -> RuntimeHostError {
    let mut error = RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
        .with_native_detail(detail);
    error.lifecycle.instance_id = Some(instance_id);
    error
}

impl HostShared {
    /// The idle check of every registered physical instance now: at every policy cycle (after
    /// its evaluation) and, `from_sweep`, at most every second of the lease sweep, which skips
    /// it while the policy host is busy rather than wait for it. A step that fails for one
    /// instance is contained to it and the next instance goes on; only a failure the Runtime does
    /// not survive is the error.
    pub(super) fn check_idle_instances(&self, from_sweep: bool) -> RuntimeHostResult<()> {
        self.check_idle(from_sweep, None)
    }

    /// [`Self::check_idle_instances`]; `plans`, when given (tests), stand for the scheduler's
    /// latest plan of the instances they name, and an instance they leave out has none.
    fn check_idle(
        &self,
        from_sweep: bool,
        plans: Option<&BTreeMap<String, IdlePlan>>,
    ) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() || self.lifecycle_draining()? {
            return Ok(());
        }
        if from_sweep {
            let now_ms = self.monotonic_ms()?;
            if !lock(&self.idle, IdleLocation::Check)?.sweep_due(now_ms, IDLE_SWEEP_INTERVAL_MS) {
                return Ok(());
            }
        }
        let identities = lock(&self.policy_inputs, IdleLocation::Check)?
            .as_ref()
            .map(|inputs| {
                inputs
                    .instance_identities()
                    .map(|identity| {
                        (
                            identity.instance_id.to_owned(),
                            (identity.game_id.to_owned(), identity.server_id.to_owned()),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let instances = lock(&self.registered_instances, IdleLocation::Check)?
            .values()
            .filter(|instance| instance.provenance() == ExecutionBackendProvenance::PhysicalDevice)
            .map(|instance| {
                (
                    instance.instance_id(),
                    instance.instance_alias.clone(),
                    matches!(instance.emulator_state, EmulatorRunningState::On { .. })
                        && !instance.endpoint_pending(),
                )
            })
            .collect::<Vec<_>>();
        // Only the instances the configuration names have an idle action; a host started without
        // actingd's manifest names none and reads nothing more.
        let mut configured = Vec::with_capacity(instances.len());
        for (instance_id, alias, emulator_on) in instances {
            if let Some(config) = self.idle_settings(instance_id)? {
                configured.push((instance_id, alias, emulator_on, config));
            }
        }
        if configured.is_empty() {
            return Ok(());
        }
        // The scheduler's latest plan of each configured instance (IDLE-1P: its tasks and its
        // fact-refresh reading apart).
        let latest = {
            let read = |policy: &crate::policy_host::PolicyHost| {
                configured
                    .iter()
                    .map(|(_, alias, _, _)| (alias.clone(), policy.idle_plan(alias)))
                    .collect::<BTreeMap<_, _>>()
            };
            let policy = self.policy()?;
            if from_sweep {
                match policy.try_lock() {
                    Ok(policy) => read(&policy),
                    Err(TryLockError::WouldBlock) => return Ok(()),
                    Err(TryLockError::Poisoned(_)) => {
                        return Err(lock_poison_error(IdleLocation::Check));
                    }
                }
            } else {
                read(&*lock(policy, IdleLocation::Check)?)
            }
        };
        let now_unix_ms = self.runtime_clock_sample()?.unix_ms;
        let mut planner = lock(&self.idle, IdleLocation::Check)?;
        for (instance_id, alias, emulator_on, config) in configured {
            if self.fatal.is_shutdown_requested() {
                break;
            }
            let view = self.idle_view(instance_id, emulator_on)?;
            let plan = plans.map_or_else(
                || latest.get(&alias).copied().flatten(),
                |given| given.get(&alias).copied(),
            );
            let Some(step) = planner.check(&alias, &config, view, plan, now_unix_ms) else {
                continue;
            };
            let taken = match step {
                IdleStep::Act {
                    action,
                    next_due_unix_ms,
                    wake_at_unix_ms,
                } => self.request_idle_action(PendingIdleAction {
                    instance_id,
                    instance_alias: alias.clone(),
                    action,
                    next_due_unix_ms,
                    wake_at_unix_ms,
                    identity: identities.get(&alias).cloned(),
                    causation_id: self
                        .events
                        .issuer()
                        .mint_causation_id()
                        .map_err(|_| runtime_identifier_error())?,
                }),
                IdleStep::Wake { action, work } => {
                    self.queue_idle_wake(instance_id, &alias, action, work)
                }
                IdleStep::Unsupported { action, missing } => {
                    self.record_idle_unsupported(instance_id, &alias, action, missing)
                }
            };
            if let Err(error) = taken {
                if matches!(step, IdleStep::Act { .. }) {
                    // The action's claim was not queued: nothing was sent.
                    planner.finished(&alias, IdleEnd::NotPerformed);
                }
                self.contain_idle_failure(instance_id, error, self.idle_links(instance_id)?)?;
            }
        }
        // IDLE-1P: the closed instances as this check leaves them, kept for the next start.
        self.keep_idle_state(&mut planner)
    }

    /// IDLE-1P (#405): records `host.idle_state` when the planner's closed instances changed since
    /// it was last recorded. The caller holds the planner, so the records follow its changes in
    /// order.
    fn keep_idle_state(&self, planner: &mut IdlePlanner) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if self.idle_state_unkept_for_test {
            return Ok(());
        }
        let Some(closed) = planner.unkept() else {
            return Ok(());
        };
        self.record_idle_state(&closed)?;
        planner.mark_kept(closed);
        Ok(())
    }

    /// `host.idle_state`: the writer row naming this owner's epoch (review M-2), then one row per
    /// instance of `closed`, observed strictly after the previous record (a manual clock may not
    /// move between two).
    fn record_idle_state(&self, closed: &BTreeMap<String, KeptIdle>) -> RuntimeHostResult<()> {
        let writer = BTreeMap::from([(
            KEPT_OWNER.to_owned(),
            FactScalar::String(idle_epoch_text(self.owner_epoch, IdleLocation::Check)?),
        )]);
        let rows = std::iter::once(writer)
            .chain(closed.iter().map(|(alias, kept)| {
                let mut row = BTreeMap::from([
                    (KEPT_ALIAS.to_owned(), FactScalar::String(alias.clone())),
                    (
                        KEPT_ACTION.to_owned(),
                        FactScalar::String(kept.action.as_str().to_owned()),
                    ),
                ]);
                if let Some(at) = kept.wake_at_unix_ms {
                    row.insert(KEPT_WAKE.to_owned(), FactScalar::TimestampMs(at));
                }
                row
            }))
            .collect::<Vec<_>>();
        let now = self.runtime_clock_sample()?.unix_ms;
        let previous = lock(&self.runtime_facts, IdleLocation::Check)?
            .get(&RuntimeFactScope::Runtime, IDLE_STATE_FACT_KEY)
            .map(|record| record.observed_at_unix_ms);
        self.record_runtime_fact_with_event(RuntimeFactRecord {
            scope: RuntimeFactScope::Runtime,
            key: IDLE_STATE_FACT_KEY.to_owned(),
            value: ContractFactValue::RecordList(rows),
            observed_at_unix_ms: previous
                .map_or(now, |previous| now.max(previous.saturating_add(1))),
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map(|_| ())
    }

    /// IDLE-1P (#405): the instances the idle action closed or is closing, by alias, as the
    /// scheduler's fact refresh reads them: the reason a held-back reading's decision carries is
    /// `instance_idle_action` (`alias`, `idle_action`, the planned wake as `at_unix_ms` when there
    /// is one), and its code is the gate the waiting candidates' `fact_refresh_pending` names.
    pub(super) fn idle_closed_for_policy(
        &self,
    ) -> RuntimeHostResult<BTreeMap<String, actingcommand_policy::IdleClosed>> {
        let closed = lock(&self.idle, IdleLocation::Check)?.closed();
        Ok(closed
            .into_iter()
            .map(|(alias, kept)| {
                let outcome = Outcome::new(IdleCode::Taken)
                    .alias(alias.as_str())
                    .idle_action(kept.action.as_str());
                let outcome = match kept.wake_at_unix_ms {
                    Some(at) => outcome.at_unix_ms(at),
                    None => outcome,
                };
                let detail = format!(
                    "the idle action {} closed instance {alias}; its reading waits for its wake",
                    kept.action.as_str()
                );
                (
                    alias,
                    actingcommand_policy::IdleClosed {
                        gate: IdleCode::Taken.code(),
                        reason: actingcommand_policy::DecisionReason { outcome, detail },
                    },
                )
            })
            .collect())
    }

    /// IDLE-1P (#405): restores the idle planner at start, after the runtime facts and the
    /// scheduling pauses and before any instance is prepared, as the pauses are restored:
    /// - `host.idle_state` written by the owner before this one (`previous_owner_epoch`, its
    ///   writer row) names the instances the idle action closed, or was closing, when the Runtime
    ///   stopped: each registered physical instance the configuration names comes back idle after
    ///   its action, owing its kept wake;
    /// - without that record after an earlier start (the Runtime before did not keep it), with a
    ///   record another owner wrote (review M-2: a start that kept none ran in between, such as a
    ///   rollback's older build, whose own idle action may have closed what the record shows
    ///   open), or with a record this build does not read (`invalid_runtime_state_fact`,
    ///   recorded, and the start goes on), the start does not know what the action left: every
    ///   such instance whose action in effect could have run on it (not `none`, its startup
    ///   package configured, and for `stop_emulator` its emulator start) comes back idle with no
    ///   wake, closed rather than open, and the first plan that shows a task gives its wake;
    /// - on the ledger's first start nothing was closed.
    ///
    /// The restored instances are then recorded again under this owner's epoch, which is the
    /// ledger's record of the restore; a start whose configuration names no idle action and that
    /// finds no record keeps nothing.
    pub(super) fn restore_idle_state(
        &self,
        previous_owner_epoch: Option<OwnerEpoch>,
    ) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if self.idle_state_unkept_for_test {
            return Ok(());
        }
        let previous = previous_owner_epoch
            .map(|epoch| idle_epoch_text(epoch, IdleLocation::Restore))
            .transpose()?;
        let instances = lock(&self.registered_instances, IdleLocation::Restore)?
            .values()
            .filter(|instance| instance.provenance() == ExecutionBackendProvenance::PhysicalDevice)
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id()))
            .collect::<Vec<_>>();
        let startup_packages = self.startup_packages()?;
        let emulator_starts = self.emulator_autostart()?;
        // Each instance the configuration names, with its action in effect and whether that
        // action could have run on it.
        let mut configured = BTreeMap::new();
        for (alias, instance_id) in instances {
            if let Some(config) = self.idle_settings(instance_id)? {
                let wakeable = startup_packages.contains_key(&instance_id)
                    && (config.action != IdleAction::StopEmulator
                        || emulator_starts
                            .iter()
                            .any(|(configured, _)| *configured == instance_id));
                configured.insert(alias, (config.action, wakeable));
            }
        }
        let recorded = lock(&self.runtime_facts, IdleLocation::Restore)?
            .get(&RuntimeFactScope::Runtime, IDLE_STATE_FACT_KEY)
            .cloned();
        if configured.is_empty() && recorded.is_none() {
            return Ok(());
        }
        let kept = match recorded {
            Some(record) => {
                let kept = kept_idle(&record);
                if kept.is_none() {
                    // Fail Loud: the record is not of this build's shape; the start goes on with
                    // the instances' state unknown.
                    let error = RuntimeHostError::request(
                        SanitizationCode::InvalidRuntimeStateFact,
                        IdleLocation::Restore,
                        RuntimeErrorCode::InvalidRequest,
                    );
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::OperationCleanup,
                        RuntimeLifecycleFailure::Host(&error),
                        self.events.system_links()?,
                        None,
                    )?;
                }
                // Review M-2: only the owner before this one's record is current; one an earlier
                // owner wrote leaves every instance's state unknown.
                kept.and_then(|(writer, kept)| (previous.as_ref() == Some(&writer)).then_some(kept))
            }
            None if previous.is_some() => None,
            None => Some(BTreeMap::new()),
        };
        let mut planner = lock(&self.idle, IdleLocation::Restore)?;
        match kept {
            Some(kept) => {
                for (alias, kept) in kept {
                    if configured.contains_key(&alias) {
                        planner.restore(&alias, kept);
                    }
                }
            }
            None => {
                for (alias, (action, wakeable)) in &configured {
                    if *action != IdleAction::None && *wakeable {
                        planner.restore(
                            alias,
                            KeptIdle {
                                action: *action,
                                wake_at_unix_ms: None,
                            },
                        );
                    }
                }
            }
        }
        self.keep_idle_state(&mut planner)
    }

    /// What the instance shows to the idle check now. Read without waiting: a held admission
    /// guard (a keyless observe, probe or Lab look, or work starting on the instance) is work.
    fn idle_view(&self, instance_id: InstanceId, emulator_on: bool) -> RuntimeHostResult<IdleView> {
        // Workflow #386 SR-4a3: a blackout that covers the instance holds its action as a pause
        // does (complete silence); a wake's claims wait at the gate for its end.
        let gate = self.routine_gate(instance_id)?;
        let paused = gate.routine_held() || gate.blackout();
        let (leased, queued) = {
            let scheduler = lock(&self.scheduler, IdleLocation::Check)?;
            (
                scheduler.active_lease(instance_id).is_some(),
                scheduler.queued_count(instance_id) > 0,
            )
        };
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let manual = match instance_guard.try_lock() {
            Ok(_) => false,
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Poisoned(_)) => return Err(lock_poison_error(IdleLocation::Check)),
        };
        let startup_package = self.startup_packages()?.contains_key(&instance_id);
        let configured_start = self
            .emulator_autostart()?
            .iter()
            .any(|(configured, _)| *configured == instance_id);
        Ok(IdleView {
            leased,
            queued,
            manual,
            paused,
            emulator_on,
            app_wakeable: startup_package,
            emulator_wakeable: startup_package && configured_start,
        })
    }

    /// Queues the action's emulator-control claim (normal priority, the scheduler's default
    /// TTL); its worker runs the action once the key is granted.
    fn request_idle_action(&self, pending: PendingIdleAction) -> RuntimeHostResult<()> {
        let resolved = self
            .resolve_instance(&pending.instance_alias)
            .map_err(|failure| *failure.error)?;
        let connection_id = ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE)
            .map_err(|error| RuntimeHostError::scheduler(IdleLocation::Action, &error))?;
        let issuer = self.events.issuer();
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let request = RuntimeRequest::new(
            issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?,
            issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?,
            Some(pending.causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::AcquireLease {
                instance_alias: pending.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let lease_ttl_ms = lock(&self.scheduler, IdleLocation::Action)?
            .config()
            .lease_ttl_ms;
        let instance_alias = pending.instance_alias.clone();
        self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: &instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::EmulatorControl,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms,
            },
            HostClaimWork::IdleAction(Box::new(pending)),
        )
        .map_err(|failure| {
            if failure.poison_runtime {
                (*failure.error).into_fatal()
            } else {
                *failure.error
            }
        })
    }

    /// Fail Loud: the one `instance_idle_action_unsupported` (warning) of an instance whose
    /// action in effect, `action`, cannot bring it back: it lacks `missing`.
    fn record_idle_unsupported(
        &self,
        instance_id: InstanceId,
        alias: &str,
        action: IdleAction,
        missing: IdleRequirement,
    ) -> RuntimeHostResult<()> {
        self.record_idle(
            self.idle_links(instance_id)?,
            Outcome::new(IdleCode::Unsupported)
                .operation(IdleLocation::Check)
                .alias(alias)
                .idle_action(action.as_str())
                .member(missing.member()),
        )
    }

    /// The wake of an idle instance after `action`: its record, then `work` queued in order. The
    /// configured emulator start hands the key to the startup package itself, so nothing after
    /// it is queued here.
    fn queue_idle_wake(
        &self,
        instance_id: InstanceId,
        alias: &str,
        action: IdleAction,
        work: &[WakeWork],
    ) -> RuntimeHostResult<()> {
        let links = self.idle_links(instance_id)?;
        self.record_idle(
            links.clone(),
            idle_outcome(alias, action, IdleLocation::Wake, None, None),
        )?;
        for item in work {
            match item {
                WakeWork::EmulatorStart => {
                    let position = self
                        .emulator_autostart()?
                        .iter()
                        .position(|(configured, _)| *configured == instance_id)
                        .ok_or_else(|| {
                            RuntimeHostError::request(
                                HostCode::EmulatorAutostartInstanceUnknown,
                                IdleLocation::Wake,
                                RuntimeErrorCode::InvalidRequest,
                            )
                            .with_outcome_values(|outcome| outcome.alias(alias))
                        })?;
                    self.record_idle(
                        links.clone(),
                        Outcome::new(HostCode::EmulatorAutostartQueued)
                            .count(1_usize)
                            .alias(alias)
                            .idle_action(action.as_str()),
                    )?;
                    return self.request_watch_autostart(position, instance_id, alias);
                }
                WakeWork::StartupPackage => {
                    let resolved = self
                        .resolve_instance(alias)
                        .map_err(|failure| *failure.error)?;
                    let control_request_id = *self
                        .events
                        .issuer()
                        .mint_request_id()
                        .map_err(|_| runtime_identifier_error())?
                        .transport();
                    let pending = self
                        .prepare_startup_package(&resolved, links.clone(), control_request_id)
                        .map_err(|failure| *failure.error)?;
                    self.schedule_startup_package(pending)
                        .map_err(|failure| *failure.error)?;
                }
            }
        }
        Ok(())
    }

    /// A granted idle action's claim on its instance's worker. The instance is checked once
    /// more on the key; then the action runs, its end is reported to the planner, the key is
    /// released and the action recorded. A failure is contained to the instance (#419); only one the
    /// Runtime does not survive is the error, which ends the worker.
    pub(super) fn run_idle_action(
        &self,
        pending: &PendingIdleAction,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let ended = self.carry_out_idle_action(pending, key);
        #[cfg(test)]
        self.count_idle_action_end();
        ended
    }

    /// [`Self::run_idle_action`]'s work.
    fn carry_out_idle_action(
        &self,
        pending: &PendingIdleAction,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let alias = pending.instance_alias.as_str();
        let gate = self.routine_gate(pending.instance_id)?;
        let paused = gate.routine_held() || gate.blackout();
        let queued =
            lock(&self.scheduler, IdleLocation::Action)?.queued_count(pending.instance_id) > 0;
        if paused
            || queued
            || pending.action == IdleAction::None
            || self.fatal.is_shutdown_requested()
        {
            let kept = self.end_idle_action(|planner| planner.abandoned(alias));
            self.release_host_key(&key)?;
            // Review L-4: a failed `host.idle_state` record goes through the one decision point
            // for this instance, as on the action's end below (#419).
            return match kept {
                Ok(()) => Ok(()),
                Err(error) => {
                    let links = self.idle_links(pending.instance_id)?;
                    self.contain_idle_failure(pending.instance_id, error, links)
                }
            };
        }
        let claim = key.request.clone();
        let validated = claim.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
        // The stop ends its key's hold itself; the close leaves the key with the claim, which
        // releases it only once the planner knows the action's end, so no check meanwhile takes
        // the free instance for one that was never acted on.
        let mut stop_sent = false;
        let (ended, held) = match pending.action {
            IdleAction::StopEmulator => (
                self.idle_stop_emulator(pending, &validated, links.clone(), key),
                None,
            ),
            _ => (
                self.idle_close_app(pending, &key, links.clone(), &mut stop_sent),
                Some(key),
            ),
        };
        // IDLE-2: a close whose stop request was sent and failed may have closed the application,
        // so the instance owes its wake. Every other failure was not performed: a close that
        // failed before its stop request, and any failed stop_emulator (the emulator watch starts
        // a stopped emulator again).
        let end = match &ended {
            Ok(Ok(_)) => IdleEnd::Done,
            _ if stop_sent => IdleEnd::Indeterminate,
            _ => IdleEnd::NotPerformed,
        };
        let kept = self.end_idle_action(|planner| planner.finished(alias, end));
        if let Some(key) = &held
            && let Err(error) = self.release_host_key(key)
        {
            return self.contain_idle_failure(pending.instance_id, error, links);
        }
        if let Err(error) = kept {
            return self.contain_idle_failure(pending.instance_id, error, links);
        }
        let application_id = match ended {
            Ok(Ok(application_id)) => application_id,
            Ok(Err(recorded)) => {
                return self.contain_recorded_idle_failure(pending.instance_id, recorded, links);
            }
            Err(error) => return self.contain_idle_failure(pending.instance_id, error, links),
        };
        let outcome = idle_outcome(
            alias,
            pending.action,
            IdleLocation::Action,
            pending.next_due_unix_ms,
            pending.wake_at_unix_ms,
        );
        let outcome = match application_id {
            Some(application_id) => outcome.application_id(application_id),
            None => outcome,
        };
        self.record_idle(links, outcome)
    }

    /// IDLE-1P (#405): the planner learns how an action it asked for ended (`end`), and the
    /// closed instances are kept again for the next start.
    fn end_idle_action(&self, end: impl FnOnce(&mut IdlePlanner)) -> RuntimeHostResult<()> {
        let mut planner = lock(&self.idle, IdleLocation::Action)?;
        end(&mut planner);
        self.keep_idle_state(&mut planner)
    }

    /// `close_app` on the claim's `key`: the declared application, the check that it is the
    /// assigned one, then the existing application stop on the key, which stays with the claim
    /// whatever the stop's result. `Ok(Ok(application))` when it stopped; `Ok(Err(failure))`
    /// when it failed and `failure` is recorded; `Err` for a failure not recorded yet.
    /// `stop_sent` is set when the stop request is sent (`application_control`): a failure from
    /// there on leaves the application's state unknown.
    fn idle_close_app(
        &self,
        pending: &PendingIdleAction,
        key: &HostKey,
        links: EventLinksDraft,
        stop_sent: &mut bool,
    ) -> RuntimeHostResult<Result<Option<String>, RuntimeHostError>> {
        let alias = pending.instance_alias.as_str();
        let declared = match self.declared_application(pending) {
            Ok(declared) => declared,
            Err(error) => {
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&error),
                    links,
                    None,
                )?;
                return Ok(Err(error));
            }
        };
        // The read-only foreground query reports the instance's assigned application.
        let refusal = match self.execution()?.observe_foreground_application(alias) {
            Ok(observation) if observation.assigned == declared => None,
            Ok(observation) => Some(
                idle_error(
                    HostCode::InvalidApplicationIdentity,
                    IdleLocation::Action,
                    pending.instance_id,
                    format!(
                        "instance_alias={alias}; declared={declared}; assigned={}",
                        observation.assigned
                    ),
                )
                .with_outcome_values(|outcome| {
                    outcome.alias(alias).application_id(declared.as_str())
                }),
            ),
            Err(error) => {
                // As the foreground gate: the ADB baseline is gone, and an adb child or pipe left
                // unconfirmed fences the instance.
                self.invalidate_adb_baseline(pending.instance_id)?;
                let detail = format!("instance_alias={alias}; adb_failed={error}");
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    let mut unconfirmed = RuntimeHostError::fatal(
                        HostCode::ApplicationForegroundUnknown,
                        IdleLocation::Action,
                        RuntimeErrorCode::RuntimeHalted,
                    )
                    .with_native_detail(detail.clone());
                    unconfirmed.lifecycle.instance_id = Some(pending.instance_id);
                    unconfirmed.lifecycle.resource_quiescence =
                        Some(ResourceQuiescence::Unconfirmed);
                    let _fence = self.settle_unconfirmed_adb(
                        unconfirmed,
                        pending.instance_id,
                        links.clone(),
                    )?;
                }
                Some(idle_error(
                    HostCode::ApplicationForegroundUnknown,
                    IdleLocation::Action,
                    pending.instance_id,
                    detail,
                ))
            }
        };
        if let Some(refusal) = refusal {
            self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&refusal),
                links,
                None,
            )?;
            return Ok(Err(refusal));
        }
        let action = actingcommand_contract::ApplicationLifecycleAction::Stop;
        let resolved = self
            .resolve_instance(alias)
            .map_err(|failure| *failure.error)?;
        let issuer = self.events.issuer();
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let message = RuntimeRequest::new(
            issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?,
            issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?,
            Some(pending.causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: alias.to_owned(),
                holder_id: key.token.holder_id(),
                action,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let stop = message.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                IdleLocation::Action,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let stop_links = self
            .events
            .request_links(&stop, Some(pending.instance_id), None, None);
        for payload in [
            CommandPayloadDraft::received(action.event_action(), AuditInput::new()),
            CommandPayloadDraft::validated(
                action.event_action(),
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        ] {
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                stop_links.clone(),
                payload,
            )?;
        }
        // The key is the claim's and stays with it whatever the stop's result, as a ladder's.
        *stop_sent = true;
        match self.application_control(
            &stop,
            &key.token,
            action,
            key.connection_id,
            None,
            ApplicationKey::Ladder,
        ) {
            Ok(_) => Ok(Ok(Some(declared))),
            Err(mut failure) => {
                if let UnconfirmedResources::Fenced(fence) = self.settle_unconfirmed_resources(
                    &failure.error,
                    Some(pending.instance_id),
                    EventLinksDraft::default(),
                )? {
                    *failure.error = fence;
                    failure.poison_runtime = false;
                }
                if failure.poison_runtime {
                    return Err((*failure.error).into_fatal());
                }
                if failure.error.is_fatal() {
                    return Err(*failure.error);
                }
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    stop_links,
                    None,
                )?;
                Ok(Err(*failure.error))
            }
        }
    }

    /// `stop_emulator` on the claim's `key` through the existing emulator-control path
    /// (`drive_idle_stop`), which records its own failure pair and ends the key's hold.
    /// `Ok(Err(failure))` when the stop failed and `failure` is recorded.
    fn idle_stop_emulator(
        &self,
        pending: &PendingIdleAction,
        validated: &ValidatedRuntimeRequest<'_>,
        links: EventLinksDraft,
        key: HostKey,
    ) -> RuntimeHostResult<Result<Option<String>, RuntimeHostError>> {
        let resolved = match self.resolve_instance(&pending.instance_alias) {
            Ok(resolved) => resolved,
            Err(failure) => {
                self.release_host_key(&key)?;
                return Err(*failure.error);
            }
        };
        match self.drive_idle_stop(validated, &resolved, links, key) {
            Ok(()) => Ok(Ok(None)),
            Err(failure) if failure.poison_runtime => Err((*failure.error).into_fatal()),
            Err(failure) if failure.error.is_fatal() => Err(*failure.error),
            Err(failure) => Ok(Err(*failure.error)),
        }
    }

    /// The application id `<standard_packs.root>/<game>/applications.json` declares for the
    /// instance's server (`servers.<server>.application_id`); `invalid_application_identity`
    /// (with what is missing) when there is no root, no file, no such member, or the file does
    /// not read.
    fn declared_application(&self, pending: &PendingIdleAction) -> RuntimeHostResult<String> {
        let missing = |detail: String| {
            idle_error(
                HostCode::InvalidApplicationIdentity,
                IdleLocation::Action,
                pending.instance_id,
                detail,
            )
            .with_outcome_values(|outcome| {
                let outcome = outcome.alias(pending.instance_alias.as_str());
                match &pending.identity {
                    Some((game, server)) => outcome.game(game.as_str()).server(server.as_str()),
                    None => outcome,
                }
            })
        };
        let (game, server) = pending.identity.as_ref().ok_or_else(|| {
            missing("no policy identity names the instance's game and server".to_owned())
        })?;
        let root = self
            .standard_packs
            .root()
            .ok_or_else(|| missing("no standard_packs.root is configured".to_owned()))?;
        let path = root.join(game).join(APPLICATIONS_FILE);
        let bytes = fs::read(&path)
            .map_err(|error| missing(format!("{APPLICATIONS_FILE} does not read: {error}")))?;
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let value = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|error| missing(format!("{APPLICATIONS_FILE} is no JSON: {error}")))?;
        value
            .get("servers")
            .and_then(|servers| servers.get(server))
            .and_then(|server| server.get("application_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|application_id| !application_id.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                missing(format!(
                    "{APPLICATIONS_FILE} declares no servers.<server>.application_id for the instance's server"
                ))
            })
    }

    /// One instance's idle failure through the one decision point (#419): a failure of the
    /// instance or of its operation is recorded under `links` and ends only this action; one
    /// that is the Runtime's latches it and is the error.
    fn contain_idle_failure(
        &self,
        instance_id: InstanceId,
        error: RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        match self.contain_failure(error, Some(instance_id), links.clone())? {
            Contained::Latched(error) => Err(error),
            Contained::Fenced(failure) | Contained::Failed(failure) => self
                .append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure),
                    links,
                    None,
                ),
        }
    }

    /// One instance's idle failure that its own path recorded already (the stop's failure pair,
    /// the close's `runtime.failed`), through the one decision point (#419), which decides only
    /// its scope here: a failure of the instance (its fence records itself) or of its operation
    /// ends only this action; one that is the Runtime's latches it and is the error.
    fn contain_recorded_idle_failure(
        &self,
        instance_id: InstanceId,
        recorded: RuntimeHostError,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        match self.contain_failure(recorded, Some(instance_id), links)? {
            Contained::Latched(error) => Err(error),
            Contained::Fenced(_) | Contained::Failed(_) => Ok(()),
        }
    }

    /// Whether the idle action holds the instance's emulator stopped (`stop_emulator` running or
    /// done, its wake not come yet): the emulator watch does not start it.
    pub(super) fn idle_holds_emulator_stopped(&self, alias: &str) -> RuntimeHostResult<bool> {
        Ok(lock(&self.idle, IdleLocation::Check)?.emulator_held_stopped(alias))
    }

    /// One `device_state_observed` record carrying `outcome` under the instance (`links`), and
    /// the same report line.
    fn record_idle(&self, links: EventLinksDraft, outcome: Outcome) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::DeviceStateObserved {
                    outcome: outcome.clone(),
                },
                AuditInput::new(),
            ),
        )?;
        self.report_emulator_autostart(outcome)
    }

    /// The links of the idle check's own records: the instance under fresh system links.
    fn idle_links(&self, instance_id: InstanceId) -> RuntimeHostResult<EventLinksDraft> {
        Ok(self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id)))
    }

    /// Test only: one more idle action's worker ended, whatever its result.
    #[cfg(test)]
    fn count_idle_action_end(&self) {
        if let Ok(mut planner) = self.idle.lock() {
            planner.ended_for_test += 1;
        }
        self.idle_action_ended.notify_all();
    }

    /// Test only: the number of idle actions whose worker ended, once it reaches `ended`,
    /// waiting at most `wait`; `None` when it did not in time.
    #[cfg(test)]
    fn await_idle_action_ends(&self, ended: u64, wait: Duration) -> Option<u64> {
        let deadline = Instant::now() + wait;
        let mut planner = self.idle.lock().ok()?;
        loop {
            if planner.ended_for_test >= ended {
                return Some(planner.ended_for_test);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            planner = self.idle_action_ended.wait_timeout(planner, left).ok()?.0;
        }
    }
}

#[cfg(test)]
impl RuntimeHost {
    /// IDLE-1, tests: one idle check now, as at a policy cycle, with `plans` standing for the
    /// scheduler's latest plan of the instances they name (an instance they leave out has none).
    pub(crate) fn check_idle_instances_for_test(
        &self,
        plans: &BTreeMap<String, IdlePlan>,
    ) -> RuntimeHostResult<()> {
        self.shared_ref(IdleLocation::Check)?
            .check_idle(false, Some(plans))
    }

    /// IDLE-1, tests: the number of idle actions whose worker ended, once it reaches `ended`,
    /// waiting at most `wait`; `None` when it did not in time.
    pub(crate) fn await_idle_action_ends_for_test(
        &self,
        ended: u64,
        wait: Duration,
    ) -> Option<u64> {
        self.shared.as_deref()?.await_idle_action_ends(ended, wait)
    }
}
