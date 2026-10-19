// SPDX-License-Identifier: AGPL-3.0-only

//! The stuck-recovery ladder (Runtime slice #316-B4; Workflow #369 H-1, H-3; Workflow #387 D1c).
//!
//! Workflow #387 D1c (frozen model M387 v1.2): everything the ladder decides by comes from the
//! `recovery_ladder` configuration in effect (`crates/runtime-config`), never from a code
//! constant (Alice 10-09). A ladder takes the section as its snapshot when it is admitted and
//! climbs by that one to its end, queue included (§4.7); `recovery_ladder_started` names its
//! digest. The triggers are an ordered list of trigger items matched by a code's class or by
//! the code (§3.1), with an exclusion list:
//!
//! - the task stage: a policy run's failed lease end (`cleanup_scheduled_failure_with_run_links`,
//!   to which only the policy-run path passes a `LadderTrigger`), and a direct run's (CLI or
//!   console task-run, MCP `ac_run_pack`) failed end only under `rules.direct_runs = trigger`
//!   (B1; Workflow #369-3, coordinator rulings Q1 and P1);
//! - the startup and connection preparation stages: a failed daemon-start or connection
//!   preparation (emulator start or restart, resume, self-check) whose resources were disposed
//!   of safely;
//! - direction ① (`stability_warnings`, §3.4): the `count`-th failure of an item's classes or
//!   codes on one instance within `window_ms`, counted in memory as records are committed (one
//!   failure once, however many records it commits: Workflow #387 D1c-H) and admitted by the
//!   lease sweep, never on the write path;
//! - a resource fence (Workflow #403), which matches no trigger item: a fence that manual work
//!   met starts none while `rules.direct_runs = exempt` (B1, coordinator ruling 4).
//!
//! A scheduled run's ladder is decided inside the run's failed lease end: an admitted ladder
//! that starts `immediately` turns that end into a transfer of the key to the ladder's claim,
//! so nothing can take the instance between the failure and the climb; one that waits for
//! performance release, or that is queued behind another ladder (B3 `queue`), waits in the
//! instance's queue instead. A daemon-start preparation's ladder is queued as a claim after the
//! preparation released its dedicated lease; after a preparation on a held key, the key's
//! release hands the key straight to the ladder's claim (`release_to_preparation_ladder`).
//! Either way the ladder climbs on its instance's worker (`host_claims`) and holds the key from
//! its first rung to its end (H-3): rung runs run on its token, each after the ladder queued
//! its own continuation, to which the run's release hands the key back; the ladder renews
//! before every step whose bound exceeds what is left; at every step it checks the rules that
//! stay in code (`ladder_invariants`, §2.2b); and it releases once.
//!
//! The rungs, in the configured order, are existing work under the instance's key:
//! `return_home` runs the failed run's bound recovery package as a standalone contained task
//! (when it bound none, the return-home package actingd configures for its package's game and
//! server, with the configured deadline; Workflow #336 L2d; a ladder without a failed run takes
//! the instance's startup package's), `application_restart` force-stops the instance's assigned
//! application and then schedules and runs the instance's startup package (Workflow #369-2),
//! and `emulator_restart` confirms Stop before Start through the existing control owner, then
//! prepares the discovered binding. Environment readiness and a configured package's target
//! attainment are separate facts. Missing packages skip only the affected rung; an unavailable
//! entry channel skips it or fails it (B5). A trigger item may start the climb at a later rung;
//! the rungs before it are skipped (`before_start_rung`). The original task is never re-run.
//!
//! One ladder per instance per cool-down window: a trigger while a ladder holds or waits for
//! the instance (read from the scheduler) is suppressed or queued behind it (B3); one inside the
//! window (opened when a ladder got the key, or at every matched trigger, B4) is suppressed
//! unless its item ignores the cool-down. Every fact is a `runtime.lifecycle_observed` linked
//! to the instance and the trigger's correlation id, under the ladder's own request and
//! causation ids; the `return_home` run carries that causation id, the startup package runs the
//! one of their scheduling event.

use super::contained_task::{ConfiguredReturnHome, FailedTaskTerminal, PackageIdentity};
use super::host_claims::{HostClaimWork, HostKey};
use super::lease::{ClaimGrantSlot, HostClaim, HostClaimAdmission};
use super::resource_fence::manual_claim;
use super::runtime_config::ladder_profile;
use super::startup_package::{HostPackageRun, PendingStartupPackage};
use super::*;
use crate::codes::HostCode;
use crate::codes::HostLocation;
use crate::error::spelled;
use crate::failure_identity::identifier_text;
use actingcommand_contract::outcome::{Code, CodeStr, Outcome};
use actingcommand_contract::{
    BackendObservationStatus, BackendOpenEntry, ContainedTaskRecoveryBinding,
    EmulatorInstanceAction, HolderId, InstanceStuckRecovery, IssuedCausationId,
    IssuedCorrelationId, IssuedRequestId, LeasePriority, RecoveryDrillMode, RecoveryLadderDrill,
    RecoveryLadderOutcome, RecoveryLadderSuppression, RecoveryLadderTrigger, RecoveryRung,
    RecoveryRungOutcome, RecoveryRungPlan, RecoveryRungSkipReason, RecoveryRungState,
    RecoveryTriggerMatch, RecoveryTriggerStage, RecoveryTriggerStart, SchedulingResumeSelfCheck,
};
use actingcommand_device::DeviceResourceQuiescence;
use actingcommand_runtime_config::{
    AlreadyRunning, ConfigCode, CooldownOpensOn, DeviceBounds, DirectRuns, EntryUnavailable,
    LadderConfig, LadderProfile, LadderRules, MatchedBy, PreparationSuperseded, RungConfig,
    TriggerItem, TriggerVerdict, ladder_class_of,
};

const LADDER_OPERATION: HostLocation = HostLocation::RunRecoveryLadder;
/// The application stop's own bound (one ADB application command: the bound endpoint's
/// `ensure_device` and `am force-stop`), the device fact rung 2's entry hold must cover.
const APPLICATION_STOP_BOUND: Duration = Duration::from_secs(60);

/// Workflow #387 D1c (M387 §4.2; D1b deviation 14): the device layer's own bounds of the steps a
/// ladder's rungs take, which the `recovery_ladder` check reads: a rung configured shorter than
/// the device would time out every time. An emulator Stop is the control dispatch, the stopped
/// state's wait, the last `info` poll and the last process listing; a Start is the control
/// dispatch, the running state's wait and the last `info` poll.
pub fn recovery_ladder_device_bounds() -> DeviceBounds {
    let ms = |bound: Duration| u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
    DeviceBounds {
        application_stop_ms: ms(APPLICATION_STOP_BOUND),
        emulator_stop_ms: ms(actingcommand_device::MUMU_MANAGER_CONTROL_TIMEOUT
            + actingcommand_device::MUMU_MANAGER_STATE_WAIT_STOP
            + actingcommand_device::MUMU_MANAGER_COMMAND_TIMEOUT
            + actingcommand_device::MUMU_MANAGER_PROCESS_LIST_TIMEOUT),
        emulator_start_ms: ms(actingcommand_device::MUMU_MANAGER_CONTROL_TIMEOUT
            + actingcommand_device::MUMU_MANAGER_STATE_WAIT_START
            + actingcommand_device::MUMU_MANAGER_COMMAND_TIMEOUT),
    }
}

/// M387 §2.2b (Alice 10-10, §12 question 4): the rules of the ladder that stay in code, whatever
/// its configuration says. `ladder_invariants` names the first one, in this order, that stops a
/// ladder; A8 is `ladder_exhausted_severity`. A9 (Alice 10-13, board #387 ruling of 09:3x JST)
/// came with Workflow #398 RD-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LadderInvariant {
    /// A1: only a physical instance has an application or an emulator to restart.
    NotPhysical,
    /// A2: the ladder's own preparation never starts a ladder (no unbounded loop).
    RecoveryPreparation,
    /// A3: the Runtime is shutting down.
    Shutdown,
    /// A4: the ladder no longer holds its instance's key.
    KeyLost,
    /// A3: an install drains business.
    Drain,
    /// A6: a scheduling pause holds the instance (a person's input comes first).
    Paused,
    /// A5: business capacity refuses new work (the ledger could not record it).
    CapacityRefused,
    /// A7: a blackout window keeps the instance silent. The host has no blackout state before
    /// SR-4a, so nothing sets it yet; a build from SR-2a on reads the `blackout` suppression
    /// SR-4a records (Workflow #386, register S3).
    Blackout,
    /// A9: the instance's handset left `device`: there is no device to act on (as A1 and A4),
    /// so the climb is interrupted and ends at warning (model-398 v1.1 §4.5).
    DeviceLinkLost,
}

impl LadderInvariant {
    /// The reason a holding check ends a climb with (§4.4); a refused admission is
    /// `recovery_admission_denied`.
    fn interruption(self) -> &'static str {
        match self {
            Self::Shutdown => HostCode::RecoveryLadderShutdownRequested.as_str(),
            Self::KeyLost => HostCode::RecoveryLadderKeyLost.as_str(),
            Self::Drain => HostCode::RecoveryLadderDrainRequested.as_str(),
            Self::DeviceLinkLost => HostCode::DeviceLinkLost.as_str(),
            Self::NotPhysical
            | Self::RecoveryPreparation
            | Self::Paused
            | Self::CapacityRefused
            | Self::Blackout => HostCode::RecoveryAdmissionDenied.as_str(),
        }
    }
}

/// What the rules of §2.2b read. A site sets what it knows; the rest stays passing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LadderFacts {
    pub(super) physical: bool,
    pub(super) recovery_preparation: bool,
    pub(super) shutdown: bool,
    pub(super) holds_key: bool,
    pub(super) draining: bool,
    pub(super) paused: bool,
    pub(super) capacity: bool,
    pub(super) blackout: bool,
    /// A9: whether the instance's device link is `device` (an emulator's always is).
    pub(super) device_linked: bool,
}

impl LadderFacts {
    /// Facts under which no rule stops a ladder.
    pub(super) const PASSING: Self = Self {
        physical: true,
        recovery_preparation: false,
        shutdown: false,
        holds_key: true,
        draining: false,
        paused: false,
        capacity: true,
        blackout: false,
        device_linked: true,
    };
}

/// §2.2b A1–A7 and A9: the first rule, in `LadderInvariant` order, that stops a ladder under
/// `facts`.
/// It reads no configuration: no `recovery_ladder` value changes it.
pub(super) const fn ladder_invariants(facts: &LadderFacts) -> Option<LadderInvariant> {
    if !facts.physical {
        Some(LadderInvariant::NotPhysical)
    } else if facts.recovery_preparation {
        Some(LadderInvariant::RecoveryPreparation)
    } else if facts.shutdown {
        Some(LadderInvariant::Shutdown)
    } else if !facts.holds_key {
        Some(LadderInvariant::KeyLost)
    } else if facts.draining {
        Some(LadderInvariant::Drain)
    } else if facts.paused {
        Some(LadderInvariant::Paused)
    } else if !facts.capacity {
        Some(LadderInvariant::CapacityRefused)
    } else if facts.blackout {
        Some(LadderInvariant::Blackout)
    } else if !facts.device_linked {
        Some(LadderInvariant::DeviceLinkLost)
    } else {
        None
    }
}

/// §2.2b A8: an exhausted climb's severity is its outcome's category (#378): `warning` when a
/// holding interruption ended it, `error` when its rungs failed on their own.
pub(super) const fn ladder_exhausted_severity(interrupted: bool) -> EventSeverity {
    if interrupted {
        EventSeverity::Warning
    } else {
        EventSeverity::Error
    }
}

/// M387 §2.2a: what each of the five rules decides, one function per rule; the ladder's code
/// paths read them, so each rule's two values lead to its two behaviours (D1-L8).
///
/// B1: manual work is left alone while `direct_runs` is `exempt`: a direct run's terminal, and
/// for direction ① every record committed while manual work holds the instance's key (the set a
/// resource fence treats as manual, `manual_claim`; Workflow #387 D1c-H).
pub(super) fn direct_run_exempt(rules: &LadderRules, direct: bool) -> bool {
    direct && rules.direct_runs == DirectRuns::Exempt
}

/// B2: a preparation ladder is suppressed when a later preparation superseded its own and
/// `preparation_superseded` is `suppress`.
pub(super) fn preparation_superseded_suppresses(rules: &LadderRules, superseded: bool) -> bool {
    superseded && rules.preparation_superseded == PreparationSuperseded::Suppress
}

/// What the admission of a ladder decides (§4.5, B3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Admission {
    /// Admitted; `behind_ladder` when another ladder holds or waits for the instance and it
    /// queues behind that one.
    Admitted {
        behind_ladder: bool,
    },
    Suppressed(RecoveryLadderSuppression),
}

/// B3 and the cool-down at admission: a ladder holds or waits for the instance (suppressed, or
/// queued behind it under `already_running = queue`), or the window it respects is open.
pub(super) fn admission(rules: &LadderRules, ladder_active: bool, cooling_down: bool) -> Admission {
    if ladder_active {
        match rules.already_running {
            AlreadyRunning::Suppress => {
                Admission::Suppressed(RecoveryLadderSuppression::AlreadyRunning)
            }
            AlreadyRunning::Queue => Admission::Admitted {
                behind_ladder: true,
            },
        }
    } else if cooling_down {
        Admission::Suppressed(RecoveryLadderSuppression::Cooldown)
    } else {
        Admission::Admitted {
            behind_ladder: false,
        }
    }
}

/// B4: whether the cool-down window opens at this moment: when a trigger item matched
/// (`on_trigger`) under `triggered`, or when a ladder got the key under `admitted`.
pub(super) fn opens_cooldown_window(rules: &LadderRules, on_trigger: bool) -> bool {
    match rules.cooldown_opens_on {
        CooldownOpensOn::Admitted => !on_trigger,
        CooldownOpensOn::Triggered => on_trigger,
    }
}

/// B5: what an unavailable entry channel makes of a rung that has not acted yet: skipped for
/// `skip`, or failed with `refused`.
fn unavailable_entry(
    rules: &LadderRules,
    skip: RecoveryRungSkipReason,
    refused: &'static str,
) -> RungAttempt {
    match rules.entry_unavailable {
        EntryUnavailable::Skip => RungAttempt::Skipped { reason: skip },
        EntryUnavailable::Fail => RungAttempt::Failed {
            run_id: None,
            reason: refused,
        },
    }
}

