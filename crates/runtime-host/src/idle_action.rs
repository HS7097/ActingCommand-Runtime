// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24 and her ruling of 10-25; Workflow #386 comment
//! 6393646823) and IDLE-2 (the owed wake, after v0.12.13 in production on 10-26): the idle
//! planner. It decides, for one instance at one idle check, whether the instance's idle action
//! runs now or whether an idle instance is brought back now, from what the instance shows
//! ([`IdleView`]), the scheduler's latest plan for it ([`IdlePlan`]: the latest policy
//! evaluation's due candidates and next wake for the instance, the evaluator's own; no other
//! estimate) and its idle keys in effect. It is pure: no clock, no device, no ledger. The host
//! (`host/idle.rs`) gathers the inputs at every policy cycle and every lease sweep, carries the
//! steps out and reports their ends ([`IdlePlanner::finished`]).
//!
//! The rule (ruling 41): close the application when there is no task, and bring it back through
//! the startup package when a task is due.
//!
//! - **The action** runs only when the instance is in no one's use (no lease, no queued request,
//!   no keyless manual, Lab or ladder work), no scheduling pause holds it, its emulator runs (the
//!   state both actions leave), it can be brought back (a startup package, and for
//!   `stop_emulator` its configured emulator start), and the plan, made after the instance was
//!   last seen in use, has no due task or fact-refresh reading and none closer than
//!   `idle.min_gap_ms`. It runs once: the instance is `Acting` until the host reports the end
//!   ([`IdleEnd`]). Done, it is
//!   `Idle` and owes its wake. Sent and failed with its result unknown (a `close_app` whose stop
//!   request failed: the application may be closed), it is `Idle` and owes its wake all the
//!   same. Not performed, it is `Failed`: it is left as it is, with no wake; only work on it
//!   again ends the wait, so the next due task goes through the normal path.
//! - **An action that cannot work** (Fail Loud): an instance that could not be brought back after
//!   its action in effect (no startup package, or for `stop_emulator` no configured emulator
//!   start) never acts, and the planner says so once per action in effect
//!   ([`IdleStep::Unsupported`], naming what is missing), not silently at every check.
//! - **An owed wake survives everything until it is queued.** The wake is an `Idle` instance's
//!   only exit. It comes:
//!   - at its time: the next due task's time minus the action's wake lead, the earlier of the one
//!     planned at the action and the one every later plan gives;
//!   - at once, when a plan shows a task of the instance due (a task that waits for a
//!     fact-refresh reading too: the wake's claim is queued before the reading's dispatch is
//!     admitted, so the reading does not meet a closed application);
//!   - at once, when the action in effect is no longer the one taken (the switch).
//!
//!   IDLE-1P (#405): a fact-refresh reading alone neither wakes an idle instance nor sets its
//!   wake: the scheduler holds a reading of a closed instance back until its next natural wake
//!   unless a task that needs it is due (`fact_refresh_pending`, gate `instance_idle_action`), so
//!   a status read never cold-starts a game.
//!
//!   Nothing in between cancels it: a claim, a queued request, a held admission guard (the
//!   emulator watch's status read, a keyless observe), the action's own claim, a person's run or
//!   a pause. A wake that comes due while the instance is held is queued all the same, and its
//!   claims wait behind the holder. `stop_emulator` wakes through the configured emulator start
//!   and the startup package, `close_app` through the startup package.
//! - **Never acted on again before the wake.** An `Idle` instance is never acted on. Work in
//!   between neither strands it (the owed wake follows at its time) nor cycles it: after the
//!   wake, the action runs again only once a plan made after the instance's last work shows a
//!   gap of at least `idle.min_gap_ms`, so a gap has at most one action and one wake.
//! - **A start keeps them** (IDLE-1P, #405; production 10-27: a restart at 17:32 forgot a game
//!   closed since 14:35 and its 17:54 wake, and its 18:01 task met the closed game). The host
//!   keeps the instances the action closed or is closing, each with its action and wake
//!   ([`IdlePlanner::closed`], the runtime fact `host.idle_state`), and the next start restores
//!   them as idle ([`IdlePlanner::restore`]): each owes its wake, which comes at once when its
//!   time has passed or a plan shows a task due. An action in flight at the stop, and an instance
//!   whose state the start does not know, are restored as closed too, never as open; the latter
//!   owes its wake at the start, so the start's first check wakes it once (review N-1).
//! - **Closed until the startup package has run** (#405 O-2, 0.12.25): a woken instance is not
//!   open yet. Its wake only queues claims (the startup package, after `stop_emulator` the
//!   configured emulator start first), which wait behind a pause or a deploy's hold in memory, so
//!   the instance stays closed and woken (`Woken`, kept as such) for as long as those claims hold
//!   it: until its startup package has run and the instance is free again. A start restores a
//!   woken instance as idle owing its wake at the time it was woken, so the start's first check
//!   queues its startup package again instead of finding a closed game counted as open. Once the
//!   instance is free, its wake's claims have ended (its startup package ran, or the claims ended
//!   without it: a configured start that found the emulator running, a claim an emulator control
//!   cancelled), and it is active again, as after every wake before.

