// SPDX-License-Identifier: AGPL-3.0-only

//! IDLE-1 (Alice's ruling 41 of 10-24 and her ruling of 10-25; Workflow #386 comment
//! 6393646823): the idle planner. It decides, for one instance at one idle check, whether the
//! instance's idle action runs now or whether an idle instance is brought back now, from what the
//! instance shows ([`IdleView`]), the scheduler's latest plan for it ([`IdlePlan`]: the latest
//! policy evaluation's due candidates and next wake for the instance, the evaluator's own; no
//! other estimate) and its idle keys in effect. It is pure: no clock, no device, no ledger. The
//! host (`host/idle.rs`) gathers the inputs at every policy cycle and every lease sweep, carries
//! the steps out and reports their ends ([`IdlePlanner::finished`]).
//!
//! - **The action** runs only when the instance is in no one's use (no lease, no queued request,
//!   no keyless manual, Lab or ladder work), no scheduling pause holds it, its emulator runs (the
//!   state both actions leave), it can be brought back (a startup package, and for
//!   `stop_emulator` its configured emulator start), and the plan, made after the instance was
//!   last seen in use, has no due task and no next wake closer than `idle.min_gap_ms`. It runs
//!   once: the instance is `Acting` until the host reports the end, then `Idle`, or `Failed` when
//!   the action failed (the instance is left as it is; only work on it again ends the wait, so
//!   the next due task goes through the normal path).
//! - **The wake** of an idle instance comes at its next due time minus the action's wake lead,
//!   the earlier of the one planned at the action and the one every later plan gives, or at once
//!   when a plan shows a due task, or when the action in effect is no longer the one taken (the
//!   switch). `stop_emulator` wakes through the configured emulator start and the startup
//!   package, `close_app` through the startup package.
//! - An idle instance that work holds again (a person's run, an emulator control, a ladder) is in
//!   use: it is no longer idle and no wake is queued for it.

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

    const fn wakeable(&self, action: IdleAction) -> bool {
        match action {
            IdleAction::None => false,
            IdleAction::CloseApp => self.app_wakeable,
            IdleAction::StopEmulator => self.emulator_wakeable,
        }
    }
}

/// The scheduler's plan for one instance, from its latest evaluated policy cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdlePlan {
    /// When the evaluation ran.
    pub(crate) evaluated_at_unix_ms: u64,
    /// A task of the instance is due: selected, or due and waiting.
    pub(crate) due_now: bool,
    /// The instance's earliest wake in the evaluation; none: no task of the instance wakes.
    pub(crate) next_wake_unix_ms: Option<u64>,
}

impl IdlePlan {
    /// The instance's next due time in this plan: the evaluation's time when a task is due,
    /// otherwise its next wake.
    pub(crate) const fn next_due_unix_ms(&self) -> Option<u64> {
        if self.due_now {
            Some(self.evaluated_at_unix_ms)
        } else {
            self.next_wake_unix_ms
        }
    }
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
    /// The action finished: the instance is idle until its wake, `wake_at_unix_ms`: the wake
    /// planned at the action, or an earlier one a later plan gave.
    Idle {
        action: IdleAction,
        wake_at_unix_ms: Option<u64>,
    },
    /// The action failed: the instance is left as it is until work holds it again.
    Failed,
}

#[derive(Debug, Clone, Copy)]
struct InstanceIdle {
    phase: Phase,
    /// The last check that found the instance in use: an action waits for a plan made after it.
    in_use_at_unix_ms: Option<u64>,
}

/// The idle state of every instance, by alias (memory only: a start begins with every instance
/// active).
#[derive(Debug, Default)]
pub(crate) struct IdlePlanner {
    instances: BTreeMap<String, InstanceIdle>,
    /// When the lease sweep's idle check last ran (monotonic milliseconds).
    last_sweep_ms: Option<u64>,
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
            Phase::Idle { .. } if view.in_use() => {
                entry.in_use_at_unix_ms = Some(now_unix_ms);
                entry.phase = Phase::Active;
                None
            }
            Phase::Idle {
                action,
                wake_at_unix_ms,
            } => wake(entry, config, action, wake_at_unix_ms, plan, now_unix_ms),
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

    /// The host's report of the end of the action it was asked for: `succeeded` makes the
    /// instance idle until its planned wake; a failure leaves it as it is.
    pub(crate) fn finished(&mut self, alias: &str, succeeded: bool) {
        if let Some(entry) = self.instances.get_mut(alias)
            && let Phase::Acting {
                action,
                wake_at_unix_ms,
            } = entry.phase
        {
            entry.phase = if succeeded {
                Phase::Idle {
                    action,
                    wake_at_unix_ms,
                }
            } else {
                Phase::Failed
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
    if action == IdleAction::None || view.paused || !view.emulator_on || !view.wakeable(action) {
        return None;
    }
    let plan = plan?;
    // A plan made before the instance's work settled does not know what follows that work.
    if entry
        .in_use_at_unix_ms
        .is_some_and(|at| plan.evaluated_at_unix_ms < at)
        || plan.due_now
    {
        return None;
    }
    let next_due_unix_ms = plan.next_wake_unix_ms;
    if next_due_unix_ms.is_some_and(|due| due.saturating_sub(now_unix_ms) < config.min_gap_ms) {
        return None;
    }
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

/// An idle instance's check: the wake, when it is due; `current_unix_ms` is the wake in effect.
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
        entry.phase = Phase::Active;
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
