// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-2 (model-398 v1.1 §4; board #387 ruling A9): the wired device link of the
//! handsets, and its wait and restore.
//!
//! - **Watch.** One `adb track-devices` process per Runtime (the provider's
//!   `open_device_link_watch`), started after the daemon-start preparation and only when a bound
//!   handset is registered; nothing polls. Its reader thread hands every event to
//!   [`HostShared::device_link_event`]; the lease sweep's tick runs the timers
//!   ([`tick`]): the settle wait, the restore retries and the watch's restarts, all measured on
//!   the host clock.
//! - **States.** [`LinkMachine`] keeps each bound handset's link state as adb reports it
//!   (`device`, `absent`, `offline`, `unauthorized`, `no_permissions`, `other`) and the Runtime's
//!   own `unwatched`; an unbound handset stays `unbound` (RD-1b). There is no "disconnected"
//!   status of its own: leaving `device` withdraws the policy availability through the one
//!   self-check gate, under the source `device_link:<state>`, records `device_link_waiting`
//!   once per state, writes `device.link` and `device.connected = false`, closes the retained
//!   session and drops the self-check facts, so the instance is available again only after a
//!   preparation passes (§4.2).
//! - **Restore.** Back at `device`, after `device_link.settle_ms` the handset's preparation runs
//!   on a self-check claim of its queue (`HostClaimWork::LinkRestore`, today's connection
//!   preparation; no new claim kind). Passed: `device_link_restored` and
//!   `device.connected = true`, the gate restores the availability. Failed: today's preparation
//!   failure (its backend codes start the ladder by `preparation_backend`), then the next try
//!   after each wait of `device_link.restore_retry_ms`; used up, the Runtime waits for the next
//!   link change or a person's self-check (§4.3).
//! - **Offline.** `device_link.offline_action = kick_once` sends one `adb -s <serial> reconnect`
//!   per `offline` episode; `wait` sends nothing. `unauthorized` only waits. Nothing here runs
//!   `kill-server`, `reboot`, `usb` or `tcpip`.
//! - **Watch failures.** A watch that ends is started again after each wait of
//!   `device_link.watch.restart_backoff_ms` (`device_watch_restarted`); one that cannot start,
//!   reads an output of unknown shape, or ends more than `watch.restart_limit` times within
//!   `watch.restart_window_ms` is failed (`device_watch_failed`): every handset is `unwatched`
//!   and unavailable, emulators go on, and a person's self-check of a handset starts it again.
//! - **In flight.** A run whose device failed while its handset is not `device` ends with
//!   `device_link_lost` (the device failure its cause, [`HostShared::device_link_lost`]); a
//!   climbing ladder is interrupted (A9, `device_link_hold`).
//!
//! No record names a device serial: a handset is its label `handset:<alias>`.

use super::emulator_instance::DEVICE_CONNECTED_FACT_KEY;
use super::host_claims::{HostClaimWork, HostKey};
use super::lease::HostClaim;
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::RawSource;
use actingcommand_contract::{
    DeviceLinkState, DeviceWatchProblem, FactValue as RuntimeFactValue, LeasePriority,
};
use actingcommand_device::handset_label;
use actingcommand_execution_kernel::{DeviceLinkWatch, DeviceLinkWatchEvent, DeviceLinkWatchStop};
use actingcommand_runtime_config::{DeviceLinkConfig, OfflineAction};

/// The runtime fact of a handset's link state (instance scope, string, model-398 §4.7).
pub(super) const DEVICE_LINK_FACT_KEY: &str = "device.link";

/// What the Runtime knows about one bound handset's link.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HandsetLink {
    alias: String,
    /// The last state the watch reported (or `unwatched`); `None` before its first list.
    state: Option<DeviceLinkState>,
    /// The state `device_link_waiting` was last recorded for, until the link is restored.
    waiting: Option<DeviceLinkState>,
    restore: Restore,
    /// Whether this `offline` episode's one reconnect was sent (`kick_once`).
    kicked: bool,
}

/// Where a handset's restore stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restore {
    Idle,
    /// `device` since `since_ms`: the preparation is queued once `settle_ms` passed.
    Settling {
        since_ms: u64,
    },
    /// A preparation is queued or running; `attempt` failed ones came before it.
    Preparing {
        attempt: usize,
    },
    /// `attempt` preparations failed; the next is queued at `due_ms`.
    Retrying {
        attempt: usize,
        due_ms: u64,
    },
}

/// Where the watch stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchState {
    /// No watch runs: no bound handset, or before the start opened it.
    Idle,
    Running,
    /// The watch ended with `problem`; it starts again at `due_ms`.
    Restarting {
        due_ms: u64,
        problem: DeviceWatchProblem,
    },
    /// The watch failed; a person's self-check of a handset starts it again.
    Failed,
}

