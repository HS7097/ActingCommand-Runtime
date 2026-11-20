// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #489 AVAIL-1 (`contracts/scheduling/README.md`, "An unavailable instance is probed
//! again"; register §1.493): an instance its device self-check made unavailable is probed again,
//! and a recovery its ladder's cool-down held back is taken up when that cool-down ends.
//!
//! The production defect of 11-20 (two instances, v0.12.75): a scheduled run's paired backend
//! open failed, its device self-check failed with it, and the backend self-check gate withdrew the
//! instance's policy availability (`backend_selfcheck:<entry>:<generation>`). The ladder its
//! failure matched was suppressed by an open cool-down window (`recovery_ladder_suppressed`
//! `cooldown`), which drops the admitted ladder and schedules nothing for `until_unix_ms`. From
//! then on the instance had no way out by itself: an unavailable instance is given no task,
//! without a task nothing fails, without a failure no ladder is entered, and the self-check runs
//! only for a dispatch, a preparation or a recovery. Each stayed unavailable for 15 and 16 hours
//! although a self-check passed at once when a person ran one.
//!
//! - **Which instances.** One that the backend self-check gate holds unavailable after a device
//!   self-check that *failed* (its `device.self_check` record with `failure_code`), while its
//!   configured seed is available and no fence holds it (`HostShared::track_unavailability`, called
//!   at every decision of the gate). Fix round 1 (the coordinator's rulings R1 to R3 of 11-20):
//!   also one the gate holds as `backend_selfcheck:unchecked` after any other failing step of a
//!   connection preparation (a close of the kept session or the close after the opens that failed
//!   with its resources confirmed released, an open that failed without a report, the
//!   preparation's lease refused; `withhold_after_failed_preparation`), with that step's code.
//!   Fix round 2 (item 3, the coordinator's ruling of 11-21): also one the gate holds after a
//!   recorded open whose report left an entry `unknown`, with no cause (its `device.self_check`
//!   record, status `unknown`, says why). It leaves the set when the gate no longer holds it
//!   (its self-check passed and its availability came back, a fence, a device link or a device
//!   state took the instance, or the configuration marks it unavailable); fix round 2 (item 1):
//!   while a probe of it is queued or runs, that probe's settlement decides, so an open that
//!   passes and a close that fails within one probe are one failed probe of the same
//!   unavailability, not a new one. An instance the gate holds for a self-check never made (a start
//!   before its preparation, facts dropped at an emulator stop or control) or for the
//!   preparation's emulator status read (#411 A48: no emulator process, a state that could not be
//!   read on a pending binding, a device not recognized) enters only once a later step of a
//!   preparation of it fails: the emulator watch, the idle wake and a person's resume own those.
//! - **The probe** is the device self-check a person's `actingctl selfcheck` runs (a preparation
//!   on the instance's key: it closes a retained session, reads the emulator status first, opens
//!   the backends and checks capture and input; no game input, no application launch, no emulator
//!   start), as the Runtime's own recovery preparation, as #411 A38's fence recheck runs it: it
//!   starts no ladder, and direction ① counts none of its records. It runs on a self-check claim
//!   of the instance's queue (no new claim kind), queued by the lease sweep only while nothing
//!   holds or waits for the instance's key and no takeover cool-down runs, so it never queues in
//!   front of a client's, the Lab's or the scheduler's own work; it takes the key for the
//!   preparation's few seconds and releases it. Like every preparation it closes a session a
//!   keyless Lab observation kept on the instance first; the instance's admission lock, which the
//!   preparation holds and an observation takes, serialises the two. A probe whose worker panics
//!   ends at the worker's root (`end_panicked_unavailable_probe`; fix round 2, item 2, the
//!   coordinator's ruling of 11-21): the panic is the worker's `panic_caught` record under the
//!   probe's links, nothing probes the instance again in this unavailability, and a person is
//!   told at once. A panic inside the preparation poisons the device's admission lock, whose next
//!   lock stops the Runtime (`runtime_state_poisoned`); this slice does not change that (board
//!   issue #496).
//! - **When.** `scheduling.unavailable_probe.first_interval_ms` after the instance became
//!   unavailable, then after each failed probe the interval times `factor`, at most
//!   `max_interval_ms`. An interval counts quiet time: while a claim holds or waits for the
//!   instance's key, its interval starts again at the sweep's tick. Read at every tick, so an
//!   instruction applies from the next one.
//! - **A cool-down's end** (item 1): a ladder that its cool-down suppressed while the instance was
//!   in the set is held with the instance, with the `until_unix_ms` its suppression named. From
//!   that time (no later than the sweep's next tick) a probe is due whatever the interval: a probe
//!   that passes drops the held recovery with the unavailability; one that fails reads the
//!   instance's cool-down window in force again (fix round 1: a later climb, or a `triggered`
//!   admission, may have opened one since) and, once no window holds it, admits the recovery as
//!   it would have been admitted without the cool-down (every other admission rule as before) and
//!   queues its claim; while one does, the recovery stays held until that window's end. The
//!   cool-down still holds a recovery back while it runs. Fix round 2 (item 4): one is held per
//!   instance, a later one replaces the earlier, only a failed probe's settlement releases it,
//!   and every drop (replaced, the instance leaving the set, a pause, a fence or an idle stop)
//!   writes the trigger's `recovery_ladder_suppressed` `cooldown` once more.
//! - **What holds probing.** A scheduling pause of the instance or the global one (any stage, any
//!   mode), a fence (its own recheck owns it), an emulator the idle action holds stopped, a
//!   blackout that covers the instance, and a stop. Each probe is checked again on the key. A
//!   pause, a fence or an idle stop also drops a held recovery: those have their own rules.
//! - **Fail loud.** Every probe is recorded (`instance_unavailable_probe_failed` with the next
//!   probe's time, `instance_unavailable_probe_passed`). An instance in the set for
//!   `attention_after_ms` is brought to a person's attention once per unavailability:
//!   `instance_unavailable_awaiting_person` (a warning `runtime.failed` record under the instance,
//!   handed to the daemon's log too), or at once after a probe's panic (`failure_code`
//!   `panic_caught`, no next probe); it ends when the instance is available again.
//! - **The status** answers nothing of the set in 0.12.76 (fix round 1, the coordinator's ruling):
//!   the contract declares `RuntimeInstanceStatus.unavailable` (`runtime-client` 14) and the host
//!   fills it for no instance, since a reader of 0.12.75 refuses a status that carries it; the
//!   slice that answers it raises the floor to 14 in the same release. A reader of today tells
//!   the instance by the overview row's `backend_selfcheck` and `not_running`, and by the records
//!   above.
//!
//! Memory only: a restart starts the set again from the gate's next decision.