/// One admitted ladder: its trigger, the facts' links, its claim and the configuration it was
/// admitted with.
#[derive(Clone)]
pub(super) struct PendingRecoveryLadder {
    instance_id: InstanceId,
    instance_alias: String,
    trigger: RecoveryLadderTrigger,
    recovery: Option<ContainedTaskRecoveryBinding>,
    /// Workflow #336 L2d: when `recovery` is the registry's return-home package (the run bound
    /// none; Workflow #373 NAV-3a), the failed package it must match and its package id.
    recovery_configured: Option<ConfiguredReturnHome>,
    /// Instance, trigger correlation id, the ladder's request id and causation id.
    links: EventLinksDraft,
    /// The ladder's request id: task timing admission id of its rung runs, and its claim's.
    request_id: RequestId,
    correlation_id: IssuedCorrelationId,
    causation_id: IssuedCausationId,
    cooldown_ms: u64,
    /// Workflow #369 H-1: the ladder claim's own synthetic request (`AcquireLease` of the
    /// ladder's holder), whose links carry the claim's records; its key keeps it.
    claim: RuntimeRequest,
    holder_id: HolderId,
    /// Workflow #387 D1c (§4.7): the `recovery_ladder` section the ladder was admitted with.
    config: Arc<LadderConfig>,
    /// Workflow #398 RD-4 (model-398 §6.2): the profile of `config` the instance climbs by,
    /// taken at admission with the section; `None` for the section's `rungs`.
    profile: Option<LadderProfile>,
    /// The position in the rungs it climbs, from 1, its trigger starts the climb at (§4.1);
    /// `rules.start_beyond_profile` says what one beyond them does (model-398 §6.6).
    start_rung: usize,
    /// Whether the cool-down window holds this ladder (§4.6).
    respect_cooldown: bool,
    /// B3 `queue`: admitted behind a ladder that holds or waits for the instance; judged
    /// against the cool-down again when it gets the key.
    behind_ladder: bool,
    /// Workflow #390 DRILL-1: a person's drill, and what it may do; `None` for a ladder a
    /// trigger started. A drill's own records are tagged `drill`.
    drill: Option<RecoveryDrillMode>,
}

impl PendingRecoveryLadder {
    /// Workflow #390 DRILL-1: a plan drill never acts on the device and changes nothing the
    /// ladder keeps (the cool-down window, a resource fence's stage).
    fn plan_drill(&self) -> bool {
        self.drill == Some(RecoveryDrillMode::Plan)
    }

    pub(super) const fn claim(&self) -> &RuntimeRequest {
        &self.claim
    }

    pub(super) const fn holder_id(&self) -> HolderId {
        self.holder_id
    }

    pub(super) const fn links(&self) -> &EventLinksDraft {
        &self.links
    }

    /// The TTL its claim and each continuation is granted with (§4.3 `hold.claim_ttl_ms`).
    pub(super) fn claim_ttl_ms(&self) -> u64 {
        self.config.hold.claim_ttl_ms
    }

    /// Whether a lease end hands its key to the ladder at once. A ladder that waits for
    /// performance release (§3.7), or one queued behind another ladder (B3), waits in the
    /// instance's queue instead.
    pub(super) fn hands_off(&self) -> bool {
        self.trigger.start == RecoveryTriggerStart::Immediately && !self.behind_ladder
    }

    /// Whether its claim waits until performance control releases the instance (§3.7).
    pub(super) fn awaits_performance_release(&self) -> bool {
        self.trigger.start == RecoveryTriggerStart::AfterPerformanceRelease
    }
}

/// An admitted ladder, or why its admission was suppressed with the `until_unix_ms` its
/// `recovery_ladder_suppressed` record names.
type LadderAdmission = Result<PendingRecoveryLadder, (RecoveryLadderSuppression, u64)>;

/// Per instance: where the cool-down window ends, and the identity of the latest preparation
/// attempt (`PreparationSuperseded`). Whether a ladder holds or waits is the scheduler's
/// (Workflow #369 §3.5).
#[derive(Default)]
pub(super) struct RecoveryLadderWindow {
    until_unix_ms: u64,
    pub(super) preparation: Option<TerminalEvent>,
}

/// Workflow #369 H-1 (review M-1): what only a run that may start a ladder notes on its run's
/// control: a policy run, and (Workflow #387 D1c, B1) a direct run, whose failed end starts one
/// only under `rules.direct_runs = trigger`.
pub(super) struct LadderSource {
    pub(super) correlation_id: IssuedCorrelationId,
    pub(super) recovery: Option<ContainedTaskRecoveryBinding>,
    pub(super) direct: bool,
}

/// Workflow #369 H-1: the trigger a run's failed lease end passes on: the run's committed
/// `task.failed`, its correlation, its bound recovery package, its package's identity and
/// whether it was a direct run.
pub(super) struct LadderTrigger {
    terminal: FailedTaskTerminal,
    correlation_id: IssuedCorrelationId,
    recovery: Option<ContainedTaskRecoveryBinding>,
    package: Option<PackageIdentity>,
    direct: bool,
}

impl LadderTrigger {
    pub(super) fn new(
        terminal: FailedTaskTerminal,
        source: &LadderSource,
        package: Option<PackageIdentity>,
    ) -> Self {
        Self {
            terminal,
            correlation_id: source.correlation_id,
            recovery: source.recovery.clone(),
            package,
            direct: source.direct,
        }
    }
}

/// Workflow #369 H-3: the ladder's continuation, queued before a rung run so that the run's
/// release hands the key back to the ladder.
pub(super) struct LadderContinuation {
    request_id: RequestId,
    grant: Arc<ClaimGrantSlot>,
}

/// Workflow #369-2: what the application restart rung does after its entry checks.
enum RestartEntry {
    /// A known unavailable entry channel or ADB, the game untouched: skipped, or failed with
    /// `refused` under `rules.entry_unavailable = fail` (B5).
    Unavailable {
        skip: RecoveryRungSkipReason,
        refused: &'static str,
    },
    /// The entry's ADB probe left its cleanup unconfirmed and fenced the instance (Workflow
    /// #387 D1c-G): the rung fails with `fence`, the fence's code, without touching the game.
    Fenced { fence: &'static str },
    /// Stop the assigned application, then run the startup package.
    Stop,
    /// The startup package cannot be admitted, its prerequisite chain is refused, or it is
    /// incompatible with a startup run: run it without the stop, so its refusal is recorded as
    /// before.
    RunOnly,
}

enum RungAttempt {
    Recovered {
        run_id: RunId,
    },
    EnvironmentReady {
        event: TerminalEvent,
    },
    Skipped {
        reason: RecoveryRungSkipReason,
    },
    Failed {
        run_id: Option<RunId>,
        reason: &'static str,
    },
}

/// Rung 3's bounds from the ladder's configuration (§4.2).
struct EmulatorRestartBounds<'a> {
    stop_bound_ms: u64,
    start_bound_ms: u64,
    readiness_window_ms: u64,
    readiness_backoff_ms: &'a [u64],
    readiness_poll_ms: u64,
}

/// Workflow #387 D1c (M387 §3.4): direction ①'s per-instance windows, memory only and untagged
/// (a restart starts every window empty, LEDG:932). A leaf lock: the append path takes it while
/// it holds `fact_write_gate` and takes nothing under it; the lease sweep admits what it
/// registered. Workflow #387 D1c-H (Alice 10-14): a window holds failures, not records; one
/// failure counts once, however many matching records it commits. The scheduler reports each
/// change of an instance key's holder under its own lock (scheduler → this leaf), so the write
/// path takes no lock but this one.
#[derive(Default)]
pub(super) struct StabilityCounter {
    /// The section in effect, refreshed whenever the configuration changes.
    config: Option<Arc<LadderConfig>>,
    /// The instances a ladder climbs on: their records are its own work.
    climbing: BTreeSet<InstanceId>,
    /// Per instance, the kind of claim the scheduler last reported holding its key (B1).
    key_holders: BTreeMap<InstanceId, ClaimKind>,
    /// Per instance and trigger item, when each counted failure counted, in time order, oldest
    /// first (#411 A24: not in the order their records were committed).
    windows: BTreeMap<InstanceId, BTreeMap<String, VecDeque<u64>>>,
    /// Per instance, the failures of recent records and the failures counted (D1c-H).
    failures: BTreeMap<InstanceId, InstanceFailures>,
    /// Bursts registered and not admitted yet, oldest first.
    bursts: Vec<StabilityBurst>,
    /// Codes met that the catalog does not list, not recorded yet.
    unclassified: Vec<String>,
}

/// Workflow #387 D1c-H: the failure a counted record belongs to, from the links the record
/// already carries, in this order: the run it belongs to; else the request it answers; else
/// the record its failure entered from (a `runtime.failed` record's `entered_event_id`), as
/// the failure that record belongs to; else the record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FailureIdentity {
    Run(RunId),
    Request(RequestId),
    Record(EventId),
}

/// Workflow #387 D1c-H: one instance's memory of failures. `records` names the failure each
/// recent record belongs to, so that a record entering from it joins that failure; `counted`
/// holds the failures that counted, each with when it counted. Both forget what is older than
/// the longest window.
#[derive(Default)]
struct InstanceFailures {
    records: BTreeMap<EventId, (u64, FailureIdentity)>,
    counted: BTreeMap<FailureIdentity, u64>,
}

impl InstanceFailures {
    fn forget_older_than(&mut self, at_unix_ms: u64, horizon_ms: u64) {
        self.records
            .retain(|_, (seen, _)| at_unix_ms.saturating_sub(*seen) <= horizon_ms);
        self.counted
            .retain(|_, counted| at_unix_ms.saturating_sub(*counted) <= horizon_ms);
    }

    /// The failure `record` belongs to; the record is remembered as part of it.
    fn identify(&mut self, record: &CountedRecord<'_>) -> FailureIdentity {
        let failure = match (record.run_id, record.request_id, record.entered_event_id) {
            (Some(run_id), _, _) => FailureIdentity::Run(run_id),
            (None, Some(request_id), _) => FailureIdentity::Request(request_id),
            (None, None, Some(entered)) => self
                .records
                .get(&entered)
                .map_or(FailureIdentity::Record(entered), |(_, failure)| *failure),
            (None, None, None) => FailureIdentity::Record(record.event_id),
        };
        self.records
            .insert(record.event_id, (record.at_unix_ms, failure));
        failure
    }
}

/// One burst direction ① registered (§3.4): its trigger item in the configuration it matched
/// by, the window's first and last failure and the last record's code.
pub(super) struct StabilityBurst {
    instance_id: InstanceId,
    config: Arc<LadderConfig>,
    item: usize,
    count: u32,
    window_ms: u64,
    first_unix_ms: u64,
    last_unix_ms: u64,
    failure_code: CodeStr,
}

/// What direction ① reads of one committed record: its instance, its own event id, its run and
/// request links, the record its failure entered from, when, how severe, and its code.
pub(super) struct CountedRecord<'a> {
    pub(super) instance_id: InstanceId,
    pub(super) event_id: EventId,
    pub(super) run_id: Option<RunId>,
    pub(super) request_id: Option<RequestId>,
    pub(super) entered_event_id: Option<EventId>,
    pub(super) at_unix_ms: u64,
    pub(super) severity: EventSeverity,
    pub(super) spelling: &'a str,
}

impl StabilityCounter {
    pub(super) fn configure(&mut self, config: Arc<LadderConfig>) {
        self.config = Some(config);
    }

    /// A ladder starts or ends its climb on `instance_id`.
    pub(super) fn set_climbing(&mut self, instance_id: InstanceId, climbing: bool) {
        if climbing {
            self.climbing.insert(instance_id);
        } else {
            self.climbing.remove(&instance_id);
        }
    }

    /// Workflow #387 D1c-H (B1): the scheduler reports which kind of claim holds
    /// `instance_id`'s key now (`None`: the key is free).
    pub(super) fn set_key_holder(&mut self, instance_id: InstanceId, holder: Option<ClaimKind>) {
        match holder {
            Some(kind) => {
                self.key_holders.insert(instance_id, kind);
            }
            None => {
                self.key_holders.remove(&instance_id);
            }
        }
    }

    /// Counts one committed record (§3.4): a warning or worse, on an instance no ladder climbs
    /// on, not committed while manual work holds the instance's key under
    /// `rules.direct_runs = exempt` (B1, the fence's set), whose code the first
    /// stability-warnings trigger item matches. Workflow #387 D1c-H: the record counts only as
    /// the first matching record of its failure (`FailureIdentity`); the `count`-th failure
    /// within the item's window registers a burst. Whether it is then admitted or suppressed,
    /// every window of the instance starts empty again, and the failures it counted stay
    /// counted, so their later records do not count again. #411 A24: a record's time is its
    /// producer's, not its commit time, so records can arrive out of time order (a run's
    /// diagnostic stream failure is stamped with the stream's start and committed at the run's
    /// end); the window keeps its failures in time order and counts those within the item's
    /// `window_ms` of its latest failure, whatever order they arrived in.
    pub(super) fn count(&mut self, record: &CountedRecord<'_>) {
        let Some(config) = self.config.clone() else {
            return;
        };
        if !matches!(
            record.severity,
            EventSeverity::Warning | EventSeverity::Error | EventSeverity::Fatal
        ) || self.climbing.contains(&record.instance_id)
            || direct_run_exempt(
                &config.rules,
                self.key_holders
                    .get(&record.instance_id)
                    .copied()
                    .is_some_and(manual_claim),
            )
        {
            return;
        }
        let horizon_ms = config
            .triggers
            .iter()
            .filter_map(|item| item.burst)
            .map(|burst| burst.window_ms)
            .max()
            .unwrap_or(0);
        let failures = self.failures.entry(record.instance_id).or_default();
        failures.forget_older_than(record.at_unix_ms, horizon_ms);
        let failure = failures.identify(record);
        if ladder_class_of(record.spelling).is_none() {
            if !self
                .unclassified
                .iter()
                .any(|spelling| spelling == record.spelling)
            {
                self.unclassified.push(record.spelling.to_owned());
            }
            return;
        }
        let TriggerVerdict::Matched { item, .. } =
            config.classify(RecoveryTriggerStage::StabilityWarnings, record.spelling)
        else {
            return;
        };
        let Some(burst) = item.burst else {
            return;
        };
        let Some(position) = config
            .triggers
            .iter()
            .position(|candidate| candidate.id == item.id)
        else {
            return;
        };
        if failures.counted.contains_key(&failure) {
            return;
        }
        failures.counted.insert(failure, record.at_unix_ms);
        let window = self
            .windows
            .entry(record.instance_id)
            .or_default()
            .entry(item.id.clone())
            .or_default();
        let place = window.partition_point(|counted| *counted <= record.at_unix_ms);
        window.insert(place, record.at_unix_ms);
        let latest_unix_ms = window.back().copied().unwrap_or(record.at_unix_ms);
        while window
            .front()
            .is_some_and(|first| latest_unix_ms.saturating_sub(*first) > burst.window_ms)
        {
            window.pop_front();
        }
        if window.len() < usize::try_from(burst.count).unwrap_or(usize::MAX) {
            return;
        }
        let first_unix_ms = window.front().copied().unwrap_or(latest_unix_ms);
        self.windows.remove(&record.instance_id);
        self.bursts.push(StabilityBurst {
            instance_id: record.instance_id,
            config: Arc::clone(&config),
            item: position,
            count: burst.count,
            window_ms: burst.window_ms,
            first_unix_ms,
            last_unix_ms: latest_unix_ms,
            failure_code: Outcome::relayed(record.spelling).code().clone(),
        });
    }