/// What a handset's link change asks the host to do (model-398 §4.2, §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkAction {
    /// The handset left `device` for `state`, or changed state while away: `device.link`,
    /// `device.connected = false` for a state adb reports, the availability withdrawn under
    /// `device_link:<state>`, `device_link_waiting`, and for a state adb reports the retained
    /// session closed and the self-check facts dropped.
    Waiting {
        instance_id: InstanceId,
        state: DeviceLinkState,
    },
    /// Only `device.link` (and `device.connected = false` for a state adb reports) changes.
    LinkState {
        instance_id: InstanceId,
        state: DeviceLinkState,
    },
    /// The first list found a prepared handset at `device`: `device.link` and
    /// `device.connected = true`.
    Linked { instance_id: InstanceId },
    /// Queue the handset's preparation on a self-check claim.
    Prepare { instance_id: InstanceId },
    /// One `adb -s <serial> reconnect` of the handset's transport.
    Reconnect { instance_id: InstanceId },
    /// The preparation passed: `device_link_restored` and `device.connected = true`.
    Restored { instance_id: InstanceId },
}

/// What the watch's own state asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WatchAction {
    /// Start the watch again; `device_watch_restarted` with the problem that ended it.
    Restart { problem: DeviceWatchProblem },
    /// `device_watch_failed`; every handset is then `unwatched` (its own `Waiting`).
    Failed { problem: DeviceWatchProblem },
}

/// The actions of one step of [`LinkMachine`], the watch's first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct LinkActions {
    pub(super) watch: Option<WatchAction>,
    pub(super) links: Vec<LinkAction>,
}

/// Whether adb reports `state` itself (the others are the Runtime's own).
const fn reported_by_adb(state: DeviceLinkState) -> bool {
    !matches!(
        state,
        DeviceLinkState::Unwatched | DeviceLinkState::Unbound | DeviceLinkState::Device
    )
}

/// The link state machine of the bound handsets and their watch (model-398 §4.2, §4.3). Pure:
/// the host gives it the time and the configuration in effect and carries out its actions.
#[derive(Debug, Default)]
pub(super) struct LinkMachine {
    handsets: BTreeMap<InstanceId, HandsetLink>,
    watch: Option<WatchState>,
    /// The monotonic times of the watch's restarts that are still within the window.
    restarts: VecDeque<u64>,
}

impl LinkMachine {
    /// The machine of `handsets` (instance id, alias), every link not yet listed.
    pub(super) fn new(handsets: impl IntoIterator<Item = (InstanceId, String)>) -> Self {
        Self {
            handsets: handsets
                .into_iter()
                .map(|(instance_id, alias)| {
                    (
                        instance_id,
                        HandsetLink {
                            alias,
                            state: None,
                            waiting: None,
                            restore: Restore::Idle,
                            kicked: false,
                        },
                    )
                })
                .collect(),
            watch: Some(WatchState::Idle),
            restarts: VecDeque::new(),
        }
    }

    /// Whether the machine watches any handset.
    pub(super) fn watches_handsets(&self) -> bool {
        !self.handsets.is_empty()
    }

    /// The alias of a watched handset.
    pub(super) fn alias(&self, instance_id: InstanceId) -> Option<&str> {
        self.handsets
            .get(&instance_id)
            .map(|handset| handset.alias.as_str())
    }

    /// The link state that holds a watched handset: its last known state when that is not
    /// `device`; `None` for a handset at `device` or not yet listed, and for any other
    /// instance.
    pub(super) fn hold(&self, instance_id: InstanceId) -> Option<DeviceLinkState> {
        self.handsets
            .get(&instance_id)
            .and_then(|handset| handset.state)
            .filter(|state| *state != DeviceLinkState::Device)
    }

    /// The watch opened.
    pub(super) fn watch_opened(&mut self) {
        self.watch = Some(WatchState::Running);
    }

    /// The watch could not be started, at the start or at a restart (`spawn_failed`): it is
    /// failed (§4.3).
    pub(super) fn watch_spawn_failed(&mut self) -> LinkActions {
        self.fail_watch(DeviceWatchProblem::SpawnFailed)
    }