use actingcommand_runtime_config::{IdleAction, IdleConfig};
use std::collections::BTreeMap;

/// What one instance shows to an idle check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct IdleView {
    /// A claim holds the instance's key.
    pub(crate) leased: bool,
    /// A request waits in the instance's queue.
    pub(crate) queued: bool,
    /// Keyless manual, direct, Lab or ladder work holds the instance (its admission guard).
    pub(crate) manual: bool,
    /// A scheduling pause holds the instance or the global gate.
    pub(crate) paused: bool,
    /// The instance's emulator runs, as the Runtime last read it.
    pub(crate) emulator_on: bool,
    /// The instance can be brought back after `close_app` (it has a startup package).
    pub(crate) app_wakeable: bool,
    /// The instance can be brought back after `stop_emulator` (it has a startup package and its
    /// emulator is configured to be started by the Runtime).
    pub(crate) emulator_wakeable: bool,
}

impl IdleView {
    const fn in_use(&self) -> bool {
        self.leased || self.queued || self.manual
    }

    /// What the instance lacks to be brought back after `action`; none when it can be (and for
    /// `none`, which never acts).
    const fn missing(&self, action: IdleAction) -> Option<IdleRequirement> {
        let wakeable = match action {
            IdleAction::None => return None,
            IdleAction::CloseApp => self.app_wakeable,
            IdleAction::StopEmulator => self.emulator_wakeable,
        };
        if wakeable {
            None
        } else if !self.app_wakeable {
            Some(IdleRequirement::StartupPackage)
        } else {
            Some(IdleRequirement::EmulatorStart)
        }
    }
}

/// What an instance lacks for its idle action to work: the wake needs it to bring the instance
/// back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleRequirement {
    /// Its startup package (both actions' wake runs it).
    StartupPackage,
    /// Its configured emulator start (the wake after `stop_emulator` starts the emulator).
    EmulatorStart,
}

impl IdleRequirement {
    /// The member of the instance's `instances[]` entry that configures it.
    pub(crate) const fn member(self) -> &'static str {
        match self {
            Self::StartupPackage => "startup_package",
            Self::EmulatorStart => actingcommand_runtime_config::START_EMULATOR,
        }
    }
}

/// The scheduler's plan for one instance, from its latest evaluated policy cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdlePlan {
    /// When the evaluation ran.
    pub(crate) evaluated_at_unix_ms: u64,
    /// A task of the instance is due: selected, or due and waiting. The fact-refresh reading is
    /// no task here (`reading_due_unix_ms`); a task that waits for it is.
    pub(crate) due_now: bool,
    /// The earliest wake of the instance's tasks in the evaluation; none: no task of the instance
    /// wakes.
    pub(crate) next_wake_unix_ms: Option<u64>,
    /// IDLE-1P (#405): when the instance's fact-refresh reading is due: the evaluation's time when
    /// it is due now, otherwise its wake; none when no reading is planned. It keeps an active
    /// instance from its action as a task does, but it never wakes an idle instance nor sets its
    /// wake: a reading alone is no reason for a cold start.
    pub(crate) reading_due_unix_ms: Option<u64>,
}