    /// The bursts and unlisted codes registered since the last call.
    pub(super) fn take(&mut self) -> (Vec<StabilityBurst>, Vec<String>) {
        (
            std::mem::take(&mut self.bursts),
            std::mem::take(&mut self.unclassified),
        )
    }
}

/// Binds the configured stuck-recovery settings to registered instances at startup; an alias
/// that is not registered fails with `stuck_recovery_instance_unknown`. An instance without
/// settings uses the configuration's.
pub(super) fn resolve_stuck_recovery(
    configured: &BTreeMap<String, InstanceStuckRecovery>,
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceStuckRecovery>> {
    configured
        .iter()
        .map(|(alias, settings)| {
            instances
                .values()
                .find(|instance| instance.instance_alias == *alias)
                .map(|instance| (instance.instance_id(), *settings))
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        HostCode::StuckRecoveryInstanceUnknown,
                        HostLocation::StartRuntimeHost,
                        RuntimeErrorCode::RuntimeHalted,
                    )
                    .with_native_detail(format!("instance_alias={alias}"))
                })
        })
        .collect()
}

/// Folds a staging failure (always fatal: a lock, an identifier or the ledger) into the
/// run's own result.
pub(super) fn with_recovery_ladder_staging<T>(
    executed: Result<T, RequestFailure>,
    staged: RuntimeHostResult<()>,
) -> Result<T, RequestFailure> {
    match (executed, staged) {
        (executed, Ok(())) => executed,
        (Ok(_), Err(error)) => Err(RequestFailure::poison_without_terminal(error)),
        (Err(failure), Err(error)) => Err(failure.replace_with_poison(error)),
    }
}

fn ladder_invariant(code: impl Into<Code>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, LADDER_OPERATION, RuntimeErrorCode::RuntimeHalted)
}

/// How a trigger item matched, as the ladder records it (§6.1).
const fn trigger_match(by: MatchedBy) -> RecoveryTriggerMatch {
    match by {
        MatchedBy::Code => RecoveryTriggerMatch::Code,
        MatchedBy::Class(ladder_class) => RecoveryTriggerMatch::Class { ladder_class },
    }
}

