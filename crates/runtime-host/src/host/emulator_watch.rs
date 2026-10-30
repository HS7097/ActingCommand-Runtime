// SPDX-License-Identifier: AGPL-3.0-only

//! emu-watch (Alice's ruling 35: starting the emulators belongs to the Runtime; one that runs is
//! used, one that does not is started, and a scheduling pause holds only dispatches; review item
//! 5): the Runtime watches the emulators of its instances. WATCH-P (0.12.32, Alice's ruling 112:
//! 「暂停中的实例，监视只看状态、不重启、不识别绑定，和「玩家持有」一样；恢复时照常检查」): for the
//! watch, an instance's own scheduling pause holds its restarts and recognitions too (below).
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
//!   (never two starts at once), is started again unless its own scheduling pause holds it
//!   (WATCH-P, below; the global pause does not): `emulator_autostart_queued`
//!   (info, `count` 1), then one autostart claim marked `watch` (`emulator_autostart`), whose
//!   Start and result are recorded and reported as every configured start's. At most
//!   `emulator_control.restart_attempts` restarts of one instance within
//!   `emulator_control.restart_window_ms`; once they are used up, `emulator_watch_restart_exhausted`
//!   (warning) once, and no restart until the oldest leaves the window or a person's emulator
//!   control of the instance clears the count. The stuck-recovery ladder adds none: direction ①
//!   does not count an autostart claim's records (`recovery_ladder`).
//! - **Recognition (EMU-BIND, 0.12.28; production 10-29: a start whose readiness wait failed left
//!   a running emulator with its endpoint pending for five hours).** An instance, configured or
//!   not, whose read shows its emulator running with Android and an ADB port while the Runtime
//!   holds its endpoint pending or bound to another port, under the same conditions as a restart
//!   (key free, no autostart or emulator control claim waiting, not held stopped by IDLE-1),
//!   unless its own scheduling pause holds it (WATCH-P), gets one autostart claim marked
//!   `recognize` (`emulator_autostart_queued`,
//!   `watch_kind` `recognize`): on its key the self-check's recognition (#411 A48) binds the
//!   reported port, awaits the ADB baseline and opens input and capture, then the startup package
//!   is queued as after a start; it never launches or stops an emulator. Its bound is a restart's
//!   (`restart_attempts` within `restart_window_ms`), counted apart; used up, one
//!   `emulator_watch_restart_exhausted` with `watch_kind` `recognize`. EMU-BIND fix 1 (0.12.28,
//!   review M1: a Start that bound the reported port and then failed, at its ADB baseline or at
//!   its deadline with a read that saw the emulator running, left the instance bound and `on`
//!   with its self-check facts dropped, and no startup package and no ladder followed): so is an
//!   instance bound to the reported port that has no self-check fact at all, whose key no claim
//!   holds or waits for, and that neither its idle action, a scheduling pause in stage
//!   `released` nor a #403 fence holds (same bound, same claim). On its key the claim reads the
//!   status once more and ends as `emulator_autostart_skipped` (`watch_kind` `recognize`, the
//!   read's values; also a record) without preparing when no recognition is needed any more.
//! - **Pauses (WATCH-P, 0.12.32; production 10-29 21:35 and 21:38: the watch restarted an
//!   instance twice while it was paused for a person's own check).** An instance that its own
//!   scheduling pause holds, in any stage (`draining`, `paused`, `released`; the player hold of
//!   #429 R-7 is such a pause), is polled and recorded as above (R2), but the watch requests no
//!   restart and no recognition for it, on either recognition path. A restart or recognition so
//!   skipped is `emulator_autostart_skipped` (info; `alias`, `watch_kind`, `skip_reason`
//!   `instance_paused`; also a `device_state_observed` record), once per pause revision and
//!   kind, not once per poll; it counts against no bound and reports no used-up bound. The resume
//!   adds nothing: its reconnect and self-check run as before, and the first poll after the pause
//!   is gone restarts or recognizes as for any instance. A watch claim requested before the pause
//!   runs as before.
//! - **Blackouts** (Workflow #386 SR-4a3, `scheduling-blackout.md`, "Complete silence"): the
//!   polls go on (a status read starts nothing). A configured instance found stopped inside a
//!   blackout gets its one autostart claim as above, which the routine gate holds until the
//!   blackout's end (no key, no Start; `emulator_autostart`); while it waits, the watch requests
//!   none again, so the restart counts once.
//! - **Codes (#420 S4).** The watch's own codes are the state change and the used-up bound; the
//!   read failure, the restart's enqueue, a skip for a pause and a failed thread spawn are
//!   existing codes (`emulator_state_read_failed`, `emulator_autostart_queued`,
//!   `emulator_autostart_skipped`, `runtime_monitor_spawn_failed`).
//! - **Observe only (R4).** An instance without `start_emulator` is read and recorded (and
//!   recognized, EMU-BIND), never started. Nothing here stops or restarts a running emulator, and
//!   nothing names a device: an instance is its alias.
//!
//! Each record is also a report line, which actingd writes as a daemon line.