impl IdlePlan {
    /// The instance's next due task in this plan: the evaluation's time when a task is due,
    /// otherwise its next wake.
    pub(crate) const fn next_due_unix_ms(&self) -> Option<u64> {
        if self.due_now {
            Some(self.evaluated_at_unix_ms)
        } else {
            self.next_wake_unix_ms
        }
    }

    /// What keeps an active instance from its action: its next due task or reading, the
    /// earlier.
    fn next_work_unix_ms(&self) -> Option<u64> {
        match (self.next_due_unix_ms(), self.reading_due_unix_ms) {
            (Some(task), Some(reading)) => Some(task.min(reading)),
            (due, None) | (None, due) => due,
        }
    }
}

/// IDLE-1P (#405): an instance the idle action closed, or is closing, as the Runtime keeps it
/// across a start (`host.idle_state`): the action taken and the wake in effect (none: no task
/// wakes, or the first plan after a start gives it). #405 O-2 (0.12.25): `woken`, the instance was
/// woken and its startup package has not run yet; its wake is then the time it was woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeptIdle {
    pub(crate) action: IdleAction,
    pub(crate) wake_at_unix_ms: Option<u64>,
    pub(crate) woken: bool,
}

/// What a wake queues, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeWork {
    /// The instance's configured emulator start (the autostart claim).
    EmulatorStart,
    /// The instance's startup package.
    StartupPackage,
}

const STOP_EMULATOR_WAKE: &[WakeWork] = &[WakeWork::EmulatorStart, WakeWork::StartupPackage];
const CLOSE_APP_WAKE: &[WakeWork] = &[WakeWork::StartupPackage];

/// What a wake after `action` queues, in order.
pub(crate) const fn wake_work(action: IdleAction) -> &'static [WakeWork] {
    match action {
        IdleAction::StopEmulator => STOP_EMULATOR_WAKE,
        IdleAction::CloseApp => CLOSE_APP_WAKE,
        IdleAction::None => &[],
    }
}

/// One step the host takes for an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleStep {
    /// Run `action` now. The plan's next due time (none: no task wakes) and the planned wake.
    Act {
        action: IdleAction,
        next_due_unix_ms: Option<u64>,
        wake_at_unix_ms: Option<u64>,
    },
    /// Bring the instance back now after `action`: queue `work` in order.
    Wake {
        action: IdleAction,
        work: &'static [WakeWork],
    },
    /// Report once that `action`, the action in effect, cannot work on the instance: it lacks
    /// `missing`. The instance never acts while it does.
    Unsupported {
        action: IdleAction,
        missing: IdleRequirement,
    },
}

/// How the action the planner asked for ended, as the host reports it
/// ([`IdlePlanner::finished`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleEnd {
    /// The action was done: the instance owes its wake.
    Done,
    /// The action was not performed: it failed before its stop was sent, or it is a failed
    /// `stop_emulator`, which the host reports so (the emulator watch starts a stopped emulator
    /// again). The instance is left as it is, with no wake.
    NotPerformed,
    /// The action failed once its stop was sent (`close_app`'s stop request): what it left on the
    /// instance is not known, so the instance owes its wake as after one done.
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The instance is in use, or may become idle.
    Active,
    /// The action was asked for; its end is not reported yet.
    Acting {
        action: IdleAction,
        wake_at_unix_ms: Option<u64>,
    },
    /// The action was done, or sent with its result unknown: the instance owes its wake,
    /// `wake_at_unix_ms` (the wake planned at the action, or an earlier one a later plan gave).
    /// The wake is its only exit.
    Idle {
        action: IdleAction,
        wake_at_unix_ms: Option<u64>,
    },
    /// #405 O-2: the wake after `action` was queued at `woken_at_unix_ms` and its claims still
    /// hold the instance: closed and woken until its startup package has run and the instance is
    /// free. Never acted on, never woken again meanwhile.
    Woken {
        action: IdleAction,
        woken_at_unix_ms: u64,
    },
    /// The action was not performed: the instance is left as it is until work holds it again.
    Failed,
}