impl HostShared {
    /// §3.1: the trigger item the code `failure_code` matches at `stage` in `config`, with how
    /// it matched; `None` when the exclusion list names the code or no item matches it. A code
    /// the catalog does not list is read as class `none` and recorded once.
    fn match_trigger<'a>(
        &self,
        config: &'a LadderConfig,
        stage: RecoveryTriggerStage,
        failure_code: &CodeStr,
    ) -> RuntimeHostResult<Option<(&'a TriggerItem, RecoveryTriggerMatch)>> {
        if ladder_class_of(failure_code.as_str()).is_none() {
            self.note_unclassified_code(failure_code.as_str())?;
        }
        Ok(match config.classify(stage, failure_code.as_str()) {
            TriggerVerdict::Matched { item, by } => Some((item, trigger_match(by))),
            TriggerVerdict::Excluded | TriggerVerdict::Unmatched => None,
        })
    }

    /// `recovery_ladder_code_unclassified` (warning), once per process and code (§3.1): the
    /// ladder met a code the embedded catalog does not list and read it as class `none`.
    pub(super) fn note_unclassified_code(&self, spelling: &str) -> RuntimeHostResult<()> {
        if !lock(
            &self.unclassified_codes,
            HostLocation::RecoveryUnclassifiedNote,
        )?
        .insert(spelling.to_owned())
        {
            return Ok(());
        }
        self.append_event_raw(
            // The envelope derives the record's severity from the outcome's category.
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.events.system_links()?,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::new(ConfigCode::RecoveryLadderCodeUnclassified)
                    .raw_code(spelling.to_owned())
                    .operation(HostLocation::RecoveryUnclassifiedNote),
                DiagnosticDetailDraft::new(
                    "recovery_ladder",
                    HostLocation::RecoveryUnclassifiedNote,
                    "runtime_host",
                    HostLocation::RecoveryUnclassifiedNote.as_str(),
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// Workflow #369-1: whether a failed preparation is one a recovery readiness wait retries:
    /// Workflow #387 D1c, the configured trigger items name its code `spelling` for a connection
    /// preparation (the rule the preparation code table held before).
    pub(super) fn preparation_retryable(&self, spelling: Option<&str>) -> RuntimeHostResult<bool> {
        let Some(spelling) = spelling else {
            return Ok(false);
        };
        let config = self.ladder_config()?;
        Ok(matches!(
            config.classify(RecoveryTriggerStage::ConnectionPreparation, spelling),
            TriggerVerdict::Matched { .. }
        ))
    }

    /// The summary is appended only after original acquisition disposal and the existing
    /// session close/release path returned. Its event identity is the preparation attempt.
    /// Returns it with the ladder a failed preparation admitted, which is not queued yet: the
    /// caller queues it, or hands it the key it holds (`release_to_preparation_ladder`).
    /// `recoverable` says the attempt's resources were disposed of safely; which codes start a
    /// ladder is the configuration's (§3.5).
    pub(super) fn record_preparation_finished(
        &self,
        instance_id: InstanceId,
        links: EventLinksDraft,
        stage: RecoveryTriggerStage,
        observations: &[actingcommand_device::BackendOpenObservation],
        check: &SchedulingResumeSelfCheck,
        recoverable: bool,
    ) -> RuntimeHostResult<(TerminalEvent, Option<PendingRecoveryLadder>)> {
        let channel = |entry, passed| {
            if passed {
                return BackendObservationStatus::Passed;
            }
            observations
                .iter()
                .rev()
                .map(|value| &value.report)
                .find(|report| report.entry == entry || report.entry == BackendOpenEntry::NemuPair)
                .map_or(BackendObservationStatus::Unknown, |report| {
                    let own = if entry == BackendOpenEntry::Input {
                        report.input_check
                    } else {
                        report.capture_check
                    };
                    if [report.status, report.connection, own]
                        .contains(&BackendObservationStatus::Failed)
                    {
                        BackendObservationStatus::Failed
                    } else {
                        BackendObservationStatus::Unknown
                    }
                })
        };
        // Ledger revision 3: the preparation's failure is the record's outcome, the code the
        // self-check relays; a ladder it starts names that recorded code as its trigger.
        let preparation_failure = check.failure_code.as_deref().map(Outcome::relayed);
        let event = self.append_event_raw(
            if preparation_failure.is_some() {
                EventSeverity::Warning
            } else {
                EventSeverity::Info
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage,
                    capture: channel(BackendOpenEntry::Capture, check.capture.ok),
                    input: channel(BackendOpenEntry::Input, check.touch.ok),
                    outcome: preparation_failure.clone(),
                },
                AuditInput::new(),
            ),
        )?;
        let event = terminal(&event);
        lock(
            &self.recovery_ladders,
            HostLocation::RecordPreparationAttempt,
        )?
        .entry(instance_id)
        .or_default()
        .preparation = Some(event);
        // Workflow #369-3 (coordinator ruling P1): the direct-run exemption covers task runs
        // only, which make no preparation of their own. A failed daemon-start or connection
        // preparation (emulator start or restart, resume, self-check) still starts a ladder, as
        // before: nothing else retries those. Only the ladder's own preparation never does (A2),
        // and none after a shutdown request (A3).
        let Some(preparation_failure) = &preparation_failure else {
            return Ok((event, None));
        };
        let gated = ladder_invariants(&LadderFacts {
            recovery_preparation: stage == RecoveryTriggerStage::RecoveryPreparation,
            shutdown: self.fatal.is_shutdown_requested(),
            ..LadderFacts::PASSING
        });
        if !recoverable || gated.is_some() {
            return Ok((event, None));
        }
        let config = self.ladder_config()?;
        let Some((item, matched)) =
            self.match_trigger(&config, stage, preparation_failure.code())?
        else {
            return Ok((event, None));
        };
        let settings = self.instance_stuck_recovery(instance_id, &config)?;
        let resolved = lock(
            &self.registered_instances,
            HostLocation::ReadPreparationInstance,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderInstanceMissing))?;
        let physical = resolved.provenance() == ExecutionBackendProvenance::PhysicalDevice;
        if !settings.enabled
            || ladder_invariants(&LadderFacts {
                physical,
                ..LadderFacts::PASSING
            })
            .is_some()
        {
            return Ok((event, None));
        }
        let profile = ladder_profile(&resolved, &config)?;
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let correlation_id = issuer
            .mint_correlation_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let claim = self.ladder_claim_request(
            &resolved,
            request_id,
            correlation_id,
            causation_id,
            holder_id,
        )?;
        // Workflow #369 H-1, P1: the admitted ladder goes back to the preparation. After a
        // daemon-start preparation, whose dedicated lease was released before this record, it
        // is queued and the pump grants it on the free instance; after a preparation on a held
        // key (an emulator start or restart, a resume's reconnect, a self-check), the key's
        // holder hands that key straight to it at its release (`release_to_preparation_ladder`).
        let pending = PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias,
            trigger: RecoveryLadderTrigger {
                stage,
                run_id: None,
                task_id: None,
                preparation: Some(event),
                failure_code: preparation_failure.code().clone(),
                rule: Some(item.id.clone()),
                matched: Some(matched),
                start: item.start,
            },
            recovery: None,
            recovery_configured: None,
            links: links
                .with_request_id(request_id)
                .with_causation_id(causation_id),
            request_id: *request_id.transport(),
            correlation_id,
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
            claim,
            holder_id,
            start_rung: item.start_rung,
            respect_cooldown: item.respect_cooldown,
            behind_ladder: false,
            config: Arc::clone(&config),
            profile,
            drill: None,
        };
        Ok((event, self.admit_recovery_ladder(pending)?))
    }

    /// Workflow #403: the ladder of a fenced instance (trigger stage `resource_fence`, naming
    /// the unconfirmed failure's code; it matches no trigger item, so it starts at
    /// `trigger_defaults.start_rung` and the cool-down holds it as `trigger_defaults` says),
    /// admitted as every ladder is: `None` when stuck recovery is off for the instance, it is
    /// not a physical one, or its admission is refused or suppressed (recorded as for every
    /// ladder). Its claim waits for the instance's key.
    pub(super) fn resource_fence_ladder(
        &self,
        instance_id: InstanceId,
        cause: CodeStr,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        let config = self.ladder_config()?;
        let settings = self.instance_stuck_recovery(instance_id, &config)?;
        let resolved = lock(
            &self.registered_instances,
            HostLocation::ReadRecoveryLadderInstance,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderInstanceMissing))?;
        let physical = resolved.provenance() == ExecutionBackendProvenance::PhysicalDevice;
        if !settings.enabled
            || ladder_invariants(&LadderFacts {
                physical,
                ..LadderFacts::PASSING
            })
            .is_some()
        {
            return Ok(None);
        }
        let profile = ladder_profile(&resolved, &config)?;
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let correlation_id = issuer
            .mint_correlation_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let claim = self.ladder_claim_request(
            &resolved,
            request_id,
            correlation_id,
            causation_id,
            holder_id,
        )?;
        let pending = PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias,
            trigger: RecoveryLadderTrigger {
                stage: RecoveryTriggerStage::ResourceFence,
                run_id: None,
                task_id: None,
                preparation: None,
                failure_code: cause,
                rule: None,
                matched: None,
                start: RecoveryTriggerStart::Immediately,
            },
            recovery: None,
            recovery_configured: None,
            links: EventLinksDraft::default()
                .with_instance_id(issuer.issue_registered_instance(instance_id))
                .with_request_id(request_id)
                .with_correlation_id(correlation_id)
                .with_causation_id(causation_id),
            request_id: *request_id.transport(),
            correlation_id,
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
            claim,
            holder_id,
            start_rung: config.default_start_rung,
            respect_cooldown: config.default_respect_cooldown,
            behind_ladder: false,
            config: Arc::clone(&config),
            profile,
            drill: None,
        };
        self.admit_recovery_ladder(pending)
    }

    /// Workflow #387 D1c (§3.4): the lease sweep's admission of the bursts direction ① registered
    /// since its last tick, outside every write lock. Each starts a `stability_warnings` ladder,
    /// admitted as every ladder is, whose claim is queued; its recovery package comes from the
    /// instance's startup package when it climbs. Codes the counter met that the catalog does
    /// not list are recorded here.
    pub(super) fn admit_stability_bursts(&self) -> RuntimeHostResult<()> {
        let (bursts, unclassified) = lock(
            &self.stability_counter,
            HostLocation::RecoveryStabilityAdmission,
        )?
        .take();
        for spelling in unclassified {
            self.note_unclassified_code(&spelling)?;
        }
        for burst in bursts {
            if self.fatal.is_shutdown_requested() {
                return Ok(());
            }
            let Some(item) = burst.config.triggers.get(burst.item) else {
                continue;
            };
            let Some(resolved) = lock(
                &self.registered_instances,
                HostLocation::RecoveryStabilityAdmission,
            )?
            .get(&burst.instance_id)
            .cloned() else {
                continue;
            };
            let settings = self.instance_stuck_recovery(burst.instance_id, &burst.config)?;
            let physical = resolved.provenance() == ExecutionBackendProvenance::PhysicalDevice;
            if !settings.enabled
                || ladder_invariants(&LadderFacts {
                    physical,
                    ..LadderFacts::PASSING
                })
                .is_some()
            {
                continue;
            }
            let profile = ladder_profile(&resolved, &burst.config)?;
            let issuer = self.events.issuer();
            let request_id = issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?;
            let correlation_id = issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?;
            let causation_id = issuer
                .mint_causation_id()
                .map_err(|_| runtime_identifier_error())?;
            let holder_id = *issuer
                .mint_holder_id()
                .map_err(|_| runtime_identifier_error())?
                .transport();
            let claim = self.ladder_claim_request(
                &resolved,
                request_id,
                correlation_id,
                causation_id,
                holder_id,
            )?;
            let pending = PendingRecoveryLadder {
                instance_id: burst.instance_id,
                instance_alias: resolved.instance_alias,
                trigger: RecoveryLadderTrigger {
                    stage: RecoveryTriggerStage::StabilityWarnings,
                    run_id: None,
                    task_id: None,
                    preparation: None,
                    failure_code: burst.failure_code,
                    rule: Some(item.id.clone()),
                    matched: Some(RecoveryTriggerMatch::WarningBurst {
                        count: burst.count,
                        window_ms: burst.window_ms,
                        first_unix_ms: burst.first_unix_ms,
                        last_unix_ms: burst.last_unix_ms,
                    }),
                    start: item.start,
                },
                recovery: None,
                recovery_configured: None,
                links: EventLinksDraft::default()
                    .with_instance_id(issuer.issue_registered_instance(burst.instance_id))
                    .with_request_id(request_id)
                    .with_correlation_id(correlation_id)
                    .with_causation_id(causation_id),
                request_id: *request_id.transport(),
                correlation_id,
                causation_id,
                cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
                claim,
                holder_id,
                start_rung: item.start_rung,
                respect_cooldown: item.respect_cooldown,
                behind_ladder: false,
                config: Arc::clone(&burst.config),
                profile,
                drill: None,
            };
            if let Some(ladder) = self.admit_recovery_ladder(pending)? {
                self.enqueue_ladder_claim(ladder)?;
            }
        }
        Ok(())
    }

    /// Workflow #387 D1c-H (B1): reports to direction ① which kind of claim holds `instance_id`'s
    /// key as `scheduler` now has it. Called under the scheduler's lock wherever the key's holder
    /// changes (a grant, a transfer, a release, an expiry, a rollback); it takes only the
    /// counter's leaf lock.
    pub(super) fn note_key_holder(
        &self,
        scheduler: &SeedScheduler,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<()> {
        let holder = scheduler
            .active_lease(instance_id)
            .map(|lease| lease.kind());
        lock(
            &self.stability_counter,
            HostLocation::RecoveryStabilityCount,
        )?
        .set_key_holder(instance_id, holder);
        Ok(())
    }

    /// Workflow #387 D1c (§3.4): direction ①'s count of one committed record, under the append
    /// path's `fact_write_gate`: memory only, nothing recorded and no admission. A record counts
    /// by its outcome's code; a record without an outcome counts for nothing (Workflow #387
    /// D1c-H: a backend open observation carries none; an open failure counts through the
    /// preparation, capture, input or task record that carries it). Its run and request links
    /// and the record its failure entered from name the failure it belongs to; the holder kind
    /// the scheduler last reported for its instance's key says whether manual work committed it
    /// (B1). It takes no lock but the counter's own.
    pub(super) fn count_stability_record(&self, event: &PersistedEvent) -> RuntimeHostResult<()> {
        let Some(instance_id) = event.links().instance_id().copied() else {
            return Ok(());
        };
        let Some(outcome) = event.payload().outcome() else {
            return Ok(());
        };
        let entered_event_id = match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(value)) => value
                .lifecycle_failure()
                .and_then(|failure| failure.entered_event_id()),
            _ => None,
        };
        lock(
            &self.stability_counter,
            HostLocation::RecoveryStabilityCount,
        )?
        .count(&CountedRecord {
            instance_id,
            event_id: *event.event_id(),
            run_id: event.links().run_id().copied(),
            request_id: event.links().request_id().copied(),
            entered_event_id,
            at_unix_ms: event.timestamp_unix_ms(),
            severity: event.severity(),
            spelling: outcome.code().as_str(),
        });
        Ok(())
    }

    /// Workflow #369 H-1: the ladder claim's own synthetic request.
    fn ladder_claim_request(
        &self,
        resolved: &RegisteredInstance,
        request_id: IssuedRequestId,
        correlation_id: IssuedCorrelationId,
        causation_id: IssuedCausationId,
        holder_id: HolderId,
    ) -> RuntimeHostResult<RuntimeRequest> {
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        RuntimeRequest::new(
            request_id,
            correlation_id,
            Some(causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::AcquireLease {
                instance_alias: resolved.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| ladder_invariant(HostCode::RecoveryLadderClaimRequestInvalid))
    }

    /// The two parts of a ladder's admission that can change while it holds (A6 the
    /// scheduling pause, A5 business capacity), read into `facts`. Nothing is recorded.
    fn recovery_gate_open(
        &self,
        pending: &PendingRecoveryLadder,
        facts: &mut LadderFacts,
    ) -> RuntimeHostResult<()> {
        facts.paused = lock(&self.scheduling_pause, HostLocation::GateRecoveryPause)?
            .deferral_code(&pending.instance_alias)
            .is_some();
        if ladder_invariants(facts).is_some() {
            return Ok(());
        }
        facts.capacity = match self.admit_capacity() {
            Ok(_) => true,
            Err(error) if error.is_fatal() => return Err(error),
            Err(_) => false,
        };
        Ok(())
    }

    /// A ladder's admission (A3 shutdown, A6 the scheduling pause, A5 business capacity); a
    /// refusal records `recovery_ladder_suppressed` `admission_denied`.
    fn recovery_admitted(&self, pending: &PendingRecoveryLadder) -> RuntimeHostResult<bool> {
        let mut facts = LadderFacts {
            shutdown: self.fatal.is_shutdown_requested(),
            paused: lock(&self.scheduling_pause, HostLocation::GateRecoveryPause)?
                .deferral_code(&pending.instance_alias)
                .is_some(),
            ..LadderFacts::PASSING
        };
        if ladder_invariants(&facts).is_none() {
            facts.capacity = match self.require_business_capacity(pending.links.clone()) {
                Ok(()) => true,
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(_) => false,
            };
        }
        if ladder_invariants(&facts).is_none() {
            return Ok(true);
        }
        self.append_recovery_ladder_fact(
            pending,
            EventSeverity::Warning,
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason: RecoveryLadderSuppression::AdmissionDenied,
                until_unix_ms: 0,
                rule: pending.trigger.rule.clone(),
            },
        )?;
        Ok(false)
    }

    /// Workflow #369 H-1: the trigger of the run whose task request is `request_id`, when that
    /// run noted a ladder source (a policy run, or a direct run) and committed `task.failed`.
    pub(super) fn ladder_trigger_for(
        &self,
        request_id: RequestId,
    ) -> RuntimeHostResult<Option<LadderTrigger>> {
        let control = lock(
            &self.contained_runs,
            HostLocation::ReadRecoveryLadderTrigger,
        )?
        .get(&request_id)
        .cloned();
        match control {
            Some(control) => control.ladder_trigger(),
            None => Ok(None),
        }
    }

    /// Workflow #369 H-1: decides, inside a run's failed lease end, whether its trigger starts
    /// a ladder: not a direct run under `rules.direct_runs = exempt` (B1), a physical instance
    /// (A1), `stuck_recovery` on, a trigger item that matches the code at the task stage, then
    /// the admission. Returns the admitted ladder.
    pub(super) fn ladder_at_lease_end(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        resolved: &RegisteredInstance,
        trigger: LadderTrigger,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        let instance_id = resolved.instance_id();
        let config = self.ladder_config()?;
        if direct_run_exempt(&config.rules, trigger.direct) {
            return Ok(None);
        }
        let settings = self.instance_stuck_recovery(instance_id, &config)?;
        let physical = resolved.provenance() == ExecutionBackendProvenance::PhysicalDevice;
        if !settings.enabled
            || ladder_invariants(&LadderFacts {
                physical,
                ..LadderFacts::PASSING
            })
            .is_some()
        {
            return Ok(None);
        }
        let Some((item, matched)) = self.match_trigger(
            &config,
            RecoveryTriggerStage::Task,
            &trigger.terminal.failure_code,
        )?
        else {
            return Ok(None);
        };
        // Workflow #336 L2d (R23): a run that binds no recovery package takes the return-home
        // package of its package's game and server: Workflow #373 NAV-3a, the one the
        // standard-pack registry of its game names.
        let (recovery, recovery_configured) = match trigger.recovery {
            Some(binding) => (Some(binding), None),
            None => {
                let failed = trigger
                    .package
                    .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderPackageMissing))?;
                match self.ladder_return_home(&failed.game, &failed.server)? {
                    Some(found) => (
                        Some(found.binding),
                        Some(ConfiguredReturnHome {
                            failed,
                            package_id: found.package_id,
                        }),
                    ),
                    None => (None, None),
                }
            }
        };
        let profile = ladder_profile(resolved, &config)?;
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let claim = self.ladder_claim_request(
            resolved,
            request_id,
            trigger.correlation_id,
            causation_id,
            holder_id,
        )?;
        let pending = PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias.clone(),
            trigger: RecoveryLadderTrigger {
                stage: RecoveryTriggerStage::Task,
                run_id: Some(*trigger.terminal.run_id.transport()),
                task_id: Some(*trigger.terminal.task_id.transport()),
                preparation: None,
                failure_code: trigger.terminal.failure_code.clone(),
                rule: Some(item.id.clone()),
                matched: Some(matched),
                start: item.start,
            },
            recovery,
            recovery_configured,
            links: request
                .event_links(Some(instance_id), None, None)
                .with_request_id(request_id)
                .with_causation_id(causation_id),
            request_id: *request_id.transport(),
            correlation_id: trigger.correlation_id,
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
            claim,
            holder_id,
            start_rung: item.start_rung,
            respect_cooldown: item.respect_cooldown,
            behind_ladder: false,
            config: Arc::clone(&config),
            profile,
            drill: None,
        };
        self.admit_recovery_ladder(pending)
    }

    /// Workflow #387 D1c (B1 `trigger`): after a direct run's failed end released its key, the
    /// ladder its trigger admits is queued as a claim. Under `direct_runs = exempt` (the
    /// default) nothing happens.
    pub(super) fn ladder_after_direct_run(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        token: &LeaseToken,
    ) -> RuntimeHostResult<()> {
        let Some(trigger) = self.ladder_trigger_for(request.request_id())? else {
            return Ok(());
        };
        let Some(resolved) = lock(
            &self.registered_instances,
            HostLocation::ReadRecoveryLadderInstance,
        )?
        .get(&token.instance_id())
        .cloned() else {
            return Ok(());
        };
        match self.ladder_at_lease_end(request, &resolved, trigger)? {
            Some(ladder) => self.enqueue_ladder_claim(ladder),
            None => Ok(()),
        }
    }

    /// Workflow #390 DRILL-1 (forced compromise T-08; coordinator ruling 9 of wave 13):
    /// `DrillRecoveryLadder`, a person's drill of one physical instance's ladder. The request is
    /// recorded as a command. The drill matches no trigger item (stage `drill`): it starts at
    /// `trigger_defaults.start_rung` and the cool-down holds it as
    /// `trigger_defaults.respect_cooldown` says, as a resource fence's ladder does, and it is
    /// admitted as every ladder is (shutdown, the scheduling pause, business capacity, a ladder
    /// that holds or waits for the instance, the cool-down). Its claim is queued (routine, high,
    /// no deadline) and it climbs on the instance's worker like any ladder, every record of its
    /// own tagged `drill`. A plan drill acts on nothing and changes nothing the ladder keeps; a
    /// device drill climbs its rungs on the instance under the same key, admission and fences.
    /// Stuck recovery off for the instance, a suppressed admission or a refused claim refuses
    /// the request after its `command.rejected`. A direct task run still starts no ladder.
    pub(super) fn drill_recovery_ladder(
        &self,
        original: &RuntimeRequest,
        request: &ValidatedRuntimeRequest<'_>,
        instance_alias: &str,
        mode: RecoveryDrillMode,
    ) -> Result<OperationSuccess, RequestFailure> {
        let resolved = self.resolve_instance(instance_alias)?;
        let instance_id = resolved.instance_id();
        let links = self.append_client_command_intent(
            original,
            request,
            instance_id,
            EventAction::RuntimeAction,
            None,
        )?;
        let config = self.ladder_config()?;
        let settings = self.instance_stuck_recovery(instance_id, &config)?;
        if !settings.enabled {
            return Err(self.refuse_drill(
                links,
                RuntimeHostError::request(
                    HostCode::RecoveryLadderDrillDisabled,
                    HostLocation::RecoveryLadderDrill,
                    RuntimeErrorCode::InvalidRequest,
                ),
            ));
        }
        let profile = ladder_profile(&resolved, &config)?;
        let issuer = self.events.issuer();
        let ladder_request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let correlation_id = issuer
            .mint_correlation_id()
            .map_err(|_| runtime_identifier_error())?;
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let claim = self.ladder_claim_request(
            &resolved,
            ladder_request_id,
            correlation_id,
            causation_id,
            holder_id,
        )?;
        let pending = PendingRecoveryLadder {
            instance_id,
            instance_alias: resolved.instance_alias.clone(),
            trigger: RecoveryLadderTrigger {
                stage: RecoveryTriggerStage::Drill,
                run_id: None,
                task_id: None,
                preparation: None,
                failure_code: CodeStr::from(HostCode::RecoveryLadderDrillRequested.code()),
                rule: None,
                matched: None,
                start: RecoveryTriggerStart::Immediately,
            },
            recovery: None,
            recovery_configured: None,
            links: request
                .event_links(Some(instance_id), None, None)
                .with_request_id(ladder_request_id)
                .with_causation_id(causation_id),
            request_id: *ladder_request_id.transport(),
            correlation_id,
            causation_id,
            cooldown_ms: u64::from(settings.cooldown_secs) * 1_000,
            claim,
            holder_id,
            start_rung: config.default_start_rung,
            respect_cooldown: config.default_respect_cooldown,
            behind_ladder: false,
            config: Arc::clone(&config),
            profile,
            drill: Some(mode),
        };
        let pending = match self.admit_or_suppress_ladder(pending)? {
            Ok(pending) => pending,
            Err((reason, until_unix_ms)) => {
                return Err(self.refuse_drill(links, drill_suppressed(reason, until_unix_ms)));
            }
        };
        let drill = RecoveryLadderDrill {
            instance_alias: pending.instance_alias.clone(),
            mode,
            ladder_request_id: pending.request_id,
        };
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    HostLocation::BuildLadderConnection,
                    &error,
                ))
            })?;
        let claim = pending.claim.clone();
        let claim_alias = pending.instance_alias.clone();
        let lease_ttl_ms = pending.claim_ttl_ms();
        if let Err(failure) = self.request_host_claim_work(
            HostClaim {
                request: &claim,
                instance_alias: &claim_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::Ladder,
                priority: LeasePriority::High,
                lease_ttl_ms,
            },
            HostClaimWork::RecoveryLadder(Box::new(pending)),
        ) {
            if failure.poison_runtime || failure.error.is_fatal() {
                return Err(failure);
            }
            return Err(self.refuse_drill(links, *failure.error));
        }
        let validated = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                EventAction::RuntimeAction,
                EffectDisposition::Performed,
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&validated)),
            result: RuntimeResult::RecoveryLadderDrillQueued { drill },
            failure_outcome: None,
        })
    }

    /// Workflow #390 DRILL-1: a refused drill's `command.rejected` with `error`'s outcome, and
    /// the denial that names it.
    fn refuse_drill(&self, links: EventLinksDraft, error: RuntimeHostError) -> RequestFailure {
        match self.append_event(
            EventSeverity::Error,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::rejected_with_outcome(
                EventAction::RuntimeAction,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                error.outcome(),
                AuditInput::new(),
            ),
        ) {
            Ok(rejected) => RequestFailure::request(
                error,
                RuntimeReceiptState::Denied,
                Some(terminal(&rejected)),
            ),
            Err(failure) => failure,
        }
    }

    /// Admits a ladder, or records why it is suppressed, naming the trigger item that matched:
    /// a ladder already holds or waits for the instance and `rules.already_running` is
    /// `suppress` (B3; Workflow #369 §5.3, read from the scheduler), or the cool-down window is
    /// open and the ladder respects it. Under `already_running = queue` it is admitted behind
    /// the other ladder. Under `cooldown_opens_on = triggered` every admission opens the window
    /// from now (B4). Returns the admitted ladder.
    fn admit_recovery_ladder(
        &self,
        pending: PendingRecoveryLadder,
    ) -> RuntimeHostResult<Option<PendingRecoveryLadder>> {
        Ok(self.admit_or_suppress_ladder(pending)?.ok())
    }

    /// `admit_recovery_ladder`, answering a suppressed ladder with its recorded reason and the
    /// `until_unix_ms` its `recovery_ladder_suppressed` names (Workflow #390 DRILL-1: a drill's
    /// request is refused by it). A plan drill reads the cool-down window and never opens it.
    fn admit_or_suppress_ladder(
        &self,
        mut pending: PendingRecoveryLadder,
    ) -> RuntimeHostResult<LadderAdmission> {
        if !self.recovery_admitted(&pending)? {
            return Ok(Err((RecoveryLadderSuppression::AdmissionDenied, 0)));
        }
        let now_unix_ms = self.clock.sample()?.unix_ms;
        let ladder_active = lock(&self.scheduler, HostLocation::ReadRecoveryLadderHold)?
            .ladder_active(pending.instance_id);
        let until_unix_ms = {
            let mut windows = lock(&self.recovery_ladders, HostLocation::AdmitRecoveryLadder)?;
            let until_unix_ms = windows
                .get(&pending.instance_id)
                .map_or(0, |window| window.until_unix_ms);
            if opens_cooldown_window(&pending.config.rules, true) && !pending.plan_drill() {
                windows
                    .entry(pending.instance_id)
                    .or_default()
                    .until_unix_ms = now_unix_ms.saturating_add(pending.cooldown_ms);
            }
            until_unix_ms
        };
        let reason = match admission(
            &pending.config.rules,
            ladder_active,
            pending.respect_cooldown && now_unix_ms < until_unix_ms,
        ) {
            Admission::Admitted { behind_ladder } => {
                pending.behind_ladder = behind_ladder;
                return Ok(Ok(pending));
            }
            Admission::Suppressed(reason) => reason,
        };
        self.append_recovery_ladder_fact(
            &pending,
            EventSeverity::Info,
            RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                reason,
                until_unix_ms,
                rule: pending.trigger.rule.clone(),
            },
        )?;
        Ok(Err((reason, until_unix_ms)))
    }

    /// Workflow #369 H-1, P1 (review #716 follow-up): the explicit release `request_id` of the
    /// held key (`token` on `connection_id`) on which a preparation ran that admitted `ladder`.
    /// The release hands the key straight to the ladder's claim (`release_lease_to_claim`: the
    /// claim's own `lease.requested` + `scheduler.queued`, then `lease.transferred`, no grant
    /// of its own), so no claim queued earlier can take the instance first. With no hand-off (a
    /// pause closed the gate since the admission, or a destructive step is open), or for a
    /// ladder that waits in the queue (§3.7, B3), the ladder waits as a claim and the key is
    /// released; a refused hand-off is recorded as a refused queueing is, and the key is
    /// released.
    pub(super) fn release_to_preparation_ladder(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        token: &LeaseToken,
        connection_id: ConnectionId,
        ladder: PendingRecoveryLadder,
    ) -> Result<(), RequestFailure> {
        if !ladder.hands_off() {
            self.enqueue_ladder_claim(ladder)
                .map_err(RequestFailure::poison_without_terminal)?;
            return self
                .release_lease(request, request_id, token, connection_id, None)
                .map(|_| ());
        }
        let claim_connection =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    HostLocation::BuildLadderConnection,
                    &error,
                ))
            })?;
        let claim = ladder.claim.clone();
        let instance_alias = ladder.instance_alias.clone();
        let handed = self.release_lease_to_claim(
            request,
            request_id,
            token,
            connection_id,
            HostClaim {
                request: &claim,
                instance_alias: &instance_alias,
                holder_id: ladder.holder_id,
                connection_id: claim_connection,
                kind: ClaimKind::Ladder,
                priority: LeasePriority::High,
                lease_ttl_ms: ladder.claim_ttl_ms(),
            },
            HostClaimWork::RecoveryLadder(Box::new(ladder.clone())),
        );
        match handed {
            Ok(true) => return Ok(()),
            Ok(false) => self
                .enqueue_ladder_claim(ladder)
                .map_err(RequestFailure::poison_without_terminal)?,
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(failure);
            }
            Err(failure) => self
                .append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    ladder.links,
                    None,
                )
                .map_err(RequestFailure::poison_without_terminal)?,
        }
        self.release_lease(request, request_id, token, connection_id, None)
            .map(|_| ())
    }

    /// Workflow #369 H-1: queues an admitted ladder's claim (routine, high, no deadline); the
    /// pump grants it, and its instance's worker climbs it. A refusal to queue is recorded.
    pub(super) fn enqueue_ladder_claim(
        &self,
        pending: PendingRecoveryLadder,
    ) -> RuntimeHostResult<()> {
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler(HostLocation::BuildLadderConnection, &error)
            })?;
        let claim = pending.claim.clone();
        let links = pending.links.clone();
        let instance_alias = pending.instance_alias.clone();
        let holder_id = pending.holder_id;
        let lease_ttl_ms = pending.claim_ttl_ms();
        match self.request_host_claim_work(
            HostClaim {
                request: &claim,
                instance_alias: &instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::Ladder,
                priority: LeasePriority::High,
                lease_ttl_ms,
            },
            HostClaimWork::RecoveryLadder(Box::new(pending)),
        ) {
            Ok(()) => Ok(()),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(&failure.error),
                links,
                None,
            ),
        }
    }

    /// Climbs one ladder on its instance's worker, holding `key`, and releases the key once
    /// whatever the outcome. Only a fatal failure is returned. While it climbs, direction ①
    /// counts none of its instance's records: they are the ladder's own work (§3.4).
    pub(super) fn run_recovery_ladder(
        &self,
        pending: &PendingRecoveryLadder,
        mut key: HostKey,
    ) -> RuntimeHostResult<()> {
        let marked = lock(
            &self.stability_counter,
            HostLocation::RecoveryStabilityCount,
        )
        .map(|mut counter| counter.set_climbing(pending.instance_id, true));
        let climbed = marked.and_then(|()| self.climb_recovery_ladder(pending, &mut key));
        // Workflow #403: a ladder that ends with its instance still fenced did not prove the
        // close; the instance waits for a person (set before the key is released). Workflow #390
        // DRILL-1: a plan drill tried nothing, so a fence's stage stays as it was.
        let awaited = if pending.plan_drill() {
            Ok(())
        } else {
            self.resource_fence_awaits_person(pending.instance_id)
        };
        let unmarked = lock(
            &self.stability_counter,
            HostLocation::RecoveryStabilityCount,
        )
        .map(|mut counter| counter.set_climbing(pending.instance_id, false));
        let released = self.release_host_key(&key);
        climbed.and(awaited).and(unmarked).and(released)
    }

    fn climb_recovery_ladder(
        &self,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
    ) -> RuntimeHostResult<()> {
        let fence_ladder = pending.trigger.stage == RecoveryTriggerStage::ResourceFence;
        // Workflow #403: a person's passing preparation lifted the fence while its ladder
        // waited: nothing is left to prove.
        if fence_ladder && !self.resource_fenced(pending.instance_id)? {
            return Ok(());
        }
        if !self.recovery_admitted(pending)? {
            return Ok(());
        }
        // B2 (§2.2a): a preparation ladder whose preparation a later one superseded.
        if let Some(trigger) = pending.trigger.preparation {
            let current = lock(
                &self.recovery_ladders,
                HostLocation::GateRecoveryPreparation,
            )?
            .get(&pending.instance_id)
            .and_then(|window| window.preparation);
            if preparation_superseded_suppresses(&pending.config.rules, current != Some(trigger)) {
                return self.append_recovery_ladder_fact(
                    pending,
                    EventSeverity::Info,
                    RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                        reason: RecoveryLadderSuppression::PreparationSuperseded,
                        until_unix_ms: 0,
                        rule: pending.trigger.rule.clone(),
                    },
                );
            }
        }
        let now_unix_ms = self.clock.sample()?.unix_ms;
        // B3 `queue`: a ladder admitted behind another is judged against the cool-down again
        // when it gets the key.
        if pending.behind_ladder && pending.respect_cooldown {
            let until_unix_ms = lock(&self.recovery_ladders, HostLocation::AdmitRecoveryLadder)?
                .get(&pending.instance_id)
                .map_or(0, |window| window.until_unix_ms);
            if now_unix_ms < until_unix_ms {
                return self.append_recovery_ladder_fact(
                    pending,
                    EventSeverity::Info,
                    RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                        reason: RecoveryLadderSuppression::Cooldown,
                        until_unix_ms,
                        rule: pending.trigger.rule.clone(),
                    },
                );
            }
        }
        // B4 `admitted` (Workflow #369 §5.3): only a ladder that got the key opens its window;
        // a plan drill, which acts on nothing, opens none (Workflow #390 DRILL-1).
        if opens_cooldown_window(&pending.config.rules, false) && !pending.plan_drill() {
            lock(
                &self.recovery_ladders,
                HostLocation::OpenRecoveryLadderWindow,
            )?
            .entry(pending.instance_id)
            .or_default()
            .until_unix_ms = now_unix_ms.saturating_add(pending.cooldown_ms);
        }
        let resolved = lock(
            &self.registered_instances,
            HostLocation::ReadRecoveryLadderInstance,
        )?
        .get(&pending.instance_id)
        .cloned()
        .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderInstanceMissing))?;
        // A ladder without a failed run (a preparation's, a burst's; M387 §3.4) has no failed
        // task identity. Resolve its recovery binding from the configured startup resource
        // through the same hash/containment admission owner. Each eventual run still admits its
        // own material, so a mutable locator cannot carry this identity read across a changed
        // package. A fence's ladder never runs its return-home rung (Workflow #403).
        let mut effective = pending.clone();
        let mut recovery_resolution_error = None;
        if pending.trigger.run_id.is_none()
            && !fence_ladder
            && let Some(startup) = self.startup_packages()?.get(&pending.instance_id)
        {
            match super::contained_task::prepare_contained_task(
                &pending.instance_alias,
                startup,
                self.execution()?.vision_provider(),
                Instant::now() + Duration::from_millis(startup.response_deadline_ms()),
            ) {
                Ok(prepared) => {
                    if let Some(binding) = startup.recovery() {
                        effective.recovery = Some(binding.clone());
                    } else if let Some(found) =
                        self.ladder_return_home(prepared.game(), prepared.server())?
                    {
                        effective.recovery = Some(found.binding);
                        effective.recovery_configured = Some(ConfiguredReturnHome {
                            failed: PackageIdentity::of(&prepared),
                            package_id: found.package_id,
                        });
                    }
                }
                Err(failure) => {
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::OperationCleanup,
                        RuntimeLifecycleFailure::Host(&failure.error),
                        pending.links.clone(),
                        None,
                    )?;
                    if failure.poison_runtime || failure.error.is_fatal() {
                        return Err(*failure.error);
                    }
                    recovery_resolution_error = Some(failure.error.code());
                }
            }
        }
        let pending = &effective;
        let startup_package = self.startup_packages()?.contains_key(&pending.instance_id);
        let emulator_control = resolved
            .adb_endpoint
            .as_ref()
            .and_then(ResolvedInstanceEndpoint::discovered_binding)
            .is_some();
        // §4.1: the plan follows the configured rungs; the rungs before the start rung are
        // skipped, every other rung whose prerequisite is missing is skipped for that reason.
        // Workflow #398 RD-4 (model-398 §6.2, §6.6): the rungs are the instance's profile's when
        // it has one, and a start beyond them is as `rules.start_beyond_profile` says: under
        // `exhaust` every rung is before it, under `last_rung` the climb starts at the last one.
        let configured = pending
            .profile
            .as_ref()
            .map_or(&pending.config.rungs, |profile| &profile.rungs);
        let start_rung = pending
            .config
            .start_beyond_profile
            .start_rung(pending.start_rung, configured.len());
        let plan = configured
            .iter()
            .enumerate()
            .map(|(index, rung)| {
                let reason =
                    if index + 1 < start_rung {
                        Some(RecoveryRungSkipReason::BeforeStartRung)
                    } else {
                        match rung.action() {
                            // Workflow #403: the return-home rung never prepares the instance
                            // again, so it cannot prove a fenced instance's close.
                            RecoveryRung::ReturnHome if fence_ladder => {
                                Some(RecoveryRungSkipReason::ResourceFence)
                            }
                            RecoveryRung::ReturnHome => (pending.recovery.is_none()
                                && recovery_resolution_error.is_none())
                            .then_some(RecoveryRungSkipReason::NoRecoveryPackage),
                            RecoveryRung::ApplicationRestart => (!startup_package)
                                .then_some(RecoveryRungSkipReason::NoStartupPackage),
                            RecoveryRung::EmulatorRestart => (!emulator_control)
                                .then_some(RecoveryRungSkipReason::NoEmulatorControl),
                        }
                    };
                (rung, reason)
            })
            .collect::<Vec<_>>();
        let rungs = plan
            .iter()
            .map(|(rung, reason)| RecoveryRungPlan {
                rung: rung.action(),
                state: if reason.is_some() {
                    RecoveryRungState::Skipped
                } else {
                    RecoveryRungState::Pending
                },
                reason: *reason,
            })
            .collect();
        self.append_recovery_ladder_fact(
            pending,
            EventSeverity::Info,
            RuntimeLifecyclePhase::RecoveryLadderStarted {
                trigger: pending.trigger.clone(),
                rungs,
                config_digest: pending.config.digest().to_owned(),
                profile: pending.profile.as_ref().map(|profile| profile.name.clone()),
            },
        )?;
        let mut rungs_tried = 0_u8;
        // Review L6 (#666): the holding interruption that ended the last rung that ran, if one
        // did; a rung that ran and failed on its own clears it.
        let mut last_interruption = None;
        // The outcome of the last rung that ran and failed: what an exhausted climb ends on.
        let mut last_failure = None;
        for (rung_config, skipped) in plan {
            let rung = rung_config.action();
            // Workflow #369 H-3: the holding check before every rung (so after every rung run
            // but the last one, whose own failure decides the outcome; review L6).
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return self.finish_interrupted_ladder(
                    pending,
                    key,
                    Some(rung),
                    reason,
                    rungs_tried,
                );
            }
            if let Some(reason) = skipped {
                self.append_recovery_rung_finished(
                    pending,
                    rung,
                    RecoveryRungOutcome::Skipped,
                    None,
                    Some(rung_skip_outcome(reason)),
                    None,
                )?;
                continue;
            }
            // Workflow #390 DRILL-1: a plan drill never acts on the device; the rung it would
            // have run is recorded skipped.
            if pending.plan_drill() {
                self.append_recovery_rung_finished(
                    pending,
                    rung,
                    RecoveryRungOutcome::Skipped,
                    None,
                    Some(rung_skip_outcome(RecoveryRungSkipReason::DrillPlan)),
                    None,
                )?;
                continue;
            }
            rungs_tried += 1;
            let attempt = match rung_config {
                RungConfig::ReturnHome {
                    configured_package_deadline_ms,
                } => match recovery_resolution_error {
                    Some(reason) => RungAttempt::Failed {
                        run_id: None,
                        reason,
                    },
                    None => {
                        self.recovery_return_home(pending, *configured_package_deadline_ms, key)?
                    }
                },
                RungConfig::ApplicationRestart {
                    adb_probe_ms,
                    entry_hold_ms,
                } => self.recovery_application_restart(
                    pending,
                    &resolved,
                    (*adb_probe_ms, *entry_hold_ms),
                    key,
                )?,
                RungConfig::EmulatorRestart {
                    stop_bound_ms,
                    start_bound_ms,
                    readiness_window_ms,
                    readiness_backoff_ms,
                    readiness_poll_ms,
                } => self.recovery_emulator_restart(
                    pending,
                    &resolved,
                    &EmulatorRestartBounds {
                        stop_bound_ms: *stop_bound_ms,
                        start_bound_ms: *start_bound_ms,
                        readiness_window_ms: *readiness_window_ms,
                        readiness_backoff_ms,
                        readiness_poll_ms: *readiness_poll_ms,
                    },
                    key,
                )?,
            };
            match attempt {
                RungAttempt::Recovered { run_id } => {
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::Recovered,
                        Some(run_id),
                        None,
                        None,
                    )?;
                    return self.append_recovery_ladder_fact(
                        pending,
                        EventSeverity::Info,
                        RuntimeLifecyclePhase::RecoveryLadderFinished {
                            ladder_result: RecoveryLadderOutcome::Recovered,
                            rungs_tried,
                            outcome: None,
                        },
                    );
                }
                RungAttempt::EnvironmentReady { event } => {
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::EnvironmentReady,
                        None,
                        None,
                        Some(event),
                    )?;
                    return self.append_recovery_ladder_fact(
                        pending,
                        EventSeverity::Info,
                        RuntimeLifecyclePhase::RecoveryLadderFinished {
                            ladder_result: RecoveryLadderOutcome::EnvironmentReady,
                            rungs_tried,
                            outcome: None,
                        },
                    );
                }
                RungAttempt::Skipped { reason } => {
                    rungs_tried -= 1;
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::Skipped,
                        None,
                        Some(rung_skip_outcome(reason)),
                        None,
                    )?;
                }
                RungAttempt::Failed { run_id, reason } => {
                    last_interruption = is_hold_interruption(reason).then_some(reason);
                    let failure = rung_failure_outcome(pending, key, reason);
                    last_failure = Some(failure.clone());
                    self.append_recovery_rung_finished(
                        pending,
                        rung,
                        RecoveryRungOutcome::Failed,
                        run_id,
                        Some(failure),
                        None,
                    )?;
                }
            }
        }
        // Review L6 (#666), §5.10: after the last rung the climb is exhausted at Error when the
        // rungs that ran failed on their own, whatever holds at this moment; it ends at Warning
        // only when the last rung that ran was itself ended by a holding interruption.
        if let Some(reason) = last_interruption {
            return self.finish_interrupted_ladder(pending, key, None, reason, rungs_tried);
        }
        // Workflow #390 DRILL-1: a plan drill's climb tried no rung; it ends `planned`, no
        // failure.
        if pending.plan_drill() {
            return self.append_recovery_ladder_fact(
                pending,
                EventSeverity::Info,
                RuntimeLifecyclePhase::RecoveryLadderFinished {
                    ladder_result: RecoveryLadderOutcome::Planned,
                    rungs_tried,
                    outcome: None,
                },
            );
        }
        // Ledger revision 3: the exhausted climb's outcome (category error) carries the last
        // failed rung's outcome as its cause; none when every rung was skipped.
        let exhausted = Outcome::new(HostCode::RecoveryLadderExhausted);
        self.append_recovery_ladder_fact(
            pending,
            ladder_exhausted_severity(false),
            RuntimeLifecyclePhase::RecoveryLadderFinished {
                ladder_result: RecoveryLadderOutcome::Exhausted,
                rungs_tried,
                outcome: Some(match last_failure {
                    Some(failure) => exhausted.caused_by(failure),
                    None => exhausted,
                }),
            },
        )
    }

    /// Workflow #369 H-3 (ruling Q8): the holding check, by the rules that stay in code (§2.2b):
    /// with shutdown requested the reason is `recovery_ladder_shutdown_requested`; a ladder that
    /// no longer holds its key (in practice only shutdown) has lost it
    /// (`recovery_ladder_key_lost`; review L3); while holding, an install drain is
    /// `recovery_ladder_drain_requested`, and a scheduling pause or a capacity refusal (the two
    /// parts of `recovery_gate_open`) is `recovery_admission_denied`.
    fn ladder_hold_interrupted(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
    ) -> RuntimeHostResult<Option<&'static str>> {
        let mut facts = LadderFacts {
            shutdown: self.fatal.is_shutdown_requested(),
            ..LadderFacts::PASSING
        };
        if ladder_invariants(&facts).is_none() {
            facts.holds_key = self.holds_key(key)?;
        }
        if ladder_invariants(&facts).is_none() {
            facts.draining = self.lifecycle_draining()?;
        }
        if ladder_invariants(&facts).is_none() {
            self.recovery_gate_open(pending, &mut facts)?;
        }
        // Workflow #398 RD-2, A9: a handset that left `device` interrupts the climb.
        if ladder_invariants(&facts).is_none() {
            facts.device_linked = self.device_link_hold(pending.instance_id)?.is_none();
        }
        Ok(ladder_invariants(&facts).map(LadderInvariant::interruption))
    }

    /// Workflow #369 H-3 (ruling Q8; review L-7): an interrupted climb records the current rung
    /// (if one was about to start) `failed` with the interruption, then
    /// `recovery_ladder_finished` `exhausted` at Warning: no error point (§2.2b A8). The key is
    /// released once by the caller.
    fn finish_interrupted_ladder(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
        rung: Option<RecoveryRung>,
        reason: &'static str,
        rungs_tried: u8,
    ) -> RuntimeHostResult<()> {
        if let Some(rung) = rung {
            self.append_recovery_rung_finished(
                pending,
                rung,
                RecoveryRungOutcome::Failed,
                None,
                Some(rung_failure_outcome(pending, key, reason)),
                None,
            )?;
        }
        // Ledger revision 3: the interrupted climb's outcome (category warning) carries the
        // interruption as its cause.
        self.append_recovery_ladder_fact(
            pending,
            ladder_exhausted_severity(true),
            RuntimeLifecyclePhase::RecoveryLadderFinished {
                ladder_result: RecoveryLadderOutcome::Exhausted,
                rungs_tried,
                outcome: Some(interrupted(rung_reason_outcome(pending, key, reason))),
            },
        )
    }

    /// Q-5: a renewal the ladder could not make fails the step with the renewal's own refusal
    /// (`lease_expired`, `lease_missing`); the next holding check ends the climb.
    fn ladder_step_renewal(
        &self,
        key: &mut HostKey,
        lease_ttl_ms: u64,
    ) -> RuntimeHostResult<Option<RungAttempt>> {
        match self.renew_host_key(key, lease_ttl_ms) {
            Ok(()) => Ok(None),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => Ok(Some(RungAttempt::Failed {
                run_id: None,
                reason: failure.error.code(),
            })),
        }
    }

    /// R1: the failed run's bound recovery package (or the configured return-home package,
    /// Workflow #336 L2d) as a standalone contained task under the ladder's causation id.
    fn recovery_return_home(
        &self,
        pending: &PendingRecoveryLadder,
        configured_package_deadline_ms: u64,
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        let binding = pending
            .recovery
            .as_ref()
            .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderRecoveryPackageMissing))?;
        // Workflow #336 L2d (R24): the configured package takes the configured deadline
        // (Workflow #387 D1c, `rungs[return_home].configured_package_deadline_ms`); a bound one
        // keeps the default.
        let request = ContainedTaskRequest::new(binding.package_path()).and_then(|request| {
            if pending.recovery_configured.is_some() {
                request.with_response_deadline_ms(configured_package_deadline_ms)
            } else {
                Ok(request)
            }
        });
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: error.code().as_str(),
                });
            }
        };
        self.run_recovery_rung_package(
            &PendingStartupPackage {
                instance_id: pending.instance_id,
                instance_alias: pending.instance_alias.clone(),
                request,
                causation_id: pending.causation_id,
                control_request_id: pending.request_id,
                run: HostPackageRun::ReturnHome,
                recovery_rung: true,
                configured_return_home: pending.recovery_configured.clone().map(Box::new),
            },
            false,
            pending,
            key,
        )
    }

    /// R2: restarts the instance's assigned application. Workflow #369-2: the entry checks the
    /// startup package's run would make come first (review M2), and a known unavailable channel
    /// or an ADB baseline that does not answer within `adb_probe_ms` skips the rung (or fails
    /// it, B5) without touching the game. Then the assigned application is force-stopped on the
    /// ladder's key (`stop_assigned_application`), and the startup package is scheduled
    /// (`startup_package_scheduled` under the ladder's links) and run, which launches the game
    /// and confirms its page. A failed stop fails the rung with its code; after the stop the
    /// rung no longer skips. A startup package that cannot be admitted, or whose prerequisite
    /// chain or startup compatibility refuses it, is run without the stop, so its refusal is
    /// recorded as before and the game is left alone. `bounds` are the rung's ADB probe and
    /// entry hold (§4.2).
    fn recovery_application_restart(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
        bounds: (u64, u64),
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        let (adb_probe_ms, entry_hold_ms) = bounds;
        let margin_ms = pending.config.hold.margin_ms;
        let request = self
            .startup_packages()?
            .get(&pending.instance_id)
            .cloned()
            .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderStartupPackageMissing))?;
        // Workflow #369 Q-5 (B1): the entry's ADB probe and the stop run on the key.
        if let Some(failed) =
            self.ladder_step_renewal(key, entry_hold_ms.saturating_add(margin_ms))?
        {
            return Ok(failed);
        }
        let stops = match self.recovery_restart_entry(pending, &request, adb_probe_ms)? {
            RestartEntry::Unavailable { skip, refused } => {
                return Ok(unavailable_entry(&pending.config.rules, skip, refused));
            }
            RestartEntry::Fenced { fence } => {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: fence,
                });
            }
            RestartEntry::Stop => true,
            RestartEntry::RunOnly => false,
        };
        if stops && let Err(reason) = self.stop_assigned_application(pending, resolved, key)? {
            return Ok(RungAttempt::Failed {
                run_id: None,
                reason,
            });
        }
        // Workflow #403: after the stop, a fenced instance is prepared again on the key; only a
        // passing self-check proves the close (the preparation lifts the fence) and lets the rung
        // go on to its startup run.
        if stops && self.resource_fenced(pending.instance_id)? {
            if let Some(failed) =
                self.ladder_step_renewal(key, entry_hold_ms.saturating_add(margin_ms))?
            {
                return Ok(failed);
            }
            let instance_guard = self
                .instance_guard(pending.instance_id)
                .map_err(|failure| *failure.error)?;
            let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
            let (check, _, _) =
                self.prepare_recovery_connection(resolved, pending.links.clone(), &admission, key)?;
            drop(admission);
            if !(check.capture.ok && check.touch.ok && check.failure_code.is_none()) {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: HostCode::RecoveryEnvironmentNotReady.as_str(),
                });
            }
        }
        let mut startup = self
            .prepare_startup_package(resolved, pending.links.clone(), pending.request_id)
            .map_err(|failure| *failure.error)?
            .ok_or_else(|| ladder_invariant(HostCode::RecoveryLadderStartupPackageMissing))?;
        startup.recovery_rung = true;
        self.run_recovery_rung_package(&startup, stops, pending, key)
    }

    /// Workflow #369-2 (review M2): the checks the rung's startup package run makes before its
    /// run (`prepare_package_run`), made before the game is stopped: its admission, its
    /// prerequisite chain and its startup compatibility (no resource readings; review L-R5-1),
    /// a known unavailable capture or input channel its entry needs, and the ADB baseline the
    /// stop and the launch need (`adb_probe_ms`, the rung's configured probe). The run makes
    /// the admission refusals again and records them as before.
    fn recovery_restart_entry(
        &self,
        pending: &PendingRecoveryLadder,
        request: &ContainedTaskRequest,
        adb_probe_ms: u64,
    ) -> RuntimeHostResult<RestartEntry> {
        let material_deadline =
            Instant::now() + Duration::from_millis(request.response_deadline_ms());
        let prepared = match super::contained_task::prepare_contained_task(
            &pending.instance_alias,
            request,
            self.execution()?.vision_provider(),
            material_deadline,
        ) {
            Ok(prepared) => prepared,
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(*failure.error);
            }
            Err(_) => return Ok(RestartEntry::RunOnly),
        };
        if let Err(failure) =
            self.resolve_prerequisite_chain(&pending.instance_alias, &prepared, false, || {
                Ok(material_deadline)
            })
        {
            if failure.poison_runtime || failure.error.is_fatal() {
                return Err(*failure.error);
            }
            return Ok(RestartEntry::RunOnly);
        }
        if prepared.startup_incompatibility().is_some() {
            return Ok(RestartEntry::RunOnly);
        }
        let (capture, input) = prepared.recovery_entry_channels();
        if let Some(unavailable) =
            self.recovery_entry_unavailable(pending.instance_id, capture, input)?
        {
            let refused = unavailable.as_str();
            if let Some(skip) = recovery_entry_skip(refused) {
                return Ok(RestartEntry::Unavailable { skip, refused });
            }
        }
        if let Err(error) = self.execution()?.probe_adb_baseline_until(
            &pending.instance_alias,
            Instant::now() + Duration::from_millis(adb_probe_ms),
            &|| self.fatal.is_shutdown_requested(),
        ) {
            if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
                let fence = self.fence_recovery_adb(
                    pending,
                    RuntimeHostError::execution(LADDER_OPERATION, &error),
                )?;
                return Ok(RestartEntry::Fenced { fence });
            }
            return Ok(RestartEntry::Unavailable {
                skip: RecoveryRungSkipReason::AdbUnavailable,
                refused: HostCode::RecoveryLadderAdbNotReady.as_str(),
            });
        }
        Ok(RestartEntry::Stop)
    }

    /// Workflow #403, #387 D1c-G: an ADB child or pipe cleanup left unconfirmed by the ladder's
    /// own ADB check (the rung 2 entry probe, the readiness wait's boot check and probes) belongs
    /// to the instance the ladder climbs on: it fences that instance as every unconfirmed close
    /// of its own resources does (`settle_unconfirmed_resources`: one `instance_resources_fenced`
    /// caused by `error`, `settle_unconfirmed_adb`), and the rung fails with the fence's code;
    /// the ladder goes on. Only what the contract keeps whole-Runtime (resources no registered
    /// instance owns, a ledger failure, a shutdown) retains the owner and ends the Runtime: that
    /// fatal is returned.
    fn fence_recovery_adb(
        &self,
        pending: &PendingRecoveryLadder,
        error: RuntimeHostError,
    ) -> RuntimeHostResult<&'static str> {
        self.settle_unconfirmed_adb(error, pending.instance_id, pending.links.clone())
            .map(|fence| fence.code())
    }

    /// Workflow #369-2: force-stops the instance's assigned application as a host-minted
    /// `ApplicationLifecycle { Stop }` request under the ladder's causation id:
    /// `command.received` / `command.validated`, then the existing `application.intent` /
    /// `application.completed`. Workflow #369 H-3: it runs on the ladder's key (its holder and
    /// its connection), with no lease of its own, and leaves the key with the ladder. A
    /// non-fatal failure is recorded as a runtime lifecycle failure and returned as the rung's
    /// reason.
    fn stop_assigned_application(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
        key: &HostKey,
    ) -> RuntimeHostResult<Result<(), &'static str>> {
        let action = actingcommand_contract::ApplicationLifecycleAction::Stop;
        let instance_alias = resolved.instance_alias.as_str();
        let issuer = self.events.issuer();
        let request_id = issuer
            .mint_request_id()
            .map_err(|_| runtime_identifier_error())?;
        let correlation_id = issuer
            .mint_correlation_id()
            .map_err(|_| runtime_identifier_error())?;
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let message = RuntimeRequest::new(
            request_id,
            correlation_id,
            Some(pending.causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: instance_alias.to_owned(),
                holder_id: key.token.holder_id(),
                action,
            },
        )
        .map_err(|_| ladder_invariant(HostCode::RecoveryApplicationStopRequestInvalid))?;
        let validated = message
            .validate()
            .map_err(|_| ladder_invariant(HostCode::RecoveryApplicationStopRequestInvalid))?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
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
                links.clone(),
                payload,
            )?;
        }
        match self.application_control(&validated, &key.token, action, key.connection_id, None) {
            Ok(_) => Ok(Ok(())),
            Err(mut failure) => {
                // Workflow #403: an unconfirmed close fences the instance (or, when no single
                // instance owns it, retains the owner and marks the Runtime fatal, as on every
                // failed device path); the key itself stays with the ladder.
                if let super::resource_fence::UnconfirmedResources::Fenced(fence) = self
                    .settle_unconfirmed_resources(
                        &failure.error,
                        Some(pending.instance_id),
                        EventLinksDraft::default(),
                    )?
                {
                    *failure.error = fence;
                    failure.poison_runtime = false;
                }
                if failure.poison_runtime || failure.error.is_fatal() {
                    return Err(*failure.error);
                }
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    links,
                    None,
                )?;
                Ok(Err(failure.error.code()))
            }
        }
    }

    /// R3: the existing provider confirms the old process gone before the current instance
    /// is started. One admission guard spans both actions and the first readiness attempt.
    /// Workflow #381 B F3: a Stop whose readiness wait expired (`emulator_control_wait_timeout`)
    /// still leads on to the Start, with the Stop's rejected event as the `stop` of
    /// `recovery_environment_ready` and no `recovery_instance_stopped`; every other Stop failure
    /// fails the rung without a Start.
    /// Workflow #369 H-3: Stop and Start run on the ladder's key (the control's fence admits
    /// it, and the retained session is closed under it), each renewed before it to its
    /// configured bound and the margin (review L-3; Workflow #387 D1c); the readiness wait
    /// renews to its window's end. Workflow #369-1: the rung then waits for a usable environment
    /// (`await_recovery_readiness`), records `recovery_environment_ready`, and runs the
    /// instance's startup package on the key; only that run's success recovers the rung.
    fn recovery_emulator_restart(
        &self,
        pending: &PendingRecoveryLadder,
        resolved: &RegisteredInstance,
        bounds: &EmulatorRestartBounds<'_>,
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        let margin_ms = pending.config.hold.margin_ms;
        let instance_guard = self
            .instance_guard(pending.instance_id)
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, HostLocation::LockInstanceAdmission)?;
        let mut stopped = None;
        for action in [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start] {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason,
                });
            }
            let bound_ms = if action == EmulatorInstanceAction::Stop {
                bounds.stop_bound_ms
            } else {
                bounds.start_bound_ms
            };
            if let Some(failed) =
                self.ladder_step_renewal(key, bound_ms.saturating_add(margin_ms))?
            {
                return Ok(failed);
            }
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                pending.links.clone(),
                CommandPayloadDraft::received(action.event_action(), AuditInput::new()),
            )?;
            // Read the registered binding again after Stop; Start discovers its new port.
            let current = self
                .resolve_instance(&resolved.instance_alias)
                .map_err(|failure| *failure.error)?;
            let driven = match self.drive_emulator_control_while_guarded(
                &current,
                pending.links.clone(),
                action,
                pending.request_id,
                &admission,
                &key.token,
            ) {
                Ok(driven) => driven,
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(failure)
                    if action == EmulatorInstanceAction::Stop
                        && failure.error.code()
                            == super::emulator_instance::EMULATOR_CONTROL_WAIT_TIMEOUT =>
                {
                    let Some(rejected) = failure.terminal else {
                        return Ok(RungAttempt::Failed {
                            run_id: None,
                            reason: failure.error.code(),
                        });
                    };
                    stopped = Some(rejected);
                    continue;
                }
                Err(failure) => {
                    return Ok(RungAttempt::Failed {
                        run_id: None,
                        reason: failure.error.code(),
                    });
                }
            };
            if action == EmulatorInstanceAction::Stop {
                if driven.outcome.process_started {
                    return Ok(RungAttempt::Failed {
                        run_id: None,
                        reason: HostCode::RecoveryStopUnconfirmed.as_str(),
                    });
                }
                stopped = Some(driven.terminal);
                self.append_recovery_ladder_fact(
                    pending,
                    EventSeverity::Info,
                    RuntimeLifecyclePhase::RecoveryInstanceStopped {
                        stop: driven.terminal,
                    },
                )?;
                continue;
            }
            if !driven.outcome.running {
                return Ok(RungAttempt::Failed {
                    run_id: None,
                    reason: HostCode::RecoveryEnvironmentNotReady.as_str(),
                });
            }
            let stop = stopped.ok_or_else(|| ladder_invariant(HostCode::RecoveryStopMissing))?;
            let (current, preparation) = match self.await_recovery_readiness(
                pending,
                key,
                &instance_guard,
                admission,
                bounds,
            )? {
                Ok(ready) => ready,
                Err(reason) => {
                    return Ok(RungAttempt::Failed {
                        run_id: None,
                        reason,
                    });
                }
            };
            let event = self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                pending.links.clone(),
                ladder_fact_payload(
                    self.owner_epoch,
                    pending,
                    RuntimeLifecyclePhase::RecoveryEnvironmentReady {
                        stop,
                        start: driven.terminal,
                        preparation,
                    },
                ),
            )?;
            let startup = self
                .prepare_startup_package(&current, pending.links.clone(), pending.request_id)
                .map_err(|failure| *failure.error)?;
            return match startup {
                Some(mut startup) => {
                    startup.recovery_rung = true;
                    self.run_recovery_rung_package(&startup, true, pending, key)
                }
                None => Ok(RungAttempt::EnvironmentReady {
                    event: terminal(&event),
                }),
            };
        }
        Err(ladder_invariant(HostCode::RecoveryStartMissing))
    }

    /// Workflow #369-1: after a confirmed Start, waits until Android reports a resumed activity
    /// (the read-only boot check, review M5) and a fresh preparation passes (`capture.ok &&
    /// touch.ok`, no failure code), within the configured readiness window. The admission
    /// guard Start held covers the first attempt; later attempts take it again, and it is
    /// released while waiting, so the policy thread is never blocked by the wait (Workflow #369:
    /// the ladder's key withholds the instance from policy). The wait runs on this instance's
    /// worker and holds only this instance; other instances' host work runs on their own
    /// workers. A failed preparation is retried only when the existing rule calls it
    /// recoverable or the ADB baseline does not answer (review M3); every retry first polls the
    /// ADB baseline until it answers (bounded by the window), then waits the configured backoff
    /// (its last value repeated), never past the window's end. Each attempt makes the ladder's
    /// holding check (Workflow #369 H-3) and renews the key to the window's end; the
    /// preparation runs on the key and does not release it. Returns the current binding and the
    /// passing preparation event, or the rung's failure reason: `recovery_android_not_booted`
    /// when the window ends before the boot check ever passed (so no preparation ran, review
    /// L-R4-2), otherwise `recovery_environment_not_ready`. The window, the backoff and every
    /// poll's verdict read the host's `RuntimeClock` (test plan v2.2 section 3.10 H-1-rest,
    /// HOST-I2); a poll still sleeps `readiness_poll_ms` and reads the clock when it wakes.
    fn await_recovery_readiness<'guard>(
        &self,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
        instance_guard: &'guard Mutex<()>,
        held: MutexGuard<'guard, ()>,
        bounds: &EmulatorRestartBounds<'_>,
    ) -> RuntimeHostResult<Result<(RegisteredInstance, TerminalEvent), &'static str>> {
        let deadline_ms =
            super::installation::deadline_after(self.monotonic_ms()?, bounds.readiness_window_ms)?;
        let margin_ms = pending.config.hold.margin_ms;
        let alias = pending.instance_alias.as_str();
        let mut held = Some(held);
        let mut retries = 0_usize;
        let mut prepared = false;
        loop {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(Err(reason));
            }
            let window_ms = deadline_ms.saturating_sub(self.monotonic_ms()?);
            if let Some(RungAttempt::Failed { reason, .. }) =
                self.ladder_step_renewal(key, window_ms.saturating_add(margin_ms))?
            {
                return Ok(Err(reason));
            }
            let booted = match self.recovery_android_booted(pending)? {
                Ok(booted) => booted,
                Err(fenced) => return Ok(Err(fenced)),
            };
            if booted {
                prepared = true;
                let admission = match held.take() {
                    Some(admission) => admission,
                    None => lock(instance_guard, HostLocation::LockInstanceAdmission)?,
                };
                let current = self
                    .resolve_instance(alias)
                    .map_err(|failure| *failure.error)?;
                if !current.device_self_checked() {
                    return Ok(Err(HostCode::RecoveryPreparationUnconfirmed.as_str()));
                }
                let (check, preparation, recoverable) = self.prepare_recovery_connection(
                    &current,
                    pending.links.clone(),
                    &admission,
                    key,
                )?;
                if check.capture.ok && check.touch.ok && check.failure_code.is_none() {
                    return Ok(match preparation {
                        Some(preparation) => Ok((current, preparation)),
                        None => Err(HostCode::RecoveryPreparationUnconfirmed.as_str()),
                    });
                }
                drop(admission);
                if !recoverable {
                    match self.recovery_adb_unanswered(pending)? {
                        Ok(true) => {}
                        Ok(false) => {
                            return Ok(Err(HostCode::RecoveryEnvironmentNotReady.as_str()));
                        }
                        Err(fenced) => return Ok(Err(fenced)),
                    }
                }
            }
            drop(held.take());
            let wait_ms = bounds
                .readiness_backoff_ms
                .get(retries)
                .or_else(|| bounds.readiness_backoff_ms.last())
                .copied()
                .unwrap_or(0);
            let not_ready = if prepared {
                HostCode::RecoveryEnvironmentNotReady.as_str()
            } else {
                HostCode::RecoveryAndroidNotBooted.as_str()
            };
            if let Some(reason) = self.await_recovery_retry(
                pending,
                key,
                (wait_ms, Duration::from_millis(bounds.readiness_poll_ms)),
                deadline_ms,
                not_ready,
            )? {
                return Ok(Err(reason));
            }
            retries += 1;
        }
    }

    /// Workflow #369-1 (review M5): Android has booted once it reports a resumed activity
    /// (read-only, through the ADB baseline; no session is opened). An ADB failure is a "not
    /// yet"; one whose child or pipe cleanup is unconfirmed fences the instance (Workflow #387
    /// D1c-G, `fence_recovery_adb`): `Err` with the fence's code, which fails the rung.
    fn recovery_android_booted(
        &self,
        pending: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<Result<bool, &'static str>> {
        match self
            .execution()?
            .observe_foreground_application(&pending.instance_alias)
        {
            Ok(observation) => Ok(Ok(observation.foreground.is_some())),
            Err(error) => {
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    return self
                        .fence_recovery_adb(pending, recovery_adb_fault(pending, &error))
                        .map(Err);
                }
                Ok(Ok(false))
            }
        }
    }

    /// Workflow #369-1 (review M3): whether the ADB baseline fails to answer one probe now. One
    /// whose child or pipe cleanup is unconfirmed fences the instance (Workflow #387 D1c-G):
    /// `Err` with the fence's code.
    fn recovery_adb_unanswered(
        &self,
        pending: &PendingRecoveryLadder,
    ) -> RuntimeHostResult<Result<bool, &'static str>> {
        match self
            .execution()?
            .probe_adb_baseline(&pending.instance_alias)
        {
            Ok(()) => Ok(Ok(false)),
            Err(error) => {
                if error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed) {
                    return self
                        .fence_recovery_adb(pending, recovery_adb_fault(pending, &error))
                        .map(Err);
                }
                Ok(Ok(true))
            }
        }
    }

    /// Before the next readiness attempt: polls the ADB baseline every `poll` until it answers
    /// again, as `await_adb_baseline` does (review M-R4-1), then waits `wait_ms`; `timing` is
    /// `(wait_ms, poll)`, the configured backoff and poll. Both are bounded by `deadline_ms` on
    /// the host clock (HOST-I2), and every poll makes the ladder's holding check (review L1;
    /// Workflow #369 H-3, review2 L7: the scheduling pause too). `Some(reason)` ends the rung:
    /// shutdown, drain, a pause, a lost key, or no time left in the window (`not_ready`). The
    /// ADB probe itself is bounded by the wall-clock instant of what remains of the window.
    fn await_recovery_retry(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
        timing: (u64, Duration),
        deadline_ms: u64,
        not_ready: &'static str,
    ) -> RuntimeHostResult<Option<&'static str>> {
        let (wait_ms, poll) = timing;
        let alias = pending.instance_alias.as_str();
        let stopped = || self.fatal.is_shutdown_requested();
        loop {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(Some(reason));
            }
            let now_ms = self.monotonic_ms()?;
            if now_ms >= deadline_ms {
                return Ok(Some(not_ready));
            }
            let probe_deadline = Instant::now() + Duration::from_millis(deadline_ms - now_ms);
            match self
                .execution()?
                .probe_adb_baseline_until(alias, probe_deadline, &stopped)
            {
                Ok(()) => break,
                Err(error) => {
                    // Workflow #387 D1c-G: an unconfirmed cleanup fences the instance and ends
                    // the rung with the fence's code.
                    if error.resource_quiescence() == Some(ResourceQuiescence::Unconfirmed) {
                        return self
                            .fence_recovery_adb(
                                pending,
                                RuntimeHostError::execution(LADDER_OPERATION, &error),
                            )
                            .map(Some);
                    }
                    let remaining_ms = deadline_ms.saturating_sub(self.monotonic_ms()?);
                    thread::sleep(poll.min(Duration::from_millis(remaining_ms)));
                }
            }
        }
        let until_ms = self.monotonic_ms()?.saturating_add(wait_ms);
        if until_ms >= deadline_ms {
            return Ok(Some(not_ready));
        }
        loop {
            if let Some(reason) = self.ladder_hold_interrupted(pending, key)? {
                return Ok(Some(reason));
            }
            let now_ms = self.monotonic_ms()?;
            if now_ms >= until_ms {
                return Ok(None);
            }
            thread::sleep(poll.min(Duration::from_millis(until_ms - now_ms)));
        }
    }

    /// Runs a rung's package on the ladder's key; it recovers when the run completes with
    /// `success` (its target page reached). Workflow #369 H-3: the ladder queues its
    /// continuation before the run, so the run's release hands the key back. A failure was
    /// already recorded by the runner. An unavailable entry channel or ADB skips the rung, or
    /// fails it under `rules.entry_unavailable = fail` (B5); after a destructive action (the
    /// emulator restart, Workflow #369-1; the application stop, Workflow #369-2) it always
    /// fails it, so `rungs_tried` counts the action that ran.
    fn run_recovery_rung_package(
        &self,
        run: &PendingStartupPackage,
        destructive: bool,
        pending: &PendingRecoveryLadder,
        key: &mut HostKey,
    ) -> RuntimeHostResult<RungAttempt> {
        Ok(match self.run_package_on_key(run, key, Some(pending))? {
            Ok(OperationSuccess {
                result:
                    RuntimeResult::ContainedTaskCompleted {
                        run_id,
                        task_result: outcome,
                        ..
                    },
                ..
            }) => {
                if outcome == TaskOutcome::Success {
                    RungAttempt::Recovered { run_id }
                } else {
                    RungAttempt::Failed {
                        run_id: Some(run_id),
                        reason: HostCode::RecoveryRungTargetNotReached.as_str(),
                    }
                }
            }
            Ok(_) => {
                return Err(ladder_invariant(HostCode::RecoveryRungResultInvalid));
            }
            Err(code) => match recovery_entry_skip(code) {
                Some(reason) if !destructive => {
                    unavailable_entry(&pending.config.rules, reason, code)
                }
                _ => RungAttempt::Failed {
                    run_id: None,
                    reason: code,
                },
            },
        })
    }

    /// Workflow #369 H-3: queues the ladder's holder-owned continuation (high, no deadline,
    /// ordered first and never gated) under the ladder's correlation and causation, just
    /// before a rung run.
    pub(super) fn queue_ladder_continuation(
        &self,
        pending: &PendingRecoveryLadder,
        key: &HostKey,
    ) -> Result<LadderContinuation, RequestFailure> {
        let request_id = self
            .events
            .issuer()
            .mint_request_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        let request = self
            .ladder_claim_request(
                &resolved,
                request_id,
                pending.correlation_id,
                pending.causation_id,
                key.token.holder_id(),
            )
            .map_err(RequestFailure::poison_without_terminal)?;
        match self.request_host_claim(HostClaim {
            request: &request,
            instance_alias: &pending.instance_alias,
            holder_id: key.token.holder_id(),
            connection_id: key.connection_id,
            kind: ClaimKind::LadderContinuation,
            priority: LeasePriority::High,
            lease_ttl_ms: pending.claim_ttl_ms(),
        })? {
            HostClaimAdmission::Queued { grant, .. } => Ok(LadderContinuation {
                request_id: *request_id.transport(),
                grant,
            }),
            HostClaimAdmission::Granted(_) => Err(RequestFailure::poison_without_terminal(
                ladder_invariant(HostCode::RecoveryLadderContinuationGrantedEarly),
            )),
        }
    }

    /// Workflow #369 H-3: after a rung run, the key is the continuation's grant. A
    /// continuation that was not granted is cancelled (unless W-2's shutdown order already
    /// did); the next holding check then finds whether the ladder still holds its key.
    pub(super) fn take_ladder_continuation(
        &self,
        key: &mut HostKey,
        continuation: LadderContinuation,
    ) -> RuntimeHostResult<()> {
        if let Some(token) = continuation.grant.wait(Duration::ZERO)? {
            key.token = token;
            return Ok(());
        }
        let cancelled = self.cancel_host_claim(
            key.token.instance_id(),
            continuation.request_id,
            key.connection_id,
        )?;
        if !cancelled && let Some(token) = continuation.grant.wait(Duration::ZERO)? {
            key.token = token;
        }
        Ok(())
    }

    /// One `recovery_rung_finished` fact: a failed or skipped rung carries why as its outcome
    /// (ledger revision 3; was the `reason` text).
    fn append_recovery_rung_finished(
        &self,
        pending: &PendingRecoveryLadder,
        rung: RecoveryRung,
        rung_result: RecoveryRungOutcome,
        run_id: Option<RunId>,
        outcome: Option<Outcome>,
        environment: Option<TerminalEvent>,
    ) -> RuntimeHostResult<()> {
        self.append_recovery_ladder_fact(
            pending,
            if rung_result == RecoveryRungOutcome::Failed {
                EventSeverity::Warning
            } else {
                EventSeverity::Info
            },
            RuntimeLifecyclePhase::RecoveryRungFinished {
                rung,
                rung_result,
                run_id,
                outcome,
                environment,
            },
        )
    }

    fn append_recovery_ladder_fact(
        &self,
        pending: &PendingRecoveryLadder,
        severity: EventSeverity,
        phase: RuntimeLifecyclePhase,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            severity,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            pending.links.clone(),
            ladder_fact_payload(self.owner_epoch, pending, phase),
        )
        .map(|_| ())
    }
}