use super::host_claims::{HostClaimWork, HostKey};
use super::lease::HostClaim;
use super::recovery_ladder::PendingRecoveryLadder;
use super::resource_fence::with_failure_code;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::identifier_text;
use crate::policy_host::InstanceOccupancy;
use crate::time::RuntimeClockSample;
use actingcommand_contract::LeasePriority;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_runtime_config::UnavailableProbeConfig;
use std::collections::btree_map::Entry;

/// Where an unavailable instance's probe stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeStep {
    /// None is queued or running.
    Idle,
    /// Its self-check claim waits for the instance's key, or its worker has not begun.
    Queued,
    /// Its preparation runs on the instance's key.
    Running,
    /// Fix round 2 (item 2, the coordinator's ruling of 11-21): a probe of this unavailability
    /// panicked. Nothing probes the instance again until it leaves the set (a decision of the
    /// gate: a passing self-check of a person's resume or self-check, a fence, the configuration)
    /// or the Runtime restarts.
    Panicked,
}

impl ProbeStep {
    /// Fix round 2 (item 1): whether a probe of the instance is queued or runs, so its
    /// settlement, not a decision of the gate, decides whether the instance leaves the set.
    const fn in_flight(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// A recovery the ladder's cool-down held back while the instance was unavailable, and the end of
/// that cool-down, as its `recovery_ladder_suppressed` named it.
struct HeldRecovery {
    ladder: PendingRecoveryLadder,
    until_unix_ms: u64,
}

/// One instance its device self-check made unavailable (memory only).
pub(super) struct UnavailableInstance {
    /// When the withdrawal in effect was observed.
    since_unix_ms: u64,
    /// The code of the failed self-check, or of the failed step of a connection preparation,
    /// that made it unavailable (the latest one); none for an instance an open whose report left
    /// an entry `unknown` took in (fix round 2, item 3) until such a code comes.
    cause: Option<CodeStr>,
    /// When the interval to the next probe began (the host clock's monotonic reading and Unix
    /// time): the instance's entry, the end of a probe, or the latest tick at which a claim held
    /// or waited for its key.
    from: RuntimeClockSample,
    /// The probes started in this unavailability.
    probes: u32,
    step: ProbeStep,
    /// Whether a person was told (`instance_unavailable_awaiting_person`).
    awaiting_person: bool,
    held: Option<HeldRecovery>,
}

/// Test builds: one instance of the set as the probes hold it (the status answers none of it in
/// this release).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnavailableForTest {
    pub(crate) since_unix_ms: u64,
    pub(crate) cause: Option<CodeStr>,
    pub(crate) probes: u32,
    /// When the next probe is due; `None` while one is queued or runs, and after a probe of this
    /// unavailability panicked (fix round 2, item 2).
    pub(crate) next_probe_unix_ms: Option<u64>,
    pub(crate) awaiting_person: bool,
}

impl UnavailableInstance {
    fn new(since_unix_ms: u64, now: RuntimeClockSample) -> Self {
        Self {
            since_unix_ms,
            cause: None,
            from: now,
            probes: 0,
            step: ProbeStep::Idle,
            awaiting_person: false,
            held: None,
        }
    }