use super::*;
use crate::InstanceState;
use crate::codes::{EmulatorWatchCode, EmulatorWatchLocation, HostCode};
use actingcommand_contract::EmulatorRunningState;
use actingcommand_contract::outcome::VocabularyToken;
use actingcommand_runtime_config::EmulatorWatchConfig;

/// How often the watch looks whether a poll is due and whether the Runtime stops.
const WATCH_TICK: Duration = Duration::from_millis(250);

/// The watch's state: its thread, and per instance the restarts and the recognitions it
/// requested within the window and the streaks it reports once. A leaf lock: nothing else is
/// taken while it is held.
#[derive(Default)]
pub(super) struct EmulatorWatch {
    thread: Option<JoinHandle<RuntimeHostResult<()>>>,
    /// The restarts the watch requested, by instance: their monotonic times, the oldest first.
    restarts: BTreeMap<InstanceId, VecDeque<u64>>,
    /// The instances whose used-up bound was reported, until a restart is allowed again.
    exhausted: BTreeSet<InstanceId>,
    /// EMU-BIND (0.12.28): the recognitions the watch requested, by instance, counted apart from
    /// the restarts under the same bound.
    recognitions: BTreeMap<InstanceId, VecDeque<u64>>,
    /// EMU-BIND: the instances whose used-up recognition bound was reported.
    recognitions_exhausted: BTreeSet<InstanceId>,
    /// WATCH-P (0.12.32): per instance, the revision of its own scheduling pause whose skipped
    /// restart and recognition were recorded, and which of the two were.
    paused_skips: BTreeMap<InstanceId, PausedSkips>,
    /// The instances whose last status read failed (reported once per failing streak).
    unreadable: BTreeSet<InstanceId>,
    /// Test only: the polls finished so far.
    #[cfg(test)]
    polls: u64,
}

/// EMU-BIND (0.12.28): what the watch asks of an instance's emulator, as a token of the catalog
/// vocabulary `emulator_watch_kind` (the `watch_kind` value): the start of a stopped one (R3), or
/// the recognition of a running one whose endpoint the Runtime holds pending or bound to another
/// port. Each has its own count under the one bound.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WatchKind {
    Restart,
    Recognize,
}

impl VocabularyToken for WatchKind {
    const VOCABULARY: &'static str = "emulator_watch_kind";

    fn token(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Recognize => "recognize",
        }
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// WATCH-P (0.12.32): why the watch requested no claim it would otherwise have requested for an
    /// instance (vocabulary `emulator_autostart_skip_reason`, the `skip_reason` value of
    /// `emulator_autostart_skipped`).
    pub(super) enum WatchSkipReason: "emulator_autostart_skip_reason" {
        /// The instance's own scheduling pause holds it, in any stage (Alice's ruling 112).
        InstancePaused => "instance_paused",
    }
}