/// One of `pending`'s own ladder phases; a drill's is tagged `drill` (Workflow #390 DRILL-1).
fn ladder_fact_payload(
    owner_epoch: actingcommand_contract::OwnerEpoch,
    pending: &PendingRecoveryLadder,
    phase: RuntimeLifecyclePhase,
) -> RuntimePayloadDraft {
    if pending.drill.is_some() {
        RuntimePayloadDraft::recovery_drill_observed(owner_epoch, phase, AuditInput::new())
    } else {
        RuntimePayloadDraft::lifecycle_observed(owner_epoch, phase, AuditInput::new())
    }
}

/// The outcome of a failed rung (ledger revision 3): its reason's outcome; a holding
/// interruption (review L6) is `recovery_ladder_interrupted` (warning) caused by it, so a rung
/// an interruption ended records no error point, as an interrupted climb does not (review L-7).
fn rung_failure_outcome(
    pending: &PendingRecoveryLadder,
    key: &HostKey,
    reason: &'static str,
) -> Outcome {
    let outcome = rung_reason_outcome(pending, key, reason);
    if is_hold_interruption(reason) {
        interrupted(outcome)
    } else {
        outcome
    }
}

/// `recovery_ladder_interrupted` caused by the holding interruption `cause`.
fn interrupted(cause: Outcome) -> Outcome {
    Outcome::new(HostCode::RecoveryLadderInterrupted).caused_by(cause)
}