    /// When the next probe is due by its interval (monotonic, Unix); none while one is queued or
    /// runs.
    fn next_interval_end(&self, config: UnavailableProbeConfig) -> Option<RuntimeClockSample> {
        (self.step == ProbeStep::Idle).then(|| {
            let interval_ms = config.interval_ms(self.probes.saturating_add(1));
            RuntimeClockSample {
                monotonic_ms: self.from.monotonic_ms.saturating_add(interval_ms),
                unix_ms: self.from.unix_ms.saturating_add(interval_ms),
            }
        })
    }

    /// When the next probe is due, in Unix time: its interval's end, or the end of the cool-down
    /// that holds a recovery back when that comes first.
    fn next_probe_unix_ms(&self, config: UnavailableProbeConfig) -> Option<u64> {
        let interval_end = self.next_interval_end(config)?.unix_ms;
        Some(match &self.held {
            Some(held) => interval_end.min(held.until_unix_ms),
            None => interval_end,
        })
    }

    /// Whether a probe is due at `now`.
    fn due(&self, config: UnavailableProbeConfig, now: RuntimeClockSample) -> bool {
        self.next_interval_end(config).is_some_and(|end| {
            now.monotonic_ms >= end.monotonic_ms
                || self
                    .held
                    .as_ref()
                    .is_some_and(|held| now.unix_ms >= held.until_unix_ms)
        })
    }
}

/// One probe of an unavailable instance, on a self-check claim of its instance's queue (no new
/// claim kind).
pub(super) struct PendingUnavailableProbe {
    pub(super) instance_id: InstanceId,
    /// The probe's number in this unavailability.
    pub(super) probe: u32,
    /// The instance and the probe's causation: what a worker failure is recorded under.
    pub(super) links: EventLinksDraft,
}

/// What holds an instance's probes at a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeHold {
    /// Nothing: the instance may be probed when its key is free.
    Free,
    /// A blackout covers the instance: it is probed after the blackout.
    Blackout,
    /// A scheduling pause, a fence or an idle stop owns the instance: nothing probes it, and a
    /// recovery the cool-down held back is dropped.
    Owned,
}

/// What a probe's start knew of the unavailability it probed.
struct ProbeStart {
    since_unix_ms: u64,
    cause: Option<CodeStr>,
}