/// WATCH-P (0.12.32): the skips one pause revision of an instance has recorded, by kind.
#[derive(Clone, Copy)]
struct PausedSkips {
    revision: u64,
    restart: bool,
    recognize: bool,
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
    /// The bound of `kind` for `instance_id` at `now_ms`: the requests that left the window are
    /// dropped first; an allowed one is counted. EMU-BIND: restarts and recognitions are counted
    /// apart, each against `restart_attempts` within `restart_window_ms`.
    fn restart_budget(
        &mut self,
        kind: WatchKind,
        instance_id: InstanceId,
        now_ms: u64,
        settings: &EmulatorWatchConfig,
    ) -> RestartBudget {
        let (requests, exhausted) = match kind {
            WatchKind::Restart => (&mut self.restarts, &mut self.exhausted),
            WatchKind::Recognize => (&mut self.recognitions, &mut self.recognitions_exhausted),
        };
        let window = requests.entry(instance_id).or_default();
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
                first: exhausted.insert(instance_id),
            };
        }
        window.push_back(now_ms);
        let attempts = window.len();
        exhausted.remove(&instance_id);
        RestartBudget::Allowed { attempts }
    }

    /// WATCH-P (0.12.32): whether the skip of `kind` for `instance_id`'s pause at `revision` is the
    /// first of that revision, which marks it recorded. A new revision (the instance resumed and
    /// paused again) starts over.
    fn first_paused_skip(
        &mut self,
        kind: WatchKind,
        instance_id: InstanceId,
        revision: u64,
    ) -> bool {
        let skips = self.paused_skips.entry(instance_id).or_insert(PausedSkips {
            revision,
            restart: false,
            recognize: false,
        });
        if skips.revision != revision {
            *skips = PausedSkips {
                revision,
                restart: false,
                recognize: false,
            };
        }
        let recorded = match kind {
            WatchKind::Restart => &mut skips.restart,
            WatchKind::Recognize => &mut skips.recognize,
        };
        !std::mem::replace(recorded, true)
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
    /// `start_emulator` is never started. EMU-BIND (0.12.28): an instance, configured or not,
    /// found running with Android and an ADB port while the Runtime holds its endpoint pending or
    /// bound to another port is recognized (`recognize_watched_emulator`); EMU-BIND fix 1
    /// (0.12.28, review M1): so is one bound to the reported port but unprepared on it, whose key
    /// is idle (`unprepared_on_reported_port`, `instance_key_idle`). WATCH-P (0.12.32, Alice's
    /// ruling 112): an instance its own scheduling pause holds, in any stage, is read as every
    /// instance, but gets neither; the skip is recorded once per pause revision and kind
    /// (`skip_watch_for_pause`).
    fn watch_emulator(
        &self,
        instance_id: InstanceId,
        alias: &str,
        position: Option<usize>,
        settings: &EmulatorWatchConfig,
        now_ms: u64,
    ) -> RuntimeHostResult<()> {
        let Some(state) = self.read_watched_emulator(instance_id, alias)? else {
            return Ok(());
        };
        let paused = self.instance_pause_revision(alias)?;
        if !state.process_started {
            return match (position, paused) {
                (None, _) => Ok(()),
                (Some(_), Some(revision)) => {
                    self.skip_watch_for_pause(WatchKind::Restart, instance_id, alias, revision)
                }
                (Some(position), None) => {
                    self.restart_watched_emulator(position, instance_id, alias, settings, now_ms)
                }
            };
        }
        if self.needs_recognition(instance_id, &state)?
            || (self.unprepared_on_reported_port(instance_id, alias, &state)?
                && self.instance_key_idle(instance_id)?)
        {
            return match paused {
                Some(revision) => {
                    self.skip_watch_for_pause(WatchKind::Recognize, instance_id, alias, revision)
                }
                None => {
                    self.recognize_watched_emulator(position, instance_id, alias, settings, now_ms)
                }
            };
        }
        Ok(())
    }

    /// WATCH-P (0.12.32): the revision of the scheduling pause of the instance `alias` itself, in
    /// any stage; `None` when none holds it (the global pause is no pause of the instance).
    fn instance_pause_revision(&self, alias: &str) -> RuntimeHostResult<Option<u64>> {
        Ok(lock(&self.scheduling_pause, EmulatorWatchLocation::Watch)?
            .instance_state(alias)
            .map(|pause| pause.revision))
    }

    /// WATCH-P (0.12.32, Alice's ruling 112): the restart or recognition (`kind`) the watch does
    /// not request because the instance's own pause at `revision` holds it. No claim, no count
    /// against the bound of `kind` and no used-up record; the first skip of the revision and kind
    /// is `emulator_autostart_skipped` (`alias`, `watch_kind`, `skip_reason` `instance_paused`),
    /// a `device_state_observed` record under the instance and its report line, and every later
    /// one records nothing.
    fn skip_watch_for_pause(
        &self,
        kind: WatchKind,
        instance_id: InstanceId,
        alias: &str,
        revision: u64,
    ) -> RuntimeHostResult<()> {
        let first = lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?.first_paused_skip(
            kind,
            instance_id,
            revision,
        );
        if !first {
            return Ok(());
        }
        self.record_emulator_watch(
            instance_id,
            Outcome::new(HostCode::EmulatorAutostartSkipped)
                .alias(alias)
                .watch_kind(kind)
                .skip_reason(WatchSkipReason::InstancePaused),
        )
    }

    /// EMU-BIND fix 1 (0.12.28, review M1): whether the status read `state` asks for the
    /// recognition of an instance the Runtime holds bound to the reported port but never prepared
    /// on it: its emulator runs with Android and that port; the instance holds no
    /// `backend.selfcheck.*` fact (dropped by the Start or the failed control's observation that
    /// bound it, not failed: a failed self-check admitted its ladder); and neither its idle
    /// action (IDLE-1, whatever its action), a scheduling pause of the instance in stage
    /// `released` (the device handed back; the resume's reconnect recognizes it) nor a #403
    /// fence (its ladder, recheck or person) holds it. Whether the instance's key is idle is the
    /// caller's question: the watch asks it (`instance_key_idle`), a recognition claim holds it.
    /// WATCH-P (0.12.32): a poll asks no recognition of an instance its own pause holds in any
    /// stage (`watch_emulator`); the stage check here serves the claim's read on its key.
    fn unprepared_on_reported_port(
        &self,
        instance_id: InstanceId,
        alias: &str,
        state: &InstanceState,
    ) -> RuntimeHostResult<bool> {
        let Some(adb_port) = state.adb_port.filter(|port| *port != 0 && state.running()) else {
            return Ok(false);
        };
        let bound_here = lock(&self.registered_instances, EmulatorWatchLocation::Watch)?
            .get(&instance_id)
            .is_some_and(|record| {
                !record.endpoint_pending()
                    && record
                        .bound_adb_endpoint()
                        .is_some_and(|endpoint| endpoint.port() == adb_port)
            });
        if !bound_here || self.backend_selfcheck_recorded(instance_id)? {
            return Ok(false);
        }
        let released = lock(&self.scheduling_pause, EmulatorWatchLocation::Watch)?
            .instance_state(alias)
            .is_some_and(|pause| {
                pause.stage == actingcommand_contract::InstancePauseStage::Released
            });
        Ok(!released && !self.idle_holds_instance(alias)? && !self.resource_fenced(instance_id)?)
    }

    /// EMU-BIND fix 1 (0.12.28, review M1): whether no claim holds the instance's key and none
    /// waits for it.
    fn instance_key_idle(&self, instance_id: InstanceId) -> RuntimeHostResult<bool> {
        let scheduler = lock(&self.scheduler, EmulatorWatchLocation::Watch)?;
        Ok(scheduler.active_lease(instance_id).is_none()
            && scheduler.queued_count(instance_id) == 0)
    }

    /// EMU-BIND fix 1 (0.12.28, review L2): whether the status read `state` still asks for the
    /// recognition a claim holding the instance's key was queued for: the endpoint pending or
    /// bound to another port (`needs_recognition`), or bound to the reported port but unprepared
    /// (`unprepared_on_reported_port`).
    pub(super) fn recognition_still_needed(
        &self,
        instance_id: InstanceId,
        alias: &str,
        state: &InstanceState,
    ) -> RuntimeHostResult<bool> {
        Ok(self.needs_recognition(instance_id, state)?
            || self.unprepared_on_reported_port(instance_id, alias, state)?)
    }

    /// EMU-BIND (0.12.28): whether the status read `state` asks for a recognition of the
    /// instance: its emulator runs with Android and a reported ADB port, and the Runtime holds
    /// the instance's endpoint pending or bound to another port (CAP-IDX: a port that is not the
    /// index's base port is checked when capture opens on it).
    pub(super) fn needs_recognition(
        &self,
        instance_id: InstanceId,
        state: &InstanceState,
    ) -> RuntimeHostResult<bool> {
        let Some(adb_port) = state.adb_port.filter(|port| *port != 0 && state.running()) else {
            return Ok(false);
        };
        Ok(
            lock(&self.registered_instances, EmulatorWatchLocation::Watch)?
                .get(&instance_id)
                .is_some_and(|record| {
                    record.endpoint_pending()
                        || record
                            .bound_adb_endpoint()
                            .is_none_or(|endpoint| endpoint.port() != adb_port)
                }),
        )
    }

    /// The status read of one instance under its admission guard, when the guard is free: the
    /// reading replaces `emulator_state`, recorded when it changed. The reading, or `None` when
    /// the guard was taken or the read failed.
    fn read_watched_emulator(
        &self,
        instance_id: InstanceId,
        alias: &str,
    ) -> RuntimeHostResult<Option<InstanceState>> {
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
        Ok(Some(state))
    }

    /// R3: the restart of a configured instance whose emulator the poll found stopped and that no
    /// pause of its own holds (WATCH-P; the caller asks). Never while a claim holds its key or an
    /// autostart or emulator control claim waits for it (never two starts at once), and within the
    /// bound.
    fn restart_watched_emulator(
        &self,
        position: usize,
        instance_id: InstanceId,
        alias: &str,
        settings: &EmulatorWatchConfig,
        now_ms: u64,
    ) -> RuntimeHostResult<()> {
        if !self.watch_may_request(WatchKind::Restart, instance_id, alias, settings, now_ms)? {
            return Ok(());
        }
        self.request_watch_autostart(position, instance_id, alias, false)
    }

    /// EMU-BIND (0.12.28): the recognition of an instance whose emulator the poll found running
    /// with Android and an ADB port while the Runtime holds its endpoint pending or bound to
    /// another port and that no pause of its own holds (WATCH-P; the caller asks), under the same
    /// conditions and bound as a restart, counted apart: one autostart claim marked `recognize`
    /// (`request_watch_recognition`), which starts and stops nothing. `position` is the
    /// instance's place in the configuration, if it names `start_emulator`.
    fn recognize_watched_emulator(
        &self,
        position: Option<usize>,
        instance_id: InstanceId,
        alias: &str,
        settings: &EmulatorWatchConfig,
        now_ms: u64,
    ) -> RuntimeHostResult<()> {
        if !self.watch_may_request(WatchKind::Recognize, instance_id, alias, settings, now_ms)? {
            return Ok(());
        }
        self.request_watch_recognition(position, instance_id, alias)
    }

    /// R3 and EMU-BIND: whether the watch requests a claim of `kind` for the instance now, and
    /// its record. Never while the idle action holds the emulator stopped (IDLE-1), a claim
    /// holds the instance's key or an autostart or emulator control claim waits for it (at most
    /// one per instance at a time), nor beyond the bound of `kind`: once it is used up,
    /// `emulator_watch_restart_exhausted` (`watch_kind`) once per streak. An allowed request is
    /// counted and recorded as `emulator_autostart_queued` (`watch_kind`).
    fn watch_may_request(
        &self,
        kind: WatchKind,
        instance_id: InstanceId,
        alias: &str,
        settings: &EmulatorWatchConfig,
        now_ms: u64,
    ) -> RuntimeHostResult<bool> {
        // IDLE-1: an emulator the idle action stopped stays stopped until its wake starts it.
        if self.idle_holds_emulator_stopped(alias)? {
            return Ok(false);
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
            return Ok(false);
        }
        let budget = lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?.restart_budget(
            kind,
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
                            .retry_after_ms(retry_after_ms)
                            .watch_kind(kind),
                    )?;
                }
                Ok(false)
            }
            RestartBudget::Allowed { attempts } => {
                self.record_emulator_watch(
                    instance_id,
                    Outcome::new(HostCode::EmulatorAutostartQueued)
                        .count(1_usize)
                        .alias(alias)
                        .attempts(attempts)
                        .limit_count(settings.restart_attempts)
                        .watch_kind(kind),
                )?;
                Ok(true)
            }
        }
    }

    /// R3: a person's emulator control of `instance_id` clears the restarts the watch made there;
    /// EMU-BIND: and its recognitions.
    pub(super) fn forget_watch_restarts(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let mut watch = lock(&self.emulator_watch, EmulatorWatchLocation::Watch)?;
        watch.restarts.remove(&instance_id);
        watch.exhausted.remove(&instance_id);
        watch.recognitions.remove(&instance_id);
        watch.recognitions_exhausted.remove(&instance_id);
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
    pub(super) fn record_emulator_watch(
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
