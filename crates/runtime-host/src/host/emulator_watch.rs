// SPDX-License-Identifier: AGPL-3.0-only

//! emu-watch (Alice's ruling 35: starting the emulators belongs to the Runtime; one that runs is
//! used, one that does not is started, and a scheduling pause holds only dispatches; review item
//! 5): the Runtime watches the emulators of its instances.
//!
//! - **Start and stop.** One watch thread per Runtime, started by actingd
//!   (`RuntimeHost::start_emulator_watch`) right after the configured starts are enqueued; it
//!   ends with the shutdown request and is joined at the close.
//! - **Polls (R2).** Every tick the watch reads `emulator_control.watch_enabled` and
//!   `emulator_control.watch_interval_ms` from the configuration in effect, so an instruction
//!   changes them while the Runtime runs. While it is on, a poll is due one interval after the
//!   last one (the first one interval after the start). A poll reads the emulator status of every
//!   discovery-bound instance (`MuMuManager info` through the execution kernel, bounded; the read
//!   that never starts anything; never `api`) under the instance's admission guard when that guard
//!   is free; an instance that a control, a preparation or a run holds is read at a later poll.
//!   The reading replaces the instance's `emulator_state` (`on` when the emulator runs with
//!   Android and an ADB port, as a discovery reads it). When it differs from the reading the
//!   Runtime held, one `device_state_observed` record `emulator_watch_state_changed` (info) under
//!   the instance. A failed read is `emulator_state_read_failed` (warning), once per failing
//!   streak; the reading stays.
//! - **Restarts (R3).** A configured instance (`start_emulator`) whose read shows no emulator
//!   process, whose key no claim holds and on which no autostart or emulator control claim waits
//!   (never two starts at once), is started again, paused or not: `emulator_autostart_queued`
//!   (info, `count` 1), then one autostart claim marked `watch` (`emulator_autostart`), whose
//!   Start and result are recorded and reported as every configured start's. At most
//!   `emulator_control.restart_attempts` restarts of one instance within
//!   `emulator_control.restart_window_ms`; once they are used up, `emulator_watch_restart_exhausted`
//!   (warning) once, and no restart until the oldest leaves the window or a person's emulator
//!   control of the instance clears the count. The stuck-recovery ladder adds none: direction ①
//!   does not count an autostart claim's records (`recovery_ladder`).
//! - **Blackouts** (Workflow #386 SR-4a3, `scheduling-blackout.md`, "Complete silence"): the
//!   polls go on (a status read starts nothing). A configured instance found stopped inside a
//!   blackout gets its one autostart claim as above, which the routine gate holds until the
//!   blackout's end (no key, no Start; `emulator_autostart`); while it waits, the watch requests
//!   none again, so the restart counts once.
//! - **Codes (#420 S4).** The watch's own codes are the state change and the used-up bound; the
//!   read failure, the restart's enqueue and a failed thread spawn are existing codes
//!   (`emulator_state_read_failed`, `emulator_autostart_queued`, `runtime_monitor_spawn_failed`).
//! - **Observe only (R4).** An instance without `start_emulator` is read and recorded, never
//!   started. Nothing here stops or restarts a running emulator, and nothing names a device: an
//!   instance is its alias.
//!
//! Each record is also a report line, which actingd writes as a daemon line.

use super::*;
use crate::codes::{EmulatorWatchCode, EmulatorWatchLocation, HostCode};
use actingcommand_contract::EmulatorRunningState;
use actingcommand_runtime_config::EmulatorWatchConfig;

/// How often the watch looks whether a poll is due and whether the Runtime stops.
const WATCH_TICK: Duration = Duration::from_millis(250);

/// The watch's state: its thread, and per instance the restarts it requested within the window
/// and the streaks it reports once. A leaf lock: nothing else is taken while it is held.
#[derive(Default)]
pub(super) struct EmulatorWatch {
    thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    /// The restarts the watch requested, by instance: their monotonic times, the oldest first.
    restarts: BTreeMap<InstanceId, VecDeque<u64>>,
    /// The instances whose used-up bound was reported, until a restart is allowed again.
    exhausted: BTreeSet<InstanceId>,
    /// The instances whose last status read failed (reported once per failing streak).
    unreadable: BTreeSet<InstanceId>,
    /// Test only: the polls finished so far.
    #[cfg(test)]
    polls: u64,
}