impl HostShared {
    /// The gate's decision for `instance_id` was made (`runtime_facts.rs`): the instance enters
    /// the set when the gate holds it unavailable after a self-check, or (fix round 1) another
    /// step of a connection preparation, that failed with `failed`, or (fix round 2, item 3) after
    /// a recorded open whose report left an entry `unknown` (`unknown`: it enters with no cause),
    /// stays while the gate holds it so (its cause the latest such code), and leaves it otherwise:
    /// fix round 2 (item 1), not while a probe of it is queued or runs, whose settlement decides.
    /// A recovery held with an instance that leaves is dropped with its record (item 4). Called
    /// under the gate's own lock; takes the fact store's and the set's leaf locks.
    pub(super) fn track_unavailability(
        &self,
        instance_id: InstanceId,
        failed: Option<CodeStr>,
        unknown: bool,
    ) -> RuntimeHostResult<()> {
        let withdrawn = self.selfcheck_withdrawal(instance_id)?;
        let now = self.runtime_clock_sample()?;
        let mut instances = lock(
            &self.unavailable_instances,
            HostLocation::QueueUnavailableProbes,
        )?;
        match (withdrawn, failed) {
            (None, _) => {
                let leaves = instances
                    .get(&instance_id)
                    .is_some_and(|instance| !instance.step.in_flight());
                let dropped = if leaves {
                    instances
                        .remove(&instance_id)
                        .and_then(|instance| instance.held)
                } else {
                    None
                };
                drop(instances);
                return self.drop_held_recovery(dropped);
            }
            (Some(since_unix_ms), failed) => {
                let instance = match instances.entry(instance_id) {
                    Entry::Occupied(occupied) => occupied.into_mut(),
                    // A self-check never made enters nothing (`backend_selfcheck:unchecked`).
                    Entry::Vacant(_) if failed.is_none() && !unknown => return Ok(()),
                    Entry::Vacant(vacant) => {
                        vacant.insert(UnavailableInstance::new(since_unix_ms, now))
                    }
                };
                if failed.is_some() {
                    instance.cause = failed;
                }
                // Something other than a probe of its own touched the device just now (a run, a
                // rung, a preparation, an emulator control): the interval counts from here.
                if instance.step == ProbeStep::Idle {
                    instance.from = now;
                }
            }
        }
        Ok(())
    }

    /// Workflow #489 AVAIL-1 (item 1): a ladder of `instance_id` its cool-down suppressed until
    /// `until_unix_ms`. While the instance is unavailable by its self-check, the recovery is held
    /// with it and taken up when the cool-down ends; otherwise nothing changes. Fix round 2 (item
    /// 4): one is held per instance; a later one replaces the earlier, whose drop is recorded.
    pub(super) fn hold_cooled_down_recovery(
        &self,
        instance_id: InstanceId,
        ladder: PendingRecoveryLadder,
        until_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let replaced = lock(
            &self.unavailable_instances,
            HostLocation::QueueUnavailableProbes,
        )?
        .get_mut(&instance_id)
        .and_then(|instance| {
            instance.held.replace(HeldRecovery {
                ladder,
                until_unix_ms,
            })
        });
        self.drop_held_recovery(replaced)
    }

    /// Fix round 2 (item 4): `held`, a recovery the cool-down held back with an unavailable
    /// instance, is dropped without being taken up: its trigger's `recovery_ladder_suppressed`
    /// `cooldown` once more (`record_dropped_held_recovery`). Called with no set's lock held.
    fn drop_held_recovery(&self, held: Option<HeldRecovery>) -> RuntimeHostResult<()> {
        match held {
            Some(held) => self.record_dropped_held_recovery(&held.ladder, held.until_unix_ms),
            None => Ok(()),
        }
    }