/// The outcome of a rung's failure reason: the code it relays, with the instance and lease of
/// a lost key.
fn rung_reason_outcome(
    pending: &PendingRecoveryLadder,
    key: &HostKey,
    reason: &'static str,
) -> Outcome {
    let outcome = Outcome::relayed(reason);
    if spelled(reason, &[HostCode::RecoveryLadderKeyLost.code()]) {
        outcome
            .instance_id(identifier_text(&pending.instance_id))
            .lease_id(identifier_text(&key.token.lease_id()))
    } else {
        outcome
    }
}

/// The outcome of a skipped rung (ledger revision 3): the registered code of its skip reason.
fn rung_skip_outcome(reason: RecoveryRungSkipReason) -> Outcome {
    Outcome::new(match reason {
        RecoveryRungSkipReason::NoRecoveryPackage => HostCode::RecoveryRungSkippedNoRecoveryPackage,
        RecoveryRungSkipReason::NoStartupPackage => HostCode::RecoveryRungSkippedNoStartupPackage,
        RecoveryRungSkipReason::NoEmulatorControl => HostCode::RecoveryRungSkippedNoEmulatorControl,
        RecoveryRungSkipReason::CaptureUnavailable => {
            HostCode::RecoveryRungSkippedCaptureUnavailable
        }
        RecoveryRungSkipReason::InputUnavailable => HostCode::RecoveryRungSkippedInputUnavailable,
        RecoveryRungSkipReason::AdbUnavailable => HostCode::RecoveryRungSkippedAdbUnavailable,
        RecoveryRungSkipReason::ResourceFence => HostCode::RecoveryRungSkippedResourceFence,
        RecoveryRungSkipReason::BeforeStartRung => HostCode::RecoveryRungSkippedBeforeStartRung,
        RecoveryRungSkipReason::DeviceNotReady => HostCode::RecoveryRungSkippedDeviceNotReady,
        RecoveryRungSkipReason::DrillPlan => HostCode::RecoveryRungSkippedDrillPlan,
    })
}

