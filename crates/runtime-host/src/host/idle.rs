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
//!   due task (a fact-refresh reading too) or a switched action. IDLE-WG (0.12.27): a due task is
//!   one that could be dispatched now; one a blackout or a closed activity window holds back
//!   (also behind a pause or a claim on the instance's key, such as a resume's reconnect) wakes
//!   nothing before those gates let it through. After `close_app` the startup
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
//!   when the record names another start than the last one that completed its settlement
//!   (review M-2: an older build ran in between and its idle action was not recorded); such an
//!   instance owes its wake at the start, so the first check wakes it once (review N-1). #405
//!   N-2 (0.12.25): the record names the start anchor (`host.contained_settlement`, the start
//!   event sequence of the start that wrote it), not the latest owner, so an owner that ran no
//!   Runtime start to its settlement in between (an offline ledger maintenance, a deploy's
//!   ledger check, a start that failed before its settlement) leaves it current. #405 O-2
//!   (0.12.25): a woken instance stays in the record, closed and woken, until its startup
//!   package has run, so a restart while the startup package waits queued in memory (behind a
//!   pause or a deploy's hold) wakes it again. The configured emulator start at start leaves an
//!   emulator the action holds stopped to its wake.
//! - **A startup package queued behind a pause** (W2-B, 0.12.26; review W1-6 M1): the same record
//!   keeps each instance whose startup package an emulator start (an operator's `start` /
//!   `restart`, or a configured start) queued behind a scheduling pause, from the hand-off until
//!   its claim is granted or an emulator control cancels it (`keep_startup_package`,
//!   `forget_startup_package`). A Runtime stop drops that claim (`lease.queue_disconnected`); the
//!   next start restores the record and, after the daemon-start preparation, queues the package
//!   again behind the restored pause (`requeue_startup_packages_on_start`), so it runs once after
//!   the resume.
//! - **The scheduler's fact refresh** (IDLE-1P): the closed instances are an input of every
//!   evaluation (`HostShared::idle_closed_for_policy`); a reading of one waits for its next wake
//!   unless a task that needs it is due, and the plan the check reads keeps the reading apart
//!   from the tasks, so a reading alone never wakes a closed game.

use super::contained_task::StartAnchors;
use super::failure_scope::Contained;
use super::host_claims::{HostClaimWork, HostKey};
use super::input::ApplicationKey;
use super::lease::HostClaim;
use super::resource_fence::UnconfirmedResources;
use super::*;
use crate::codes::{HostCode, HostLocation, IdleCode, IdleLocation};
use crate::idle_action::{
    IdleEnd, IdlePlan, IdlePlanner, IdleRequirement, IdleStep, IdleView, KeptIdle, KeptState,
    WakeWork,
};
use actingcommand_contract::codes::SanitizationCode;
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
/// `start_anchor` alone (#405 N-2: the start event sequence of the start that wrote it, as
/// `host.contained_settlement` names it; review M-2 named the owner epoch); every further row is
/// one instance, `instance_alias`, `action` (`close_app` or `stop_emulator`), when one is planned
/// `wake_at_unix_ms`, and #405 O-2 `woken` (true) for an instance woken whose startup package has
/// not run, its wake then the time it was woken. W2-B (0.12.26): after them, one row per instance
/// whose startup package an emulator start queued behind a scheduling pause, `instance_alias` and
/// `startup_package_queued` (true) alone. The idle planner's changes record it; a start restores
/// the planner from it when the last start that completed its settlement wrote it, and records it
/// again.
const IDLE_STATE_FACT_KEY: &str = "host.idle_state";
/// The member of a `host.idle_state` writer row.
const KEPT_ANCHOR: &str = "start_anchor";
/// The members of a `host.idle_state` instance row.
const KEPT_ALIAS: &str = "instance_alias";
const KEPT_ACTION: &str = "action";
const KEPT_WAKE: &str = "wake_at_unix_ms";
const KEPT_WOKEN: &str = "woken";
/// W2-B: the member of a `host.idle_state` row of a startup package queued behind a pause.
const KEPT_STARTUP_QUEUED: &str = "startup_package_queued";

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