    /// Test builds: `instance_id` as the set holds it, with the configuration in effect.
    #[cfg(test)]
    pub(super) fn unavailable_for_test(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<UnavailableForTest>> {
        let config = self.unavailable_probe_config()?;
        Ok(lock(
            &self.unavailable_instances,
            HostLocation::QueueUnavailableProbes,
        )?
        .get(&instance_id)
        .map(|instance| UnavailableForTest {
            since_unix_ms: instance.since_unix_ms,
            cause: instance.cause.clone(),
            probes: instance.probes,
            next_probe_unix_ms: instance.next_probe_unix_ms(config),
            awaiting_person: instance.awaiting_person,
        }))
    }

    /// Probe `probe` of `instance_id` ended by a panic of its worker, queued or running. Called at
    /// the worker's root, where the panic was caught (the join's error arm,
    /// `instance_worker_loop`); the caller records the panic and its fatal mark whatever this
    /// returns. Fix round 2 (item 2, the coordinator's ruling of 11-21, which takes back fix round
    /// 1's "the schedule goes on"): the instance's climbing mark comes off whatever state its
    /// entry is in; the entry, while it is still this probe's, stays `Panicked`, so nothing
    /// probes the instance again in this unavailability (whether or not the panic poisoned a lock
    /// the next probe would take: a panic inside the preparation poisons the device's admission
    /// lock, whose next lock stops the Runtime: board issue #496); and a person is told at once,
    /// `instance_unavailable_awaiting_person` with `failure_code` `panic_caught` (the code the
    /// worker's own record carries) and no next probe, unless this unavailability told one
    /// already. The panicked payload's key lapses at its TTL, as every panicked payload's does.
    pub(super) fn end_panicked_unavailable_probe(
        &self,
        instance_id: InstanceId,
        probe: u32,
    ) -> RuntimeHostResult<()> {
        lock(&self.stability_counter, HostLocation::RunUnavailableProbe)?
            .set_climbing(instance_id, false);
        let now = self.runtime_clock_sample()?;
        let attention = {
            let mut instances = lock(
                &self.unavailable_instances,
                HostLocation::RunUnavailableProbe,
            )?;
            match instances.get_mut(&instance_id) {
                Some(instance) if instance.probes == probe && instance.step.in_flight() => {
                    instance.step = ProbeStep::Panicked;
                    let told = std::mem::replace(&mut instance.awaiting_person, true);
                    (!told).then(|| {
                        (
                            now.unix_ms.saturating_sub(instance.since_unix_ms),
                            instance.probes,
                        )
                    })
                }
                _ => None,
            }
        };
        let Some((elapsed_ms, probes)) = attention else {
            return Ok(());
        };
        self.record_unavailable_attention(
            instance_id,
            elapsed_ms,
            self.unavailable_probe_config()?.attention_after_ms,
            probes,
            Some(&CodeStr::from(ContractCode::PanicCaught.code())),
            None,
        )
    }

    /// What holds the probes of `instance_id` (alias `alias`) now.
    fn probe_hold(&self, instance_id: InstanceId, alias: &str) -> RuntimeHostResult<ProbeHold> {
        let paused =
            lock(&self.scheduling_pause, HostLocation::QueueUnavailableProbes)?.pauses(alias);
        if paused
            || self.resource_fenced(instance_id)?
            || self.idle_holds_emulator_stopped(alias)?
        {
            return Ok(ProbeHold::Owned);
        }
        if self.blackout_covering(instance_id, alias)?.is_some() {
            return Ok(ProbeHold::Blackout);
        }
        Ok(ProbeHold::Free)
    }

    /// The lease sweep's step (after the fences' rechecks): for each unavailable instance, the
    /// attention once it lasted `attention_after_ms`, and a probe when one is due and the
    /// instance's key is free. Only a failure the Runtime does not survive is returned; a claim
    /// the queue refuses is recorded and ends that probe as failed.
    pub(super) fn probe_unavailable_instances(&self) -> RuntimeHostResult<()> {
        if self.fatal.is_shutdown_requested() {
            return Ok(());
        }
        let tracked = lock(
            &self.unavailable_instances,
            HostLocation::QueueUnavailableProbes,
        )?
        .keys()
        .copied()
        .collect::<Vec<_>>();
        if tracked.is_empty() {
            return Ok(());
        }
        let config = self.unavailable_probe_config()?;
        let now = self.runtime_clock_sample()?;
        for instance_id in tracked {
            let alias = lock(
                &self.registered_instances,
                HostLocation::QueueUnavailableProbes,
            )?
            .get(&instance_id)
            .map(|instance| instance.instance_alias.clone());
            let Some(alias) = alias else {
                continue;
            };
            let hold = self.probe_hold(instance_id, &alias)?;
            let occupancy = if hold == ProbeHold::Free {
                self.instance_occupancy(instance_id, now.unix_ms)?
            } else {
                None
            };
            let (attention, due, dropped) = {
                let mut instances = lock(
                    &self.unavailable_instances,
                    HostLocation::QueueUnavailableProbes,
                )?;
                let Some(instance) = instances.get_mut(&instance_id) else {
                    continue;
                };
                // A pause, a fence or an idle stop drops a held recovery (fix round 2, item 4:
                // with its record).
                let dropped = if hold == ProbeHold::Owned {
                    instance.held.take()
                } else {
                    None
                };
                let elapsed_ms = now.unix_ms.saturating_sub(instance.since_unix_ms);
                let attention = if hold != ProbeHold::Owned
                    && !instance.awaiting_person
                    && elapsed_ms >= config.attention_after_ms
                {
                    instance.awaiting_person = true;
                    Some((
                        elapsed_ms,
                        instance.probes,
                        instance.cause.clone(),
                        instance.next_probe_unix_ms(config),
                    ))
                } else {
                    None
                };
                let due = match (hold, occupancy) {
                    (ProbeHold::Free, None) if instance.due(config, now) => {
                        instance.probes = instance.probes.saturating_add(1);
                        instance.step = ProbeStep::Queued;
                        Some(instance.probes)
                    }
                    // Quiet time: a claim holds or waits for the key, so the interval starts
                    // again.
                    (ProbeHold::Free, Some(InstanceOccupancy::Held { .. }))
                        if instance.step == ProbeStep::Idle =>
                    {
                        instance.from = now;
                        None
                    }
                    _ => None,
                };
                (attention, due, dropped)
            };
            self.drop_held_recovery(dropped)?;
            if let Some((elapsed_ms, probes, cause, next)) = attention {
                self.record_unavailable_attention(
                    instance_id,
                    elapsed_ms,
                    config.attention_after_ms,
                    probes,
                    cause.as_ref(),
                    next,
                )?;
            }
            if let Some(probe) = due {
                self.queue_unavailable_probe(instance_id, probe)?;
            }
        }
        Ok(())
    }

    /// One `instance_unavailable_awaiting_person` under the instance's links, handed to the
    /// daemon's log too.
    fn record_unavailable_attention(
        &self,
        instance_id: InstanceId,
        elapsed_ms: u64,
        limit_ms: u64,
        probes: u32,
        cause: Option<&CodeStr>,
        next: Option<u64>,
    ) -> RuntimeHostResult<()> {
        let outcome = Outcome::new(HostCode::InstanceUnavailableAwaitingPerson)
            .operation(HostLocation::QueueUnavailableProbes)
            .instance_id(identifier_text(&instance_id))
            .elapsed_ms(elapsed_ms)
            .limit_ms(limit_ms)
            .attempts(probes);
        let outcome = match cause.and_then(CodeStr::registered) {
            Some(cause) => outcome.failure_code(cause),
            None => outcome,
        };
        let outcome = match next {
            Some(next) => outcome.next_occurrence_unix_ms(next),
            None => outcome,
        };
        let links = self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id));
        self.record_unavailable(links, outcome.clone(), HostLocation::QueueUnavailableProbes)?;
        self.fatal.report_warning(&outcome);
        Ok(())
    }

    /// One record of an unavailable instance (`runtime.failed`, its severity the outcome's
    /// category).
    fn record_unavailable(
        &self,
        links: EventLinksDraft,
        outcome: Outcome,
        operation: HostLocation,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                outcome,
                DiagnosticDetailDraft::new(
                    "unavailable_probe",
                    operation,
                    "runtime_host",
                    operation.as_str(),
                ),
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// Queues one probe on a self-check claim of the instance's queue (as a person's
    /// `actingctl selfcheck` waits for the key; no new claim kind).
    fn queue_unavailable_probe(
        &self,
        instance_id: InstanceId,
        probe: u32,
    ) -> RuntimeHostResult<()> {
        let resolved = lock(
            &self.registered_instances,
            HostLocation::QueueUnavailableProbes,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::QueueUnavailableProbes,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let issuer = self.events.issuer();
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| runtime_identifier_error())?
            .transport();
        let causation_id = issuer
            .mint_causation_id()
            .map_err(|_| runtime_identifier_error())?;
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let request = RuntimeRequest::new(
            issuer
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?,
            issuer
                .mint_correlation_id()
                .map_err(|_| runtime_identifier_error())?,
            Some(causation_id),
            actor,
            source,
            unix_ms_now()?,
            RuntimeOperation::AcquireLease {
                instance_alias: resolved.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::QueueUnavailableProbes,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler(HostLocation::QueueUnavailableProbes, &error)
            })?;
        let lease_ttl_ms = lock(&self.scheduler, HostLocation::QueueUnavailableProbes)?
            .config()
            .lease_ttl_ms;
        let links = self
            .events
            .system_links()?
            .with_instance_id(issuer.issue_registered_instance(instance_id))
            .with_causation_id(causation_id);
        let queued = self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: &resolved.instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::SelfCheck,
                priority: LeasePriority::Normal,
                lease_ttl_ms,
            },
            HostClaimWork::UnavailableProbe(PendingUnavailableProbe {
                instance_id,
                probe,
                links: links.clone(),
            }),
        );
        match queued {
            Ok(()) => Ok(()),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => {
                self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    links.clone(),
                    None,
                )?;
                let refused = failure.error.code();
                self.settle_unavailable_probe(
                    instance_id,
                    probe,
                    ProbeStep::Queued,
                    None,
                    Some(refused),
                    links,
                )
            }
        }
    }

    /// A granted probe on the instance's worker: the preparation and self-check a person's
    /// `actingctl selfcheck` makes, on the claim's key, as the Runtime's own recovery preparation
    /// (no ladder starts from it, and direction ① counts none of its records), then the key's
    /// release. A passing self-check made the instance available on its way (the gate); a failing
    /// one is recorded, the next probe counts its interval from now, and a recovery the
    /// cool-down held back whose cool-down has ended is admitted. A probe superseded while its
    /// claim waited, or an instance a pause, a fence, an idle stop or a stop took meanwhile, runs
    /// nothing. Only a fatal failure is returned.
    pub(super) fn run_unavailable_probe(
        &self,
        pending: &PendingUnavailableProbe,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let instance_id = pending.instance_id;
        let resolved = lock(
            &self.registered_instances,
            HostLocation::RunUnavailableProbe,
        )?
        .get(&instance_id)
        .cloned()
        .ok_or_else(|| {
            RuntimeHostError::fatal(
                HostCode::RuntimeInstanceRegistryIncomplete,
                HostLocation::RunUnavailableProbe,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let held_off = self.fatal.is_shutdown_requested()
            || self.probe_hold(instance_id, &resolved.instance_alias)? != ProbeHold::Free;
        let (start, dropped) = {
            // Fix round 2 (item 1): a decision of the gate made while the probe was queued left
            // the entry to it; a probe that does not run settles that decision here, under the
            // gate's own lock, as the gate would have.
            let _availability = lock(
                &self.backend_selfcheck_availability_gate,
                HostLocation::RunUnavailableProbe,
            )?;
            let withdrawn = self.selfcheck_withdrawal(instance_id)?.is_some();
            let mut instances = lock(
                &self.unavailable_instances,
                HostLocation::RunUnavailableProbe,
            )?;
            let queued = instances.get(&instance_id).is_some_and(|instance| {
                instance.step == ProbeStep::Queued && instance.probes == pending.probe
            });
            if !queued {
                (None, None)
            } else if held_off && !withdrawn {
                (
                    None,
                    instances
                        .remove(&instance_id)
                        .and_then(|instance| instance.held),
                )
            } else {
                match instances.get_mut(&instance_id) {
                    Some(instance) if held_off => {
                        // The probe did not run: it is not counted, and the next one is due when
                        // the hold ends.
                        instance.step = ProbeStep::Idle;
                        instance.probes = instance.probes.saturating_sub(1);
                        (None, None)
                    }
                    Some(instance) => {
                        instance.step = ProbeStep::Running;
                        (
                            Some(ProbeStart {
                                since_unix_ms: instance.since_unix_ms,
                                cause: instance.cause.clone(),
                            }),
                            None,
                        )
                    }
                    None => (None, None),
                }
            }
        };
        self.drop_held_recovery(dropped)?;
        let Some(start) = start else {
            return self.release_host_key(&key);
        };
        let validated = key.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RunUnavailableProbe,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(instance_id), None, None);
        lock(&self.stability_counter, HostLocation::RunUnavailableProbe)?
            .set_climbing(instance_id, true);
        let prepared = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)
            .and_then(|instance_guard| {
                let admission = lock(&instance_guard, HostLocation::RunUnavailableProbe)?;
                self.prepare_recovery_connection(&resolved, links.clone(), &admission, &key)
            });
        let unmarked = lock(&self.stability_counter, HostLocation::RunUnavailableProbe)
            .map(|mut counter| counter.set_climbing(instance_id, false));
        let released = self.release_host_key(&key);
        let (selfcheck, _, _) = prepared?;
        unmarked?;
        released?;
        let passed = selfcheck.capture.ok && selfcheck.touch.ok && selfcheck.failure_code.is_none();
        self.settle_unavailable_probe(
            instance_id,
            pending.probe,
            ProbeStep::Running,
            Some((start, passed)),
            selfcheck.failure_code.as_deref(),
            links,
        )
    }

    /// Probe `probe` of `instance_id` ended in `step` (`ran` is its start and whether its
    /// self-check passed; `None` for a claim the queue refused, `cause_spelling` the refusal's
    /// code or the self-check's failure). Fix round 2 (item 1): the decisions of the gate made
    /// while the probe was queued or ran left the instance's entry to this settlement, which reads
    /// the gate's hold now, under the gate's own lock. An instance the gate no longer holds is
    /// available again (or another rule took it): it leaves the set, and a passing probe records
    /// `instance_unavailable_probe_passed`. An instance the gate holds again (a later step of the
    /// probe failed after an open that passed, such as its close) or still is one failed probe of
    /// the same unavailability, recorded `instance_unavailable_probe_failed` with that step's
    /// code and the next probe's time; its count, `since`, attention and held recovery stay, its
    /// next interval counts from now, and a recovery held back whose cool-down has ended is
    /// admitted after the record. Fix round 1 (item 3): the cool-down that ended is the
    /// instance's window in force now, read again here, not only the one the suppression named:
    /// while a later window holds the instance (another ladder's climb, a `triggered` admission),
    /// the recovery stays held until that window's end, the next probe's time.
    fn settle_unavailable_probe(
        &self,
        instance_id: InstanceId,
        probe: u32,
        step: ProbeStep,
        ran: Option<(ProbeStart, bool)>,
        cause_spelling: Option<&str>,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        let config = self.unavailable_probe_config()?;
        let now = self.runtime_clock_sample()?;
        let window_until_unix_ms = self.cooldown_window_until(instance_id)?;
        let (still, dropped) = {
            let _availability = lock(
                &self.backend_selfcheck_availability_gate,
                HostLocation::RunUnavailableProbe,
            )?;
            let withdrawn = self.selfcheck_withdrawal(instance_id)?.is_some();
            let mut instances = lock(
                &self.unavailable_instances,
                HostLocation::RunUnavailableProbe,
            )?;
            let settles = instances
                .get(&instance_id)
                .map(|instance| instance.probes == probe && instance.step == step);
            match settles {
                // The set holds another unavailability of the instance by now.
                Some(false) => return Ok(()),
                Some(true) if !withdrawn => (
                    None,
                    instances
                        .remove(&instance_id)
                        .and_then(|instance| instance.held),
                ),
                _ => match instances.get_mut(&instance_id) {
                    Some(instance) => {
                        instance.step = ProbeStep::Idle;
                        instance.from = now;
                        let ended = instance
                            .held
                            .as_ref()
                            .is_some_and(|held| held.until_unix_ms <= now.unix_ms);
                        let window_holds = now.unix_ms < window_until_unix_ms;
                        if ended
                            && window_holds
                            && let Some(held) = instance.held.as_mut()
                        {
                            held.until_unix_ms = window_until_unix_ms;
                        }
                        let held = if ended && !window_holds {
                            instance.held.take()
                        } else {
                            None
                        };
                        (Some((instance.next_probe_unix_ms(config), held)), None)
                    }
                    None => (None, None),
                },
            }
        };
        let operation = HostLocation::RunUnavailableProbe;
        match (still, ran) {
            (None, Some((start, true))) => {
                let outcome = Outcome::new(HostCode::InstanceUnavailableProbePassed)
                    .operation(operation)
                    .instance_id(identifier_text(&instance_id))
                    .attempts(probe)
                    .elapsed_ms(now.unix_ms.saturating_sub(start.since_unix_ms));
                let outcome = match start.cause.as_ref().and_then(CodeStr::registered) {
                    Some(cause) => outcome.failure_code(cause),
                    None => outcome,
                };
                self.record_unavailable(links, outcome, operation)?;
            }
            (still, _) => {
                let (next, held) = still.unwrap_or((None, None));
                let outcome = Outcome::new(HostCode::InstanceUnavailableProbeFailed)
                    .operation(operation)
                    .instance_id(identifier_text(&instance_id))
                    .attempts(probe);
                let outcome = with_failure_code(outcome, cause_spelling);
                let outcome = match next {
                    Some(next) => outcome.next_occurrence_unix_ms(next),
                    None => outcome,
                };
                self.record_unavailable(links, outcome, operation)?;
                if let Some(held) = held {
                    self.admit_held_recovery(held.ladder)?;
                }
            }
        }
        // Fix round 2 (item 4): a recovery held with an instance that left the set is dropped
        // after the probe's record, with its own.
        self.drop_held_recovery(dropped)
    }
}