/// Whether the watch may start an instance's stopped emulator now (R3's bound).
enum RestartBudget {
    /// It may: the restart is counted, the `attempts`-th in the window.
    Allowed { attempts: usize },
    /// The bound is used up: `attempts` restarts in the window, the oldest of which leaves it in
    /// `retry_after_ms`; `first` when this poll reports the used-up bound.
    UsedUp {
        attempts: usize,
        retry_after_ms: u64,
        first: bool,
    },
}

impl EmulatorWatch {
    /// The bound of `instance_id` at `now_ms`: the restarts that left the window are dropped
    /// first; an allowed restart is counted.
    fn restart_budget(
        &mut self,
        instance_id: InstanceId,
        now_ms: u64,
        settings: &EmulatorWatchConfig,
    ) -> RestartBudget {
        let window = self.restarts.entry(instance_id).or_default();
        while window
            .front()
            .is_some_and(|at| now_ms.saturating_sub(*at) >= settings.restart_window_ms)
        {
            window.pop_front();
        }
        let limit = usize::try_from(settings.restart_attempts).unwrap_or(usize::MAX);
        if window.len() >= limit {
            let since_oldest = window.front().map_or(0, |at| now_ms.saturating_sub(*at));
            let attempts = window.len();
            return RestartBudget::UsedUp {
                attempts,
                retry_after_ms: settings.restart_window_ms.saturating_sub(since_oldest),
                first: self.exhausted.insert(instance_id),
            };
        }
        window.push_back(now_ms);
        let attempts = window.len();
        self.exhausted.remove(&instance_id);
        RestartBudget::Allowed { attempts }
    }
}

/// Starts the watch thread; a second call does nothing. Its first poll is due one interval after
/// the start: the daemon start's discovery has just read the emulators.
pub(super) fn start(shared: &Arc<HostShared>) -> RuntimeHostResult<()> {
    let first_poll_ms = shared
        .monotonic_ms()?
        .saturating_add(shared.emulator_watch_settings()?.interval_ms);
    let mut watch = lock(&shared.emulator_watch, EmulatorWatchLocation::Watch)?;
    if watch.thread.is_some() {
        return Ok(());
    }
    let watch_shared = Arc::clone(shared);
    let thread = thread::Builder::new()
        .name("actingcommand-runtime-emulator-watch".to_string())
        .spawn(move || watch_loop(&watch_shared, first_poll_ms))
        .map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::RuntimeMonitorSpawnFailed,
                EmulatorWatchLocation::Watch,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
    watch.thread = Some(thread);
    Ok(())
}

/// Joins the watch, which ends with the shutdown request.
pub(super) fn stop(shared: &HostShared) -> RuntimeHostResult<()> {
    let thread = lock(&shared.emulator_watch, EmulatorWatchLocation::Watch)?
        .thread
        .take();
    join_runtime_thread(thread, EmulatorWatchLocation::Watch)
}