    /// One list of the watch: each handset's link state by its alias. `prepared` names the
    /// handsets whose stored self-check passes, read for a handset's first list only.
    pub(super) fn on_links(
        &mut self,
        now_ms: u64,
        config: &DeviceLinkConfig,
        links: &BTreeMap<String, DeviceLinkState>,
        prepared: &BTreeSet<InstanceId>,
    ) -> LinkActions {
        let mut actions = LinkActions::default();
        for (instance_id, handset) in &mut self.handsets {
            let Some(state) = links.get(&handset.alias).copied() else {
                continue;
            };
            let previous = handset.state.replace(state);
            if previous == Some(state) {
                continue;
            }
            if state == DeviceLinkState::Device {
                handset.kicked = false;
                if previous.is_none() && handset.waiting.is_none() && prepared.contains(instance_id)
                {
                    handset.restore = Restore::Idle;
                    actions.links.push(LinkAction::Linked {
                        instance_id: *instance_id,
                    });
                } else {
                    handset.restore = Restore::Settling { since_ms: now_ms };
                    actions.links.push(LinkAction::LinkState {
                        instance_id: *instance_id,
                        state,
                    });
                }
                continue;
            }
            handset.restore = Restore::Idle;
            if state != DeviceLinkState::Offline {
                handset.kicked = false;
            }
            if handset.waiting == Some(state) {
                actions.links.push(LinkAction::LinkState {
                    instance_id: *instance_id,
                    state,
                });
            } else {
                handset.waiting = Some(state);
                actions.links.push(LinkAction::Waiting {
                    instance_id: *instance_id,
                    state,
                });
            }
            if state == DeviceLinkState::Offline
                && config.offline_action == OfflineAction::KickOnce
                && !handset.kicked
            {
                handset.kicked = true;
                actions.links.push(LinkAction::Reconnect {
                    instance_id: *instance_id,
                });
            }
        }
        actions
    }

    /// The timers due at `now_ms`: a handset settled at `device` for `settle_ms`, a restore
    /// retry, a watch restart.
    pub(super) fn on_tick(&mut self, now_ms: u64, config: &DeviceLinkConfig) -> LinkActions {
        let mut actions = LinkActions::default();
        if let Some(WatchState::Restarting { due_ms, problem }) = self.watch
            && now_ms >= due_ms
        {
            self.restarts.push_back(now_ms);
            self.watch = Some(WatchState::Running);
            actions.watch = Some(WatchAction::Restart { problem });
        }
        for (instance_id, handset) in &mut self.handsets {
            let attempt = match handset.restore {
                Restore::Settling { since_ms }
                    if now_ms.saturating_sub(since_ms) >= config.settle_ms =>
                {
                    0
                }
                Restore::Retrying { attempt, due_ms } if now_ms >= due_ms => attempt,
                Restore::Idle
                | Restore::Settling { .. }
                | Restore::Preparing { .. }
                | Restore::Retrying { .. } => continue,
            };
            handset.restore = Restore::Preparing { attempt };
            actions.links.push(LinkAction::Prepare {
                instance_id: *instance_id,
            });
        }
        actions
    }

    /// The handset's restore preparation ended, `passed` or not. A result the link no longer
    /// waits for (it left `device`, or no preparation was asked) changes nothing.
    pub(super) fn on_prepared(
        &mut self,
        now_ms: u64,
        config: &DeviceLinkConfig,
        instance_id: InstanceId,
        passed: bool,
    ) -> LinkActions {
        let mut actions = LinkActions::default();
        let Some(handset) = self.handsets.get_mut(&instance_id) else {
            return actions;
        };
        let Restore::Preparing { attempt } = handset.restore else {
            return actions;
        };
        if handset.state != Some(DeviceLinkState::Device) {
            handset.restore = Restore::Idle;
            return actions;
        }
        if passed {
            handset.restore = Restore::Idle;
            handset.waiting = None;
            actions.links.push(LinkAction::Restored { instance_id });
            return actions;
        }
        handset.restore = match config.restore_retry_ms.get(attempt) {
            Some(wait_ms) => Restore::Retrying {
                attempt: attempt.saturating_add(1),
                due_ms: now_ms.saturating_add(*wait_ms),
            },
            None => Restore::Idle,
        };
        actions
    }

    /// The watch ended with `problem` at `now_ms`: started again after its wait while the
    /// restarts within the window stay within the limit; failed otherwise, and at once for an
    /// output of unknown shape (§4.1, §4.3).
    pub(super) fn on_watch_ended(
        &mut self,
        now_ms: u64,
        config: &DeviceLinkConfig,
        problem: DeviceWatchProblem,
    ) -> LinkActions {
        if self.watch != Some(WatchState::Running) {
            return LinkActions::default();
        }
        if problem == DeviceWatchProblem::ProtocolInvalid {
            return self.fail_watch(problem);
        }
        let window_start = now_ms.saturating_sub(config.watch_restart_window_ms);
        while self
            .restarts
            .front()
            .is_some_and(|restart| *restart < window_start)
        {
            self.restarts.pop_front();
        }
        let limit = usize::try_from(config.watch_restart_limit).unwrap_or(usize::MAX);
        if self.restarts.len() >= limit {
            return self.fail_watch(DeviceWatchProblem::RestartBudgetSpent);
        }
        self.watch = Some(WatchState::Restarting {
            due_ms: now_ms.saturating_add(config.restart_backoff_ms(self.restarts.len())),
            problem,
        });
        LinkActions::default()
    }