#[derive(Debug, Clone, Copy)]
struct InstanceIdle {
    phase: Phase,
    /// The last check that found the instance in use: an action waits for a plan made after it.
    in_use_at_unix_ms: Option<u64>,
    /// The action in effect last reported as unable to work on the instance: it is reported
    /// once, and again only for another action.
    unsupported: Option<IdleAction>,
}

/// The idle state of every instance, by alias. IDLE-1P (#405): the host keeps the closed
/// instances ([`Self::closed`]) and a start restores them ([`Self::restore`]); every other
/// instance begins active.
#[derive(Debug, Default)]
pub(crate) struct IdlePlanner {
    instances: BTreeMap<String, InstanceIdle>,
    /// IDLE-1P: the closed instances the host kept last; none before the first record.
    kept: Option<BTreeMap<String, KeptIdle>>,
    /// #405 N-2 (0.12.25): the start anchor the host keeps the closed instances under: this
    /// start's start event sequence, which its settlement records as `host.contained_settlement`;
    /// none before the start's restore binds it.
    start_anchor: Option<u64>,
    /// When the lease sweep's idle check last ran (monotonic milliseconds).
    last_sweep_ms: Option<u64>,
    /// Test only: how many idle actions' workers ended, whatever their result.
    #[cfg(test)]
    pub(crate) ended_for_test: u64,
}

impl IdlePlanner {
    /// One idle check of the instance `alias` at `now_unix_ms`, with its idle keys in effect
    /// `config`, what it shows `view` and the scheduler's latest plan for it `plan` (none before
    /// the first evaluated cycle). The step to take, if any.
    pub(crate) fn check(
        &mut self,
        alias: &str,
        config: &IdleConfig,
        view: IdleView,
        plan: Option<IdlePlan>,
        now_unix_ms: u64,
    ) -> Option<IdleStep> {
        let entry = self
            .instances
            .entry(alias.to_owned())
            .or_insert(InstanceIdle {
                phase: Phase::Active,
                in_use_at_unix_ms: None,
                unsupported: None,
            });
        let phase = entry.phase;
        match phase {
            // The host reports the action's end (`finished`, `abandoned`) before the instance is
            // free again.
            Phase::Acting { .. } => None,
            Phase::Active | Phase::Failed if view.in_use() => {
                entry.in_use_at_unix_ms = Some(now_unix_ms);
                entry.phase = Phase::Active;
                None
            }
            Phase::Failed => None,
            Phase::Active => decide(entry, config, view, plan, now_unix_ms),
            // #405 O-2: the wake's claims (its startup package) still hold the instance.
            Phase::Woken { .. } if view.in_use() => {
                entry.in_use_at_unix_ms = Some(now_unix_ms);
                None
            }
            // Free again: the wake's claims have ended, the startup package's run with them.
            Phase::Woken { .. } => {
                entry.phase = Phase::Active;
                decide(entry, config, view, plan, now_unix_ms)
            }
            // IDLE-2: a hold neither ends an idle instance's wait nor cancels its wake.
            Phase::Idle {
                action,
                wake_at_unix_ms,
            } => {
                if view.in_use() {
                    // After the wake, an action waits for a plan made after this work.
                    entry.in_use_at_unix_ms = Some(now_unix_ms);
                }
                wake(entry, config, action, wake_at_unix_ms, plan, now_unix_ms)
            }
        }
    }