/// Workflow #390 DRILL-1: the refusal of a drill whose admission was suppressed, by the reason
/// its `recovery_ladder_suppressed` records: the open cool-down window (until `until_unix_ms`),
/// a ladder that holds or waits for the instance, or an admission the rules that stay in code
/// refused (shutdown, a scheduling pause, business capacity). A drill is never a preparation
/// and meets no blackout before SR-4a, so those reasons are refused as a closed admission.
fn drill_suppressed(reason: RecoveryLadderSuppression, until_unix_ms: u64) -> RuntimeHostError {
    match reason {
        RecoveryLadderSuppression::Cooldown => RuntimeHostError::request(
            HostCode::RecoveryLadderDrillCoolingDown,
            HostLocation::RecoveryLadderDrill,
            RuntimeErrorCode::LeaseCooldown,
        )
        .with_outcome_values(|outcome| outcome.next_eligible_unix_ms(until_unix_ms)),
        RecoveryLadderSuppression::AlreadyRunning => RuntimeHostError::request(
            HostCode::RecoveryLadderDrillAlreadyRunning,
            HostLocation::RecoveryLadderDrill,
            RuntimeErrorCode::LeaseBusy,
        ),
        RecoveryLadderSuppression::AdmissionDenied
        | RecoveryLadderSuppression::PreparationSuperseded
        | RecoveryLadderSuppression::Blackout => RuntimeHostError::request(
            HostCode::RecoveryAdmissionDenied,
            HostLocation::RecoveryLadderDrill,
            RuntimeErrorCode::RuntimeBusy,
        ),
    }
}