    /// A person's self-check of a handset (§4.3 end): a failed watch is started again at the
    /// next tick with a fresh restart budget, and every `unwatched` handset waits for the
    /// watch's first list.
    pub(super) fn rewatch(&mut self, now_ms: u64) {
        if self.watch != Some(WatchState::Failed) {
            return;
        }
        self.restarts.clear();
        self.watch = Some(WatchState::Restarting {
            due_ms: now_ms,
            problem: DeviceWatchProblem::RestartBudgetSpent,
        });
        for handset in self.handsets.values_mut() {
            if handset.state == Some(DeviceLinkState::Unwatched) {
                handset.state = None;
            }
        }
    }

    /// The watch is failed: `device_watch_failed`, then every handset `unwatched`.
    fn fail_watch(&mut self, problem: DeviceWatchProblem) -> LinkActions {
        self.watch = Some(WatchState::Failed);
        let mut actions = LinkActions {
            watch: Some(WatchAction::Failed { problem }),
            links: Vec::new(),
        };
        for (instance_id, handset) in &mut self.handsets {
            handset.state = Some(DeviceLinkState::Unwatched);
            handset.restore = Restore::Idle;
            handset.kicked = false;
            if handset.waiting != Some(DeviceLinkState::Unwatched) {
                handset.waiting = Some(DeviceLinkState::Unwatched);
                actions.links.push(LinkAction::Waiting {
                    instance_id: *instance_id,
                    state: DeviceLinkState::Unwatched,
                });
            }
        }
        actions
    }
}

/// The host's device-link state: the machine and the running watch.
#[derive(Default)]
pub(super) struct DeviceLinks {
    machine: LinkMachine,
    /// The generation of the running watch; events of an older one are dropped.
    generation: u64,
    stopper: Option<Arc<dyn DeviceLinkWatchStop>>,
    reader: Option<JoinHandle<RuntimeHostResult<()>>>,
}

/// One restore preparation queued on a self-check claim (model-398 §4.3).
pub(super) struct PendingLinkRestore {
    pub(super) instance_alias: String,
    pub(super) instance_id: InstanceId,
    /// The instance and the restore's causation: what a worker failure is recorded under.
    pub(super) links: EventLinksDraft,
}

/// The device-link watch's start, after the daemon-start preparation: the machine of every
/// bound handset, then the watch when there is one (§4.1). A watch that cannot start is
/// `device_watch_failed` and leaves the handsets `unwatched`; the Runtime goes on.
pub(super) fn start(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    let handsets = lock(&shared.registered_instances, HostLocation::DeviceLinkWatch)?
        .values()
        .filter(|instance| {
            instance
                .bound_adb_endpoint()
                .and_then(ResolvedAdbEndpoint::handset)
                .and_then(actingcommand_execution_kernel::HandsetEndpoint::binding)
                .is_some()
        })
        .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
        .collect::<Vec<_>>();
    let machine = LinkMachine::new(handsets);
    let watches = machine.watches_handsets();
    lock(&shared.device_links, HostLocation::DeviceLinkWatch)?.machine = machine;
    if !watches {
        return Ok(());
    }
    open_watch(shared, None)
}

/// Opens the watch and starts its reader (`restart` names the problem a restart follows;
/// `None` at the start). A watch that cannot be started is `device_watch_failed`
/// (`spawn_failed`), its OS text the cause, and every handset `unwatched`.
fn open_watch(
    shared: &Arc<HostShared>,
    restart: Option<DeviceWatchProblem>,
) -> RuntimeHostResult<()> {
    stop_watch(shared)?;
    let opened = shared.execution()?.open_device_link_watch();
    let watch = match opened {
        Ok(Some(watch)) => watch,
        // The provider has no bound handset after all: nothing to watch.
        Ok(None) => return Ok(()),
        Err(error) => {
            let actions = lock(&shared.device_links, HostLocation::DeviceLinkWatch)?
                .machine
                .watch_spawn_failed();
            shared.record_device_watch_failed(
                DeviceWatchProblem::SpawnFailed,
                Some(
                    Outcome::new(ContractCode::ForeignOsError)
                        .raw_source(RawSource::Os)
                        .raw_text(error.to_string()),
                ),
            )?;
            return shared.apply_link_actions(&actions.links);
        }
    };
    let stopper = watch.stopper();
    let generation = {
        let mut links = lock(&shared.device_links, HostLocation::DeviceLinkWatch)?;
        links.machine.watch_opened();
        links.stopper = Some(Arc::clone(&stopper));
        links.generation
    };
    let reader_shared = Arc::clone(shared);
    let reader = thread::Builder::new()
        .name("actingcommand-runtime-device-link".to_string())
        .spawn(move || read_watch(&reader_shared, watch, generation))
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::DeviceWatchFailed,
                HostLocation::DeviceLinkWatch,
                RuntimeErrorCode::RuntimeHalted,
            )
        });
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            // The process runs but nothing reads it: stop it before the error ends the Runtime.
            if let Err(stop) = stopper.stop() {
                return Err(error.with_related_failure(
                    "device_link_watch_stop",
                    &RuntimeHostError::request(
                        HostCode::DeviceWatchFailed,
                        HostLocation::DeviceLinkWatch,
                        RuntimeErrorCode::BackendOperationFailed,
                    )
                    .with_native_detail(stop.to_string()),
                ));
            }
            return Err(error);
        }
    };
    lock(&shared.device_links, HostLocation::DeviceLinkWatch)?.reader = Some(reader);
    if let Some(problem) = restart {
        shared.record_device_link_observed(
            shared.events.system_links()?,
            Outcome::new(HostCode::DeviceWatchRestarted).device_watch_problem(problem),
        )?;
    }
    Ok(())
}