    /// Whether the lease sweep's idle check is due at `now_ms` (monotonic): at most once per
    /// `interval_ms`. A due check is counted.
    pub(crate) fn sweep_due(&mut self, now_ms: u64, interval_ms: u64) -> bool {
        if self
            .last_sweep_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < interval_ms)
        {
            return false;
        }
        self.last_sweep_ms = Some(now_ms);
        true
    }

    /// The host's report of the end of the action it was asked for: one done, or sent with its
    /// result unknown, leaves the instance idle and owing its planned wake; one not performed
    /// leaves it as it is.
    pub(crate) fn finished(&mut self, alias: &str, end: IdleEnd) {
        if let Some(entry) = self.instances.get_mut(alias)
            && let Phase::Acting {
                action,
                wake_at_unix_ms,
            } = entry.phase
        {
            entry.phase = match end {
                IdleEnd::Done | IdleEnd::Indeterminate => Phase::Idle {
                    action,
                    wake_at_unix_ms,
                },
                IdleEnd::NotPerformed => Phase::Failed,
            };
        }
    }

    /// The host did not carry the action out (on its key it met a pause or work, or no key came):
    /// the instance is active again.
    pub(crate) fn abandoned(&mut self, alias: &str) {
        if let Some(entry) = self.instances.get_mut(alias)
            && matches!(entry.phase, Phase::Acting { .. })
        {
            entry.phase = Phase::Active;
        }
    }

    /// Whether the instance's emulator is stopped, or being stopped, by its idle action: the
    /// emulator watch does not start it again before its wake.
    pub(crate) fn emulator_held_stopped(&self, alias: &str) -> bool {
        self.instances.get(alias).is_some_and(|entry| {
            matches!(
                entry.phase,
                Phase::Acting {
                    action: IdleAction::StopEmulator,
                    ..
                } | Phase::Idle {
                    action: IdleAction::StopEmulator,
                    ..
                }
            )
        })
    }

    /// IDLE-1P (#405): the instances the idle action closed or is closing, by alias, each with
    /// its action and its wake in effect: what the Runtime keeps across a start. #405 O-2: a
    /// woken instance whose startup package has not run is still closed, `woken`, its wake the
    /// time it was woken.
    pub(crate) fn closed(&self) -> BTreeMap<String, KeptIdle> {
        self.instances
            .iter()
            .filter_map(|(alias, entry)| {
                let kept = match entry.phase {
                    Phase::Acting {
                        action,
                        wake_at_unix_ms,
                    }
                    | Phase::Idle {
                        action,
                        wake_at_unix_ms,
                    } => KeptIdle {
                        action,
                        wake_at_unix_ms,
                        woken: false,
                    },
                    Phase::Woken {
                        action,
                        woken_at_unix_ms,
                    } => KeptIdle {
                        action,
                        wake_at_unix_ms: Some(woken_at_unix_ms),
                        woken: true,
                    },
                    Phase::Active | Phase::Failed => return None,
                };
                Some((alias.clone(), kept))
            })
            .collect()
    }

    /// #405 N-2: binds the planner to this start's anchor (`host.contained_settlement`, this
    /// start's start event sequence; none when the start has none), which every record of the
    /// closed instances names.
    pub(crate) fn bind_start(&mut self, start_anchor: Option<u64>) {
        self.start_anchor = start_anchor;
    }

    /// #405 N-2: the start anchor the closed instances are kept under; none before the start's
    /// restore bound one.
    pub(crate) const fn start_anchor(&self) -> Option<u64> {
        self.start_anchor
    }

    /// IDLE-1P: the closed instances when they differ from the ones the host kept last (all of
    /// them before the first record); none when they do not.
    pub(crate) fn unkept(&self) -> Option<BTreeMap<String, KeptIdle>> {
        let closed = self.closed();
        (self.kept.as_ref() != Some(&closed)).then_some(closed)
    }

    /// IDLE-1P: the host kept `closed`.
    pub(crate) fn mark_kept(&mut self, closed: BTreeMap<String, KeptIdle>) {
        self.kept = Some(closed);
    }

    /// IDLE-1P (#405): a start restores `alias` as idle after `kept.action`, owing its wake
    /// `kept.wake_at_unix_ms` (none: the first plan that shows a task gives it). An action in
    /// flight when the Runtime stopped, and an instance whose state the start does not know, are
    /// restored so too: what is left on them is not known, so they count as closed, never as
    /// open, and their wake brings them back through the startup package (the host restores the
    /// latter with its wake at the start's time, review N-1). #405 O-2: so is an instance woken
    /// before the stop whose startup package had not run (`kept.woken`): its wake, the time it was
    /// woken, has passed, so the start's first check queues its startup package again.
    pub(crate) fn restore(&mut self, alias: &str, kept: KeptIdle) {
        self.instances.insert(
            alias.to_owned(),
            InstanceIdle {
                phase: Phase::Idle {
                    action: kept.action,
                    wake_at_unix_ms: kept.wake_at_unix_ms,
                },
                in_use_at_unix_ms: None,
                unsupported: None,
            },
        );
    }

    /// The wake in effect of an idle instance (the one planned at the action, or an earlier one a
    /// later plan gave); none when it is not idle or no wake is planned.
    #[cfg(test)]
    pub(crate) fn planned_wake(&self, alias: &str) -> Option<u64> {
        match self.instances.get(alias)?.phase {
            Phase::Idle {
                wake_at_unix_ms, ..
            } => wake_at_unix_ms,
            _ => None,
        }
    }
}