/// IDLE-1P (#405): the start anchor that wrote a `host.idle_state` record (#405 N-2) and what it
/// keeps: the closed instances by alias and (W2-B) the instances whose startup package waits
/// queued behind a pause; none when the record is not of the shape this build writes (not the
/// Runtime's own record list, no writer row first, a row without its alias, an action other than
/// `close_app` or `stop_emulator`, a wake or a `woken` of another type, a woken row without its
/// wake, a startup row with another member or not `true`).
fn kept_idle(record: &RuntimeFactRecord) -> Option<(u64, KeptState)> {
    if record.source != OriginModule::Runtime {
        return None;
    }
    let ContractFactValue::RecordList(rows) = &record.value else {
        return None;
    };
    // #405 N-2: the writer row first, naming the start anchor of the start that wrote it.
    let (writer, rows) = rows.split_first()?;
    let writer = match writer.get(KEPT_ANCHOR) {
        Some(FactScalar::Integer(anchor)) if writer.len() == 1 => u64::try_from(*anchor).ok()?,
        _ => return None,
    };
    let mut kept = BTreeMap::new();
    let mut startup_queued = BTreeSet::new();
    for row in rows {
        let Some(FactScalar::String(alias)) = row.get(KEPT_ALIAS) else {
            return None;
        };
        // W2-B: a startup package queued behind a pause names the instance and itself alone.
        if let Some(queued) = row.get(KEPT_STARTUP_QUEUED) {
            if row.len() != 2 || *queued != FactScalar::Boolean(true) {
                return None;
            }
            startup_queued.insert(alias.clone());
            continue;
        }
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
        // #405 O-2: a woken instance's row names the time it was woken as its wake.
        let woken = match row.get(KEPT_WOKEN) {
            None => false,
            Some(FactScalar::Boolean(woken)) => *woken,
            Some(_) => return None,
        };
        if woken && wake_at_unix_ms.is_none() {
            return None;
        }
        kept.insert(
            alias.clone(),
            KeptIdle {
                action,
                wake_at_unix_ms,
                woken,
            },
        );
    }
    Some((
        writer,
        KeptState {
            closed: kept,
            startup_queued,
        },
    ))
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
        let Some(state) = planner.unkept() else {
            return Ok(());
        };
        // #405 N-2: the record names this start's anchor, which the start's restore bound. A start
        // whose settlement found no start event has none: Fail Loud, a record it could not name is
        // no record, and the start fails as on an anchor the settlement cannot read.
        let anchor = planner.start_anchor().ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::ContainedSettlementAnchorInvalid,
                IdleLocation::Check,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        self.record_idle_state(&state, anchor)?;
        planner.mark_kept(state);
        Ok(())
    }

    /// `host.idle_state`: the writer row naming this start's `anchor` (#405 N-2), then one row per
    /// closed instance of `state` and (W2-B) one per startup package it keeps queued behind a
    /// pause, observed strictly after the previous record (a manual clock may not move between
    /// two).
    fn record_idle_state(&self, state: &KeptState, anchor: u64) -> RuntimeHostResult<()> {
        let anchor = i64::try_from(anchor).map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::ContainedSettlementAnchorInvalid,
                IdleLocation::Check,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let writer = BTreeMap::from([(KEPT_ANCHOR.to_owned(), FactScalar::Integer(anchor))]);
        let rows = std::iter::once(writer)
            .chain(state.closed.iter().map(|(alias, kept)| {
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
                if kept.woken {
                    row.insert(KEPT_WOKEN.to_owned(), FactScalar::Boolean(true));
                }
                row
            }))
            .chain(state.startup_queued.iter().map(|alias| {
                BTreeMap::from([
                    (KEPT_ALIAS.to_owned(), FactScalar::String(alias.clone())),
                    (KEPT_STARTUP_QUEUED.to_owned(), FactScalar::Boolean(true)),
                ])
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
    /// #405 O-2: a woken instance is not one: its wake's claims are queued, and a reading of it
    /// waits behind them as before.
    pub(super) fn idle_closed_for_policy(
        &self,
    ) -> RuntimeHostResult<BTreeMap<String, actingcommand_policy::IdleClosed>> {
        let closed = lock(&self.idle, IdleLocation::Check)?.closed();
        Ok(closed
            .into_iter()
            .filter(|(_, kept)| !kept.woken)
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

    /// IDLE-1P (#405): restores the idle planner at start, after the runtime facts, the
    /// settlement of direct runs and the scheduling pauses and before any instance is prepared,
    /// as the pauses are restored:
    /// - `host.idle_state` written by the last start that completed its settlement (#405 N-2: its
    ///   writer row names `anchors.previous`, the start `host.contained_settlement` named before
    ///   this start; an owner in between that ran no Runtime start to its settlement, such as an
    ///   offline ledger maintenance, leaves it current) names the instances the idle action
    ///   closed, or was closing, when the Runtime stopped, and those woken whose startup package
    ///   had not run (#405 O-2): each registered physical instance the configuration names comes
    ///   back idle after its action, owing its kept wake (a woken one's: the time it was woken);
    /// - without that record after an earlier start (`previous_owner_epoch`: the Runtime before
    ///   did not keep it), with a record another start wrote (review M-2: a start that kept none
    ///   ran in between, such as a rollback's older build, whose own idle action may have closed
    ///   what the record shows open), or with a record this build does not read
    ///   (`invalid_runtime_state_fact`, recorded, and the start goes on; a record an earlier
    ///   build wrote under its owner epoch is one), the start does not know what the action left:
    ///   every such instance whose action in effect could have run on it (not `none`, its startup
    ///   package configured, and for `stop_emulator` its emulator start) comes back idle, closed
    ///   rather than open, owing its wake at the start's time (review N-1): the start's first idle
    ///   check wakes it once, its startup package waiting behind any pause, and the next plan
    ///   closes it again with its wake;
    /// - on the ledger's first start nothing was closed.
    ///
    /// W2-B (0.12.26): that current record also names the instances whose startup package an
    /// emulator start had queued behind a scheduling pause when the Runtime stopped (the Runtime
    /// stop dropped the claim, `lease.queue_disconnected`); each that is still registered with its
    /// startup package configured is kept so again, and the start queues its package again after
    /// the daemon-start preparation (`requeue_startup_packages_on_start`). One the record also
    /// keeps woken, whose wake this start restores, is not: its wake queues the startup package.
    ///
    /// The restored instances are then recorded again under this start's anchor
    /// (`anchors.current`), which is the ledger's record of the restore; a start whose
    /// configuration names no idle action and that finds no record keeps nothing.
    pub(super) fn restore_idle_state(
        &self,
        previous_owner_epoch: Option<OwnerEpoch>,
        anchors: StartAnchors,
    ) -> RuntimeHostResult<()> {
        #[cfg(test)]
        if self.idle_state_unkept_for_test {
            return Ok(());
        }
        // #405 N-2: every record from now on names this start's anchor.
        lock(&self.idle, IdleLocation::Restore)?.bind_start(anchors.current);
        let instances = lock(&self.registered_instances, IdleLocation::Restore)?
            .values()
            .filter(|instance| instance.provenance() == ExecutionBackendProvenance::PhysicalDevice)
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id()))
            .collect::<Vec<_>>();
        let startup_packages = self.startup_packages()?;
        let emulator_starts = self.emulator_autostart()?;
        // W2-B: the instances a kept startup package may still be queued for.
        let with_startup_package = instances
            .iter()
            .filter(|(_, instance_id)| startup_packages.contains_key(instance_id))
            .map(|(alias, _)| alias.clone())
            .collect::<BTreeSet<_>>();
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
                // #405 N-2 (review M-2): only the record of the last start that completed its
                // settlement is current; one another start wrote leaves every instance's state
                // unknown.
                kept.and_then(|(writer, kept)| (anchors.previous == Some(writer)).then_some(kept))
            }
            None if previous_owner_epoch.is_some() => None,
            None => Some(KeptState::default()),
        };
        let mut planner = lock(&self.idle, IdleLocation::Restore)?;
        match kept {
            Some(kept) => {
                for (alias, closed) in &kept.closed {
                    if configured.contains_key(alias) {
                        planner.restore(alias, *closed);
                    }
                }
                // W2-B: a startup package queued behind a pause when the Runtime stopped, unless
                // the instance's restored wake brings it back (#405 O-2).
                for alias in &kept.startup_queued {
                    let woken = configured.contains_key(alias)
                        && kept.closed.get(alias).is_some_and(|closed| closed.woken);
                    if with_startup_package.contains(alias) && !woken {
                        planner.restore_startup_queued(alias);
                    }
                }
            }
            None => {
                // Review N-1: each such instance owes its wake at the start, so the start's first
                // idle check brings it back once through its startup package (which waits behind
                // any pause or install hold, as after every wake), and the next plan closes it
                // again with its proper wake. Without it, an instance whose only work waits for a
                // status reading that expired meanwhile would get no wake at all.
                let started_at_unix_ms = self.runtime_clock_sample()?.unix_ms;
                for (alias, (action, wakeable)) in &configured {
                    if *action != IdleAction::None && *wakeable {
                        planner.restore(
                            alias,
                            KeptIdle {
                                action: *action,
                                wake_at_unix_ms: Some(started_at_unix_ms),
                                woken: false,
                            },
                        );
                    }
                }
            }
        }
        self.keep_idle_state(&mut planner)
    }

    /// W2-B (0.12.26; review W1-6 M1): keeps, in `host.idle_state`, whether the startup package of
    /// `instance_id` (`alias`) waits queued behind a scheduling pause: its startup claim is queued
    /// and a pause of the instance or the global pause holds it. Called while the claim cannot be
    /// granted yet: by an emulator start's hand-off before the control releases its key, and by a
    /// start's re-queue behind the restored pause. A record that fails is this instance's failure
    /// (#419): recorded through the one decision point; only a failure the Runtime does not
    /// survive is the error.
    pub(super) fn keep_startup_package(
        &self,
        instance_id: InstanceId,
        alias: &str,
    ) -> RuntimeHostResult<()> {
        let kept = (|| -> RuntimeHostResult<()> {
            let mut planner = lock(&self.idle, HostLocation::ScheduleStartupPackage)?;
            let paused = {
                let pauses = lock(&self.scheduling_pause, HostLocation::ReadRoutineGate)?;
                pauses.holds_global() || pauses.holds_instance(alias)
            };
            let queued = paused
                && !lock(&self.scheduler, HostLocation::ReadQueuedStartupClaims)?
                    .queued_of_kind(instance_id, ClaimKind::StartupPackage)
                    .is_empty();
            if !planner.note_startup_queued(alias, queued) {
                return Ok(());
            }
            self.keep_idle_state(&mut planner)
        })();
        self.contain_startup_keep(instance_id, kept)
    }

    /// W2-B: the startup package of `instance_id` waits queued no longer: its claim was granted
    /// (`run_startup_claim`), or an emulator control cancelled it (a stop, a failed control, a
    /// reconnect or self-check whose preparation failed and admitted no ladder), so the next start
    /// queues none. The record's failure is handled as in [`Self::keep_startup_package`].
    pub(super) fn forget_startup_package(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let kept = (|| -> RuntimeHostResult<()> {
            let alias = lock(
                &self.registered_instances,
                HostLocation::ReadInstanceRegistry,
            )?
            .get(&instance_id)
            .map(|instance| instance.instance_alias.clone());
            let Some(alias) = alias else {
                return Ok(());
            };
            let mut planner = lock(&self.idle, HostLocation::ScheduleStartupPackage)?;
            if !planner.note_startup_queued(&alias, false) {
                return Ok(());
            }
            self.keep_idle_state(&mut planner)
        })();
        self.contain_startup_keep(instance_id, kept)
    }

    /// W2-B: a failed record of a kept startup package, through the one decision point for
    /// `instance_id` (#419).
    fn contain_startup_keep(
        &self,
        instance_id: InstanceId,
        kept: RuntimeHostResult<()>,
    ) -> RuntimeHostResult<()> {
        match kept {
            Ok(()) => Ok(()),
            Err(error) => {
                self.contain_idle_failure(instance_id, error, self.idle_links(instance_id)?)
            }
        }
    }

    /// W2-B (0.12.26; review W1-6 M1): at start, after the daemon-start preparation, the startup
    /// package of each instance the restored `host.idle_state` keeps as queued behind a pause is
    /// queued again, as after a wake: its scheduling intent (`startup_package_scheduled` under the
    /// instance, a fresh causation) and its claim, which waits behind the restored pause and runs
    /// once after the resume. A pause no longer holding the instance (the resume came before the
    /// Runtime stop, and the claim never got the key) lets the claim be granted at once. The kept
    /// state then follows the claim (`keep_startup_package`). A failure is this instance's (#419):
    /// it is recorded and the start goes on; only a failure the Runtime does not survive is the
    /// error.
    pub(super) fn requeue_startup_packages_on_start(&self) -> RuntimeHostResult<()> {
        let queued = lock(&self.idle, IdleLocation::Restore)?.startup_queued();
        if queued.is_empty() {
            return Ok(());
        }
        let registered = lock(&self.registered_instances, IdleLocation::Restore)?
            .values()
            .map(|instance| (instance.instance_alias.clone(), instance.instance_id()))
            .collect::<BTreeMap<_, _>>();
        for alias in queued {
            let Some(&instance_id) = registered.get(&alias) else {
                continue;
            };
            if let Err(failure) = self.requeue_startup_package(instance_id, &alias) {
                let error = if failure.poison_runtime {
                    (*failure.error).into_fatal()
                } else {
                    *failure.error
                };
                self.contain_idle_failure(instance_id, error, self.idle_links(instance_id)?)?;
            }
            self.keep_startup_package(instance_id, &alias)?;
        }
        Ok(())
    }

    /// W2-B: one kept startup package queued again at start, through the path a wake's own takes
    /// (`prepare_startup_package`, then `schedule_startup_package`); an instance whose
    /// configuration no longer names one queues nothing.
    fn requeue_startup_package(
        &self,
        instance_id: InstanceId,
        alias: &str,
    ) -> Result<(), RequestFailure> {
        let resolved = self.resolve_instance(alias)?;
        let control_request_id = *self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?
            .transport();
        let links = self
            .idle_links(instance_id)
            .map_err(RequestFailure::poison_without_terminal)?;
        let pending = self.prepare_startup_package(&resolved, links, control_request_id)?;
        self.schedule_startup_package(pending).map(|_| ())
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
    /// it is queued here; #405 O-2 (review M-1): a wake's start that finds the emulator already
    /// running queues the startup package itself too.
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
                    return self.request_watch_autostart(position, instance_id, alias, true);
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
            let released = self.release_host_key(&key);
            return self
                .contain_idle_ends(pending.instance_id, released, kept, None)
                .unwrap_or(Ok(()));
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
        let released = held
            .as_ref()
            .map_or(Ok(()), |key| self.release_host_key(key));
        // Review N-5: both ends through the helper an abandoned action's take, so a failed record
        // is not lost behind a failed release.
        if let Some(ended) =
            self.contain_idle_ends(pending.instance_id, released, kept, Some(&links))
        {
            return ended;
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

    /// The two ends of an idle action: the key's release (`released`; `Ok` when the action's own
    /// path ended the key's hold, as `stop_emulator`'s does) and the `host.idle_state` record
    /// (`kept`). Each failure goes through the one decision point for this instance (#419), the
    /// release's first, under `links` (an action carried out: its claim's request links; none: the
    /// instance's idle links), so neither is lost when both fail: review L-4 for the record,
    /// review N-3 for an action not carried out (on its key it met a pause or work), review N-5
    /// for one carried out (`finished`). `None` when neither failed; otherwise the first failure
    /// the Runtime does not survive is the error.
    fn contain_idle_ends(
        &self,
        instance_id: InstanceId,
        released: RuntimeHostResult<()>,
        kept: RuntimeHostResult<()>,
        links: Option<&EventLinksDraft>,
    ) -> Option<RuntimeHostResult<()>> {
        let failures = [released, kept]
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if failures.is_empty() {
            return None;
        }
        let links = match links {
            Some(links) => links.clone(),
            None => match self.idle_links(instance_id) {
                Ok(links) => links,
                Err(error) => return Some(Err(error)),
            },
        };
        let mut ended = Ok(());
        for failure in failures {
            let contained = self.contain_idle_failure(instance_id, failure, links.clone());
            if ended.is_ok() {
                ended = contained;
            }
        }
        Some(ended)
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

    /// Review N-3, tests: the two ends of an abandoned idle action of `instance_alias`, its key's
    /// release failing with `release` and its `host.idle_state` record with `record` when given
    /// (each as the instance's request failure at `run_idle_action`).
    pub(crate) fn contain_abandoned_idle_ends_for_test(
        &self,
        instance_alias: &str,
        release: Option<HostCode>,
        record: Option<HostCode>,
    ) -> RuntimeHostResult<()> {
        let shared = self.shared_ref(IdleLocation::Action)?;
        let instance_id = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let ended = injected_idle_end(instance_alias, instance_id);
        shared
            .contain_idle_ends(instance_id, ended(release), ended(record), None)
            .unwrap_or(Ok(()))
    }

    /// Review N-5, tests: the two ends of an idle action carried out on `instance_alias`, as its
    /// `finished` path hands them over: its key's release failing with `release` and its
    /// `host.idle_state` record with `record` when given (each as the instance's request failure at
    /// `run_idle_action`), under its claim's links, which a fresh causation stands for here.
    /// Returns that causation, and whether the ends stopped the action before its own record (one
    /// of them failed).
    pub(crate) fn contain_finished_idle_ends_for_test(
        &self,
        instance_alias: &str,
        release: Option<HostCode>,
        record: Option<HostCode>,
    ) -> RuntimeHostResult<(actingcommand_contract::CausationId, bool)> {
        let shared = self.shared_ref(IdleLocation::Action)?;
        let instance_id = shared
            .resolve_instance(instance_alias)
            .map_err(|failure| *failure.error)?
            .instance_id();
        let causation = shared
            .events
            .issuer()
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let transport = *causation.transport();
        let links = shared.idle_links(instance_id)?.with_causation_id(causation);
        let ended = injected_idle_end(instance_alias, instance_id);
        match shared.contain_idle_ends(instance_id, ended(release), ended(record), Some(&links)) {
            Some(ended) => ended.map(|()| (transport, true)),
            None => Ok((transport, false)),
        }
    }
}

/// Review N-3 and N-5, tests: one end of an idle action of `instance_alias`, failing with `code`
/// as the instance's request failure at `run_idle_action` when one is given.
#[cfg(test)]
fn injected_idle_end(
    instance_alias: &str,
    instance_id: InstanceId,
) -> impl Fn(Option<HostCode>) -> RuntimeHostResult<()> + '_ {
    move |code| match code {
        Some(code) => Err(idle_error(
            code,
            IdleLocation::Action,
            instance_id,
            format!("instance_alias={instance_alias}; injected by the test"),
        )),
        None => Ok(()),
    }
}