/// The reader of one watch: every event to the host until the watch is over or the Runtime
/// stops. Only a fatal failure ends it early, and marks the Runtime.
fn read_watch(
    shared: &HostShared,
    mut watch: Box<dyn DeviceLinkWatch>,
    generation: u64,
) -> RuntimeHostResult<()> {
    loop {
        let event = watch.next_event();
        if shared.fatal.is_shutdown_requested() {
            return Ok(());
        }
        let over = !matches!(event, DeviceLinkWatchEvent::Links(_));
        if let Err(error) = shared.device_link_event(generation, event) {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
        if over {
            return Ok(());
        }
    }
}

/// Stops the running watch, if any, and joins its reader. A new generation starts first, so
/// nothing the stopped watch still reports is applied. A stop that cannot be confirmed is
/// returned as a request error after the reader was joined.
pub(super) fn stop_watch(shared: &HostShared) -> RuntimeHostResult<()> {
    let (stopper, reader) = {
        let mut links = lock(&shared.device_links, HostLocation::DeviceLinkWatch)?;
        links.generation = links.generation.saturating_add(1);
        (links.stopper.take(), links.reader.take())
    };
    let stopped = stopper.map_or(Ok(()), |stopper| {
        stopper.stop().map_err(|error| {
            RuntimeHostError::request(
                HostCode::DeviceWatchFailed,
                HostLocation::DeviceLinkWatch,
                RuntimeErrorCode::BackendOperationFailed,
            )
            .with_native_detail(error.to_string())
        })
    });
    let joined = join_runtime_thread(reader, HostLocation::DeviceLinkWatch);
    joined.and(stopped)
}

/// The lease sweep's tick: the timers due now (settle, restore retries, the watch's restart).
pub(super) fn tick(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    if !lock(&shared.device_links, HostLocation::DeviceLinkApply)?
        .machine
        .watches_handsets()
    {
        return Ok(());
    }
    let now_ms = shared.monotonic_ms()?;
    let config = shared.device_link_config()?;
    let actions = lock(&shared.device_links, HostLocation::DeviceLinkApply)?
        .machine
        .on_tick(now_ms, &config);
    apply(shared, actions)
}

/// Carries out one step's actions: the watch's first (a restart opens it again), then each
/// handset's.
fn apply(shared: &Arc<HostShared>, actions: LinkActions) -> RuntimeHostResult<()> {
    match actions.watch {
        Some(WatchAction::Restart { problem }) => open_watch(shared, Some(problem))?,
        Some(WatchAction::Failed { problem }) => {
            shared.record_device_watch_failed(problem, None)?
        }
        None => {}
    }
    shared.apply_link_actions(&actions.links)
}

impl HostShared {
    /// One event of the watch of `generation`; an older watch's event changes nothing. A
    /// list moves the handsets' states; an end or an unknown output ends the watch. Only a
    /// fatal failure is returned.
    pub(super) fn device_link_event(
        &self,
        generation: u64,
        event: DeviceLinkWatchEvent,
    ) -> RuntimeHostResult<()> {
        let now_ms = self.monotonic_ms()?;
        let config = self.device_link_config()?;
        let handsets = {
            let links = lock(&self.device_links, HostLocation::DeviceLinkApply)?;
            if links.generation != generation {
                return Ok(());
            }
            links.machine.handsets.keys().copied().collect::<Vec<_>>()
        };
        let mut prepared = BTreeSet::new();
        if matches!(event, DeviceLinkWatchEvent::Links(_)) {
            for instance_id in handsets {
                if self.backend_selfcheck_passed(instance_id)? {
                    prepared.insert(instance_id);
                }
            }
        }
        let actions = {
            let mut links = lock(&self.device_links, HostLocation::DeviceLinkApply)?;
            if links.generation != generation {
                return Ok(());
            }
            match &event {
                DeviceLinkWatchEvent::Links(states) => {
                    links.machine.on_links(now_ms, &config, states, &prepared)
                }
                DeviceLinkWatchEvent::Ended => {
                    links
                        .machine
                        .on_watch_ended(now_ms, &config, DeviceWatchProblem::Exited)
                }
                DeviceLinkWatchEvent::ProtocolInvalid => links.machine.on_watch_ended(
                    now_ms,
                    &config,
                    DeviceWatchProblem::ProtocolInvalid,
                ),
            }
        };
        if let Some(WatchAction::Failed { problem }) = actions.watch {
            self.record_device_watch_failed(problem, None)?;
        }
        // A list or an end never restarts the watch at once: the restart waits for its tick.
        self.apply_link_actions(&actions.links)
    }

    /// The generation of the running watch (a test hands its own events in under it).
    #[cfg(test)]
    pub(super) fn device_link_generation(&self) -> RuntimeHostResult<u64> {
        Ok(lock(&self.device_links, HostLocation::DeviceLinkRead)?.generation)
    }

    /// The link state that holds the instance (A9, the availability gate): a watched handset's
    /// last known state when it is not `device`; `None` for every other instance.
    pub(super) fn device_link_hold(
        &self,
        instance_id: InstanceId,
    ) -> RuntimeHostResult<Option<DeviceLinkState>> {
        Ok(lock(&self.device_links, HostLocation::DeviceLinkRead)?
            .machine
            .hold(instance_id))
    }

    /// Workflow #398 RD-2 (model-398 v1.1 §4.4): a run's device failure `error`, met while
    /// the instance's handset is no longer `device`, as `device_link_lost` caused by it; `None`
    /// for any other failure or instance. Only a failure of the device itself (a backend that
    /// would not open or operate, a capture that failed) is the link's, whether the device
    /// layer called it fatal or not: an adb command against a handset that left fails fatal.
    /// A failure whose resource close stayed unconfirmed keeps its own handling (Workflow #403).
    pub(super) fn device_link_lost(
        &self,
        instance_id: InstanceId,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<Option<RuntimeHostError>> {
        if error.lifecycle.resource_quiescence == Some(ResourceQuiescence::Unconfirmed)
            || !matches!(
                error.projection().code,
                RuntimeErrorCode::BackendOperationFailed
                    | RuntimeErrorCode::BackendOpenFailed
                    | RuntimeErrorCode::CaptureFailed
            )
        {
            return Ok(None);
        }
        let held = {
            let links = lock(&self.device_links, HostLocation::DeviceLinkRun)?;
            links
                .machine
                .hold(instance_id)
                .zip(links.machine.alias(instance_id).map(str::to_owned))
        };
        Ok(held.map(|(state, alias)| {
            error
                .clone()
                .into_device_link_lost(&handset_label(&alias), state)
        }))
    }

    /// A person's self-check of the handset `instance_id` starts a failed watch again (§4.3).
    pub(super) fn device_link_rewatch(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let now_ms = self.monotonic_ms()?;
        let mut links = lock(&self.device_links, HostLocation::DeviceLinkApply)?;
        if links.machine.alias(instance_id).is_some() {
            links.machine.rewatch(now_ms);
        }
        Ok(())
    }

    /// The restore preparation of `instance_id` ended, `passed` or not.
    pub(super) fn device_link_prepared(
        &self,
        instance_id: InstanceId,
        passed: bool,
    ) -> RuntimeHostResult<()> {
        let now_ms = self.monotonic_ms()?;
        let config = self.device_link_config()?;
        let actions = lock(&self.device_links, HostLocation::DeviceLinkApply)?
            .machine
            .on_prepared(now_ms, &config, instance_id, passed);
        self.apply_link_actions(&actions.links)
    }

    /// Carries out the handsets' actions in order. A failure that leaves the Runtime running
    /// is recorded under the handset's links and the next action goes on; a fatal one is
    /// returned.
    fn apply_link_actions(&self, actions: &[LinkAction]) -> RuntimeHostResult<()> {
        for action in actions {
            let instance_id = match *action {
                LinkAction::Waiting { instance_id, .. }
                | LinkAction::LinkState { instance_id, .. }
                | LinkAction::Linked { instance_id }
                | LinkAction::Prepare { instance_id }
                | LinkAction::Reconnect { instance_id }
                | LinkAction::Restored { instance_id } => instance_id,
            };
            let Some(alias) = lock(&self.device_links, HostLocation::DeviceLinkApply)?
                .machine
                .alias(instance_id)
                .map(str::to_owned)
            else {
                continue;
            };
            let links = self.device_link_links(instance_id)?;
            let result = match *action {
                LinkAction::Waiting { state, .. } => {
                    self.device_link_waiting(instance_id, &alias, state, &links)
                }
                LinkAction::LinkState { state, .. } => self.record_link_facts(instance_id, state),
                LinkAction::Linked { .. } => self
                    .record_link_facts(instance_id, DeviceLinkState::Device)
                    .and_then(|()| self.record_link_connected(instance_id, true)),
                LinkAction::Prepare { .. } => self.queue_link_restore(instance_id, &alias),
                LinkAction::Reconnect { .. } => self.execution().and_then(|execution| {
                    execution.reconnect_handset(&alias).map_err(|error| {
                        RuntimeHostError::request(
                            HostCode::DeviceLinkReconnectFailed,
                            HostLocation::DeviceLinkReconnect,
                            RuntimeErrorCode::BackendOperationFailed,
                        )
                        .with_outcome_values(|outcome| outcome.device_label(handset_label(&alias)))
                        .with_native_detail(error.to_string())
                    })
                }),
                LinkAction::Restored { .. } => self
                    .record_link_connected(instance_id, true)
                    .and_then(|()| {
                        self.record_device_link_observed(
                            links.clone(),
                            Outcome::new(HostCode::DeviceLinkRestored)
                                .device_label(handset_label(&alias)),
                        )
                    }),
            };
            match result {
                Ok(()) => {}
                Err(error) if error.is_fatal() => {
                    self.fatal.mark(error.clone())?;
                    return Err(error);
                }
                Err(error) => self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&error),
                    links,
                    None,
                )?,
            }
        }
        Ok(())
    }

    /// The handset left `device` for `state` (§4.2): the facts, the availability under
    /// `device_link:<state>`, `device_link_waiting`, and for a state adb reports the retained
    /// session closed and the self-check facts dropped, so only a passing preparation makes it
    /// available again.
    fn device_link_waiting(
        &self,
        instance_id: InstanceId,
        alias: &str,
        state: DeviceLinkState,
        links: &EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        self.record_link_facts(instance_id, state)?;
        self.regate_policy_instance_availability(instance_id)?;
        self.record_device_link_observed(
            links.clone(),
            Outcome::new(HostCode::DeviceLinkWaiting)
                .device_label(handset_label(alias))
                .device_link_state(state),
        )?;
        if !reported_by_adb(state) {
            return Ok(());
        }
        self.close_device_link_session(instance_id, links)?;
        self.invalidate_backend_selfcheck_facts(instance_id)
    }

    /// Closes the session a lease-free path retained (§4.2), through the fenced close path. A
    /// held instance is left to its holder, whose next device step fails and closes it.
    fn close_device_link_session(
        &self,
        instance_id: InstanceId,
        links: &EventLinksDraft,
    ) -> RuntimeHostResult<()> {
        if lock(&self.scheduler, HostLocation::DeviceLinkSessionClose)?
            .active_lease(instance_id)
            .is_some()
        {
            return Ok(());
        }
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, HostLocation::DeviceLinkSessionClose)?;
        let closed = self.close_retained_instance_while_guarded(
            instance_id,
            links.clone(),
            false,
            LeaseReleaseReason::BackendFailure,
            &admission,
        )?;
        drop(admission);
        match closed {
            Ok(()) => Ok(()),
            Err(close_error) => Err(self.retained_close_failure(
                HostLocation::DeviceLinkSessionClose,
                instance_id,
                &close_error,
            )?),
        }
    }

    /// `device.link` = `state`, and `device.connected = false` for a state adb reports.
    fn record_link_facts(
        &self,
        instance_id: InstanceId,
        state: DeviceLinkState,
    ) -> RuntimeHostResult<()> {
        self.record_device_link_fact(
            instance_id,
            DEVICE_LINK_FACT_KEY,
            RuntimeFactValue::String(state.as_str().to_owned()),
        )?;
        if reported_by_adb(state) {
            self.record_link_connected(instance_id, false)?;
        }
        Ok(())
    }

    fn record_link_connected(
        &self,
        instance_id: InstanceId,
        connected: bool,
    ) -> RuntimeHostResult<()> {
        self.record_device_link_fact(
            instance_id,
            DEVICE_CONNECTED_FACT_KEY,
            RuntimeFactValue::Boolean(connected),
        )
    }

    /// Records one instance fact of the link when its value changes, observed now and after
    /// the stored record of its key, ledger-first.
    fn record_device_link_fact(
        &self,
        instance_id: InstanceId,
        key: &str,
        value: RuntimeFactValue,
    ) -> RuntimeHostResult<()> {
        let scope = RuntimeFactScope::Instance { instance_id };
        let stored_at = {
            let store = lock(&self.runtime_facts, HostLocation::DeviceLinkApply)?;
            match store.get(&scope, key) {
                Some(record) if record.value == value => return Ok(()),
                Some(record) => Some(record.observed_at_unix_ms),
                None => None,
            }
        };
        let now = self.clock.sample()?.unix_ms;
        let observed_at_unix_ms = match stored_at {
            Some(stored) => now.max(stored.saturating_add(1)),
            None => now,
        };
        self.record_runtime_fact(RuntimeFactRecord {
            scope,
            key: key.to_owned(),
            value,
            observed_at_unix_ms,
            source: OriginModule::Runtime,
            ttl_ms: None,
        })
        .map(|_| ())
    }

    /// A `runtime.lifecycle_observed` `device_link_observed` record carrying `outcome`; its
    /// severity is the outcome's category.
    fn record_device_link_observed(
        &self,
        links: EventLinksDraft,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::DeviceLinkObserved { outcome },
                AuditInput::new(),
            ),
        )
        .map(|_| ())
    }

    /// `device_watch_failed` with its problem under system links; a watch that could not be
    /// started carries the OS text as its `caused_by` link.
    fn record_device_watch_failed(
        &self,
        problem: DeviceWatchProblem,
        cause: Option<Outcome>,
    ) -> RuntimeHostResult<()> {
        let outcome = Outcome::new(HostCode::DeviceWatchFailed).device_watch_problem(problem);
        self.record_device_link_observed(
            self.events.system_links()?,
            match cause {
                Some(cause) => outcome.caused_by(cause),
                None => outcome,
            },
        )
    }

    /// The links of a handset's records: the instance under fresh system links.
    fn device_link_links(&self, instance_id: InstanceId) -> RuntimeHostResult<EventLinksDraft> {
        Ok(self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id)))
    }

    /// Queues the handset's preparation on a self-check claim of its queue (§4.3; no new claim
    /// kind). A claim the queue refuses is recorded and counts as a failed preparation.
    fn queue_link_restore(&self, instance_id: InstanceId, alias: &str) -> RuntimeHostResult<()> {
        let resolved = self
            .resolve_instance(alias)
            .map_err(|failure| *failure.error)?;
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
                instance_alias: alias.to_owned(),
                holder_id,
            },
        )
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::DeviceLinkRestore,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RuntimeHostError::scheduler(HostLocation::DeviceLinkRestore, &error)
            })?;
        let lease_ttl_ms = lock(&self.scheduler, HostLocation::DeviceLinkRestore)?
            .config()
            .lease_ttl_ms;
        let links = self
            .device_link_links(instance_id)?
            .with_causation_id(causation_id);
        let queued = self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: alias,
                holder_id,
                connection_id,
                kind: ClaimKind::SelfCheck,
                priority: LeasePriority::Normal,
                lease_ttl_ms,
            },
            HostClaimWork::LinkRestore(PendingLinkRestore {
                instance_alias: alias.to_owned(),
                instance_id,
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
                    links,
                    None,
                )?;
                self.device_link_prepared(instance_id, false)
            }
        }
    }

    /// A granted restore claim on the handset's worker: today's connection preparation on the
    /// claim's key, then the key's release (to the ladder a failed preparation admitted, by
    /// `preparation_backend`), then the machine hears whether it passed. Only a fatal failure
    /// is returned.
    pub(super) fn run_link_restore(
        &self,
        pending: &PendingLinkRestore,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        let validated = key.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::DeviceLinkRestore,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
        let instance_guard = self
            .instance_guard(pending.instance_id)
            .map_err(|failure| *failure.error)?;
        let admission = lock(&instance_guard, HostLocation::DeviceLinkRestore)?;
        let prepared = self.prepare_instance_connection(
            &pending.instance_alias,
            pending.instance_id,
            links,
            &admission,
            &key.token,
            key.connection_id,
        );
        drop(admission);
        let (selfcheck, ladder) = prepared?;
        let released = match ladder {
            Some(ladder) => match self.release_to_preparation_ladder(
                &validated,
                key.request.request_id(),
                &key.token,
                key.connection_id,
                ladder,
            ) {
                Ok(()) => Ok(()),
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    Err(*failure.error)
                }
                Err(failure) => self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&failure.error),
                    pending.links.clone(),
                    None,
                ),
            },
            None => self.release_host_key(&key),
        };
        released?;
        self.device_link_prepared(pending.instance_id, selfcheck.failure_code.is_none())
    }
}

#[cfg(test)]
mod tests;