/// Review L6 (#666): whether a rung's failure reason ended the rung rather than being its own
/// failure: a reason `ladder_hold_interrupted` gives; a renewal refusal of the key
/// (`lease_expired`, `lease_missing`), which its next holding check reports as the key lost; or
/// a rung run that a scheduling pause's drain cancelled (`contained_task_paused`, which only the
/// pause drain sets). These are the interruptions of §2.2b A3–A6, and A9's
/// (`device_link_lost`, Workflow #398 RD-2).
fn is_hold_interruption(reason: &str) -> bool {
    spelled(
        reason,
        &[
            HostCode::DeviceLinkLost.code(),
            HostCode::RecoveryLadderShutdownRequested.code(),
            HostCode::RecoveryLadderDrainRequested.code(),
            HostCode::RecoveryAdmissionDenied.code(),
            actingcommand_contract::RuntimeErrorCode::ContainedTaskPaused.code(),
            HostCode::RecoveryLadderKeyLost.code(),
            actingcommand_contract::RuntimeErrorCode::LeaseExpired.code(),
            actingcommand_contract::RuntimeErrorCode::LeaseMissing.code(),
        ],
    )
}

/// The skip a rung package's entry refusal stands for: a known unavailable capture or input
/// channel, or an ADB baseline that does not answer. Workflow #398 RD-4 (model-398 §6.4, §6.5):
/// a handset's device-state gate holding the run before its first step (a keyguard waiting for
/// a person, heat, a low battery off power, another application in front) is the rung's entry
/// held, `device_not_ready`, skipped or failed as `rules.entry_unavailable` says.
fn recovery_entry_skip(code: &str) -> Option<RecoveryRungSkipReason> {
    if spelled(
        code,
        &[
            actingcommand_device::codes::DeviceStateCode::DeviceLocked.code(),
            crate::codes::DeviceStateHostCode::ThermalHold.code(),
            crate::codes::DeviceStateHostCode::BatteryLow.code(),
            crate::codes::DeviceStateHostCode::InUse.code(),
        ],
    ) {
        return Some(RecoveryRungSkipReason::DeviceNotReady);
    }
    if spelled(code, &[HostCode::RecoveryCaptureUnavailable.code()]) {
        Some(RecoveryRungSkipReason::CaptureUnavailable)
    } else if spelled(code, &[HostCode::RecoveryInputUnavailable.code()]) {
        Some(RecoveryRungSkipReason::InputUnavailable)
    } else if spelled(
        code,
        &[
            HostCode::StartupPackageAdbNotReady.code(),
            HostCode::RecoveryLadderAdbNotReady.code(),
        ],
    ) {
        Some(RecoveryRungSkipReason::AdbUnavailable)
    } else {
        None
    }
}

/// Workflow #369-1: an ADB child or pipe cleanup left unconfirmed during a readiness check is
/// not a "not yet". Workflow #387 D1c-G: it is the unconfirmed failure the instance's fence is
/// caused by (`fence_recovery_adb`); only where no fence may take it is it the Runtime's fatal.
fn recovery_adb_fault(
    pending: &PendingRecoveryLadder,
    error: &actingcommand_device::DeviceError,
) -> RuntimeHostError {
    let mut fatal = RuntimeHostError::fatal(
        HostCode::RecoveryReadinessAdbUnconfirmed,
        LADDER_OPERATION,
        RuntimeErrorCode::RuntimeHalted,
    )
    .with_native_detail(format!(
        "instance_alias={}; adb_failed={error}",
        pending.instance_alias
    ));
    fatal.lifecycle.instance_id = Some(pending.instance_id);
    fatal.lifecycle.resource_quiescence = Some(ResourceQuiescence::Unconfirmed);
    fatal
}

#[cfg(test)]
mod tests;