/// An active instance's check: the action, when every condition holds.
fn decide(
    entry: &mut InstanceIdle,
    config: &IdleConfig,
    view: IdleView,
    plan: Option<IdlePlan>,
    now_unix_ms: u64,
) -> Option<IdleStep> {
    let action = config.action;
    if action == IdleAction::None {
        return None;
    }
    // Fail Loud: an action that cannot bring the instance back never runs, and says so once.
    if let Some(missing) = view.missing(action) {
        if entry.unsupported == Some(action) {
            return None;
        }
        entry.unsupported = Some(action);
        return Some(IdleStep::Unsupported { action, missing });
    }
    if view.paused || !view.emulator_on {
        return None;
    }
    let plan = plan?;
    // A plan made before the instance's work settled does not know what follows that work.
    if entry
        .in_use_at_unix_ms
        .is_some_and(|at| plan.evaluated_at_unix_ms < at)
        || plan.due_now
        || plan
            .reading_due_unix_ms
            .is_some_and(|at| at <= plan.evaluated_at_unix_ms)
    {
        return None;
    }
    // A task or a reading within the minimum gap keeps the instance as it is.
    if plan
        .next_work_unix_ms()
        .is_some_and(|due| due.saturating_sub(now_unix_ms) < config.min_gap_ms)
    {
        return None;
    }
    // IDLE-1P: the wake is planned for the next task alone; a reading never sets it.
    let next_due_unix_ms = plan.next_wake_unix_ms;
    let wake_at_unix_ms =
        next_due_unix_ms.map(|due| due.saturating_sub(config.wake_lead_ms(action)));
    entry.phase = Phase::Acting {
        action,
        wake_at_unix_ms,
    };
    Some(IdleStep::Act {
        action,
        next_due_unix_ms,
        wake_at_unix_ms,
    })
}

/// An idle instance's check, held or not: the wake, when it is due; `current_unix_ms` is the wake
/// in effect. Otherwise the instance stays idle with the same wake or an earlier one. IDLE-1P: only
/// the plan's tasks count here; its fact-refresh reading neither wakes the instance nor moves the
/// wake.
fn wake(
    entry: &mut InstanceIdle,
    config: &IdleConfig,
    action: IdleAction,
    current_unix_ms: Option<u64>,
    plan: Option<IdlePlan>,
    now_unix_ms: u64,
) -> Option<IdleStep> {
    let lead_ms = config.wake_lead_ms(action);
    let from_plan = plan
        .and_then(|plan| plan.next_due_unix_ms())
        .map(|due| due.saturating_sub(lead_ms));
    // A wake only ever moves earlier.
    let wake_at_unix_ms = match (current_unix_ms, from_plan) {
        (Some(current), Some(from_plan)) => Some(current.min(from_plan)),
        (at, None) | (None, at) => at,
    };
    let due_now = plan.is_some_and(|plan| plan.due_now);
    let switched = config.action != action;
    if switched || due_now || wake_at_unix_ms.is_some_and(|at| at <= now_unix_ms) {
        // #405 O-2: closed and woken until the wake's claims, its startup package's run among
        // them, have ended.
        entry.phase = Phase::Woken {
            action,
            woken_at_unix_ms: now_unix_ms,
        };
        // The wake's claims hold the instance next: an action waits for a plan made after them.
        entry.in_use_at_unix_ms = Some(now_unix_ms);
        return Some(IdleStep::Wake {
            action,
            work: wake_work(action),
        });
    }
    entry.phase = Phase::Idle {
        action,
        wake_at_unix_ms,
    };
    None
}