/// The watch: the due polls until the Runtime stops. Only a failure the Runtime does not survive
/// ends it early, and marks the Runtime.
fn watch_loop(shared: &HostShared, first_poll_ms: u64) -> RuntimeHostResult<()> {
    let mut next_poll_ms = first_poll_ms;
    while !shared.fatal.is_shutdown_requested() {
        thread::sleep(WATCH_TICK);
        if shared.fatal.is_shutdown_requested() {
            break;
        }
        if let Err(error) = watch_tick(shared, &mut next_poll_ms) {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
    }
    Ok(())
}

/// One tick: the watch's keys in effect, then a poll when one is due and the watch is on.
fn watch_tick(shared: &HostShared, next_poll_ms: &mut u64) -> RuntimeHostResult<()> {
    let settings = shared.emulator_watch_settings()?;
    let now_ms = shared.monotonic_ms()?;
    if !settings.enabled || now_ms < *next_poll_ms {
        return Ok(());
    }
    *next_poll_ms = now_ms.saturating_add(settings.interval_ms);
    shared.poll_emulators(&settings, now_ms)
}

/// The reading `state` holds: `Some(true)` on, `Some(false)` off, `None` without a reading.
const fn reading_of(state: EmulatorRunningState) -> Option<bool> {
    match state {
        EmulatorRunningState::On { .. } => Some(true),
        EmulatorRunningState::Off { .. } => Some(false),
        EmulatorRunningState::Unknown => None,
    }
}

impl HostShared {
    /// One poll at `now_ms` (R2): the emulator status of every discovery-bound instance, and
    /// (R3) the restart of a configured one found stopped. A failure the Runtime survives is
    /// recorded under its instance and the next instance goes on; one it does not survive is
    /// returned.
    fn poll_emulators(&self, settings: &EmulatorWatchConfig, now_ms: u64) -> RuntimeHostResult<()> {
        let configured = self.emulator_autostart()?.to_vec();
        let watched = lock(&self.registered_instances, EmulatorWatchLocation::Watch)?
            .values()
            .filter(|instance| {
                instance
                    .adb_endpoint
                    .as_ref()
                    .and_then(ResolvedInstanceEndpoint::discovered_binding)
                    .is_some()
            })
            .map(|instance| (instance.instance_id(), instance.instance_alias.clone()))
            .collect::<Vec<_>>();
        for (instance_id, alias) in watched {
            if self.fatal.is_shutdown_requested() {
                break;
            }
            let position = configured
                .iter()
                .position(|(configured, _)| *configured == instance_id);
            match self.watch_emulator(instance_id, &alias, position, settings, now_ms) {
                Ok(()) => {}
                Err(error) if error.is_fatal() => return Err(error),
                Err(error) => self.append_lifecycle_failure(
                    RuntimeLifecycleFailureStage::OperationCleanup,
                    RuntimeLifecycleFailure::Host(&error),
                    self.emulator_watch_links(instance_id)?,
                    None,
                )?,
            }
        }
        #[cfg(test)]
        {
            lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?.polls += 1;
            self.emulator_watch_polled.notify_all();
        }
        Ok(())
    }

    /// One instance of a poll: its status read, and (R3) the restart of a configured instance
    /// (its place in the configuration `position`) found stopped; R4: an instance without
    /// `start_emulator` is only read and recorded.
    fn watch_emulator(
        &self,
        instance_id: InstanceId,
        alias: &str,
        position: Option<usize>,
        settings: &EmulatorWatchConfig,
        now_ms: u64,
    ) -> RuntimeHostResult<()> {
        let Some(stopped) = self.read_watched_emulator(instance_id, alias)? else {
            return Ok(());
        };
        match position {
            Some(position) if stopped => {
                self.restart_watched_emulator(position, instance_id, alias, settings, now_ms)
            }
            _ => Ok(()),
        }
    }

    /// The status read of one instance under its admission guard, when the guard is free: the
    /// reading replaces `emulator_state`, recorded when it changed. `Some(true)` when no emulator
    /// process runs, `Some(false)` when one does, `None` when the guard was taken or the read
    /// failed.
    fn read_watched_emulator(
        &self,
        instance_id: InstanceId,
        alias: &str,
    ) -> RuntimeHostResult<Option<bool>> {
        let instance_guard = self
            .instance_guard(instance_id)
            .map_err(|failure| *failure.error)?;
        let _admission = match instance_guard.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(_)) => {
                return Err(lock_poison_error(EmulatorWatchLocation::Watch));
            }
        };
        let state = match self.execution()?.read_emulator_instance_state(alias) {
            Ok(state) => state,
            Err(error) => {
                let first = lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?
                    .unreadable
                    .insert(instance_id);
                if first {
                    self.record_emulator_watch(
                        instance_id,
                        self.emulator_watch_read_failure(
                            alias,
                            EmulatorWatchLocation::Watch,
                            error.to_string(),
                        ),
                    )?;
                }
                return Ok(None);
            }
        };
        lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?
            .unreadable
            .remove(&instance_id);
        let running = state.running();
        let observed_at_unix_ms = self.clock.sample()?.unix_ms;
        let changed = {
            let mut registry = lock(&self.registered_instances, EmulatorWatchLocation::Watch)?;
            let Some(record) = registry.get_mut(&instance_id) else {
                return Ok(None);
            };
            let changed = reading_of(record.emulator_state) != Some(running);
            record.emulator_state = EmulatorRunningState::observed(running, observed_at_unix_ms);
            changed
        };
        if changed {
            self.record_emulator_watch(
                instance_id,
                Outcome::new(EmulatorWatchCode::StateChanged)
                    .alias(alias)
                    .process_started(state.process_started)
                    .android_started(state.android_started)
                    .adb_port_present(state.adb_port.is_some()),
            )?;
        }
        Ok(Some(!state.process_started))
    }

    /// R3: the restart of a configured instance whose emulator the poll found stopped, paused or
    /// not. Never while a claim holds its key or an autostart or emulator control claim waits for
    /// it (never two starts at once), and within the bound.
    fn restart_watched_emulator(
        &self,
        position: usize,
        instance_id: InstanceId,
        alias: &str,
        settings: &EmulatorWatchConfig,
        now_ms: u64,
    ) -> RuntimeHostResult<()> {
        // IDLE-1: an emulator the idle action stopped stays stopped until its wake starts it.
        if self.idle_holds_emulator_stopped(alias)? {
            return Ok(());
        }
        let busy = {
            let scheduler = lock(&self.scheduler, EmulatorWatchLocation::Watch)?;
            scheduler.active_lease(instance_id).is_some()
                || !scheduler
                    .queued_of_kind(instance_id, ClaimKind::Autostart)
                    .is_empty()
                || !scheduler
                    .queued_of_kind(instance_id, ClaimKind::EmulatorControl)
                    .is_empty()
        };
        if busy {
            return Ok(());
        }
        let budget = lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?.restart_budget(
            instance_id,
            now_ms,
            settings,
        );
        match budget {
            RestartBudget::UsedUp {
                attempts,
                retry_after_ms,
                first,
            } => {
                if first {
                    self.record_emulator_watch(
                        instance_id,
                        Outcome::new(EmulatorWatchCode::RestartExhausted)
                            .alias(alias)
                            .attempts(attempts)
                            .limit_count(settings.restart_attempts)
                            .retry_after_ms(retry_after_ms),
                    )?;
                }
                Ok(())
            }
            RestartBudget::Allowed { attempts } => {
                self.record_emulator_watch(
                    instance_id,
                    Outcome::new(HostCode::EmulatorAutostartQueued)
                        .count(1_usize)
                        .alias(alias)
                        .attempts(attempts)
                        .limit_count(settings.restart_attempts),
                )?;
                self.request_watch_autostart(position, instance_id, alias)
            }
        }
    }

    /// R3: a person's emulator control of `instance_id` clears the restarts the watch made there.
    pub(super) fn forget_watch_restarts(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let mut watch = lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?;
        watch.restarts.remove(&instance_id);
        watch.exhausted.remove(&instance_id);
        Ok(())
    }

    /// `emulator_state_read_failed` (warning) of a status read of `alias` at `operation`, its
    /// failure text (`detail`) the foreign cause.
    pub(super) fn emulator_watch_read_failure(
        &self,
        alias: &str,
        operation: impl Into<Location>,
        detail: String,
    ) -> Outcome {
        RuntimeHostError::request(
            HostCode::EmulatorStateReadFailed,
            operation,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| outcome.alias(alias))
        .with_native_detail(detail)
        .record_outcome()
    }

    /// One `device_state_observed` record carrying `outcome` under the instance, and the same
    /// report line.
    fn record_emulator_watch(
        &self,
        instance_id: InstanceId,
        outcome: Outcome,
    ) -> RuntimeHostResult<()> {
        self.append_event_raw(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            self.emulator_watch_links(instance_id)?,
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

    /// The links of the watch's records: the instance under fresh system links.
    fn emulator_watch_links(&self, instance_id: InstanceId) -> RuntimeHostResult<EventLinksDraft> {
        Ok(self
            .events
            .system_links()?
            .with_instance_id(self.events.issuer().issue_registered_instance(instance_id)))
    }

    /// Test only: the number of finished polls once it reaches `polls`, waiting at most `wait`;
    /// `None` when it did not in time.
    #[cfg(test)]
    pub(super) fn await_emulator_watch_polls(&self, polls: u64, wait: Duration) -> Option<u64> {
        let deadline = Instant::now() + wait;
        let mut watch = self.emulator_watch.lock().ok()?;
        loop {
            if watch.polls >= polls {
                return Some(watch.polls);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            watch = self.emulator_watch_polled.wait_timeout(watch, left).ok()?.0;
        }
    }
}
