// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #369 S6c (model-369-queue v3.1 H-5, §7 row 8): the configured emulator start at
//! daemon start, rebuilt from #643 as one claim per instance in the instance's one queue.
//!
//! An instance configured with `start_emulator` (S6a; off unless the configuration names it, and
//! allowed only on a discovery-bound instance) whose emulator is stopped when the daemon starts
//! (its endpoint is pending) gets one autostart claim (`ClaimKind::Autostart`, a Runtime-internal
//! claim with no deadline that never preempts); IDLE-1P (#405): not one whose emulator the idle
//! action `stop_emulator` holds stopped (restored at this start), which its wake starts through
//! the same claim. actingd enqueues the claims once, after policy
//! initialization and before `runtime_ready`; the enqueue does not block. Every claim is visible:
//! a holder from the enqueue, or a queued entry behind another holder. emu-watch (Alice's ruling
//! 35, #414 option 3): a scheduling pause, the instance's own or the global one (a restored one
//! included), never holds or queues the claim and never makes it yield; the pause holds only the
//! instance's dispatches and its startup package (below). The claim takes neither the
//! bound-endpoint check nor the performance-control gate and is granted with drain capacity
//! (Q-2a).
//!
//! emu-watch: the emulator watch (`emulator_watch`) requests the same claim, marked `watch`, for a
//! configured instance whose emulator it found stopped. Its hold is the same, except that it
//! does not ask whether the binding is pending (an emulator that stopped behind the Runtime's
//! back is still bound): with the turn, just before the Start, it reads the emulator status once
//! (`MuMuManager info`, the read that never starts anything) and ends as
//! `emulator_autostart_skipped` when the emulator process runs, or as `emulator_autostart_failed`
//! (the read's failure its cause) when the read fails; it never stops or restarts a running
//! emulator. #405 O-2 (0.12.25, review M-1): the claim an idle wake requests (after
//! `stop_emulator`) is also marked `idle_wake`; when it so finds the emulator running, it still
//! queues the instance's startup package, once, as its Start would have handed it the key, so a
//! restart that came while the woken instance's startup package waited in memory (behind a pause
//! or an install hold) does not leave the game unstarted.
//!
//! EMU-BIND (0.12.27; production 10-29: after a reboot a configured start's readiness wait failed
//! while its emulator came up, and the running emulator kept its endpoint pending for five
//! hours): the watch requests the same claim, marked `recognize`, for an instance whose emulator
//! runs with Android and an ADB port while the Runtime holds its endpoint pending or bound to
//! another port. Its hold takes no network wait and no start turn; inside a blackout it holds on
//! until the end. On its key the self-check's recognition (#411 A48) binds the reported port
//! (`runtime.instance_bound`), awaits the ADB baseline and opens input and capture, and the hold
//! then ends as after a start: the startup package takes the key through the gate, or the ladder
//! a failed preparation admitted does. It never launches or stops an emulator. A watch claim or an
//! idle wake's claim whose read finds the emulator so recognizes it the same way instead of
//! ending as skipped.
//!
//! Once granted, the claim runs on its instance's worker and holds the key throughout. It reads
//! the operating system's connectivity status (`actingcommand_host_metrics`, P3; the Runtime
//! opens no connection and resolves no name) at once and every 5 s, for at most 600 s: internet
//! access goes on, an unknown status goes on at once with one warning line, and no internet in
//! time refuses the start with `emulator_autostart_network_unready`. It then waits for the start
//! turn, which the claims of one daemon start take one at a time in configuration order. The
//! wait ticks every 500 ms; at each tick it renews the key to 120 s whenever less than 30 s is
//! left (B1, review M-4), and ends the claim on a drain or a shutdown, yields to an operator
//! emulator control waiting on the instance (superseded: a release and a report line, no failure
//! pair), and ends without action on an instance that already runs. The Start is H-4's start path
//! on the claim's own key; its `command.received` is written only then, so a claim that yields
//! leaves no open command. A passing preparation hands the key to the startup package through the
//! gate, a failing one to the ladder it admitted; emu-watch: under a scheduling pause the startup
//! package's claim waits queued as a gated entry and runs after the resume (#411 A71: so does an
//! operator's start's; W2-B, 0.12.26: `host.idle_state` keeps it, so a restart before the resume
//! queues it again). Every outcome is one report line, which actingd writes as a daemon line; a
//! failed write never stops the daemon.
//!
//! Workflow #386 SR-4a3 (frozen scheduling model v1.4.2 C-3, T46; `scheduling-blackout.md`,
//! "Complete silence"): inside a blackout of its instance the claim gets no key and starts no
//! emulator. The routine gate holds it (`ClaimGate::Silenced`) as a queued entry, whether the
//! daemon start, a watchdog start's daemon start, the emulator watch or IDLE-1's wake asked for
//! it, so an emulator the watch found stopped stays stopped and its restart is counted once
//! (the watch requests no second claim while one waits). Its wait is bounded: the lease sweep's
//! pump grants it at the first tick after the blackout's end, and the startup package follows
//! as after every configured start. A claim granted before a blackout that then covers its
//! instance holds without its Start until the end.

use super::emulator_watch::WatchKind;
use super::host_claims::{HostClaimWork, HostKey};
use super::lease::HostClaim;
use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_host_metrics::NetworkConnectivity;

/// How often a granted claim checks its hold (H-5: the wait ticks every 500 ms).
const AUTOSTART_TICK: Duration = Duration::from_millis(500);
/// How long the claim waits for the operating system to report internet access (P3).
const NETWORK_WAIT_MS: u64 = 600_000;
/// How often the operating system's connectivity status is read during that wait (P3).
const NETWORK_READ_INTERVAL_MS: u64 = 5_000;
/// The key's TTL at its grant and at every renewal while the claim waits (H-5, review M-4).
const AUTOSTART_HOLD_TTL_MS: u64 = 120_000;
/// A waiting claim renews its key whenever less than this is left (H-5, review M-4).
const AUTOSTART_RENEW_BELOW_MS: u64 = 30_000;

/// Workflow #369 S6c: the report lines of the configured emulator starts, as outcomes in the order
/// they happened. actingd takes them and writes each as a daemon line (Workflow #378 A6a); the
/// handle stays valid after the host closed, so the lines of a claim that a shutdown ended are
/// written too.
#[derive(Clone, Default)]
pub struct EmulatorAutostartReports {
    shared: Arc<ReportLines>,
}

#[derive(Default)]
struct ReportLines {
    lines: Mutex<Vec<Outcome>>,
    /// Signalled at every report, for a test that waits for one.
    reported: Condvar,
}

impl EmulatorAutostartReports {
    /// Takes every line reported since the last take.
    pub fn take(&self) -> RuntimeHostResult<Vec<Outcome>> {
        Ok(std::mem::take(&mut *lock(
            &self.shared.lines,
            HostLocation::EmulatorAutostartReports,
        )?))
    }

    fn push(&self, line: Outcome) -> RuntimeHostResult<()> {
        lock(&self.shared.lines, HostLocation::EmulatorAutostartReports)?.push(line);
        self.shared.reported.notify_all();
        Ok(())
    }

    /// Test only: waits at most `wait` until the lines not yet taken satisfy `done`, and returns
    /// them (nothing is taken); `None` when they did not in time.
    #[cfg(test)]
    pub(crate) fn await_lines_for_test(
        &self,
        done: impl Fn(&[Outcome]) -> bool,
        wait: Duration,
    ) -> Option<Vec<Outcome>> {
        let deadline = Instant::now() + wait;
        let mut lines = self.shared.lines.lock().ok()?;
        loop {
            if done(&lines) {
                return Some(lines.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            lines = self.shared.reported.wait_timeout(lines, left).ok()?.0;
        }
    }
}

/// H-5 Order (review M-4): the start turn of one daemon start's autostart claims. The claims that
/// hold their instance's key before their Start are its contenders; at most one holds the turn,
/// and it goes to the contender first in configuration order, so the Starts follow one another
/// in that order as #643's did. A claim that queues instead (behind a pause, an operator, a
/// takeover cooldown) is no contender until it is granted. The turn holds no key and no queue
/// position.
#[derive(Default)]
pub(super) struct AutostartTurn {
    holder: Option<usize>,
    contenders: BTreeSet<usize>,
}

impl AutostartTurn {
    /// The claim at `position` holds its key and contends for the turn.
    fn enter(&mut self, position: usize) {
        self.contenders.insert(position);
    }

    /// The claim at `position`, which holds its key, asks for the turn; `true` once it holds it.
    fn take(&mut self, position: usize) -> bool {
        if self.holder == Some(position) {
            return true;
        }
        self.contenders.insert(position);
        if self.holder.is_none() && self.contenders.first() == Some(&position) {
            self.holder = Some(position);
            return true;
        }
        false
    }

    /// The claim at `position` released, handed off or yielded: it gives the turn back and
    /// contends no longer.
    fn leave(&mut self, position: usize) {
        if self.holder == Some(position) {
            self.holder = None;
        }
        self.contenders.remove(&position);
    }

    /// The claim at `position` was queued at its enqueue instead of granted: it contends no
    /// longer until its worker enters again once it is granted (a grant that came meanwhile
    /// keeps the turn it took).
    fn withdraw(&mut self, position: usize) {
        if self.holder != Some(position) {
            self.contenders.remove(&position);
        }
    }
}

/// One configured instance's autostart claim: the payload its instance's worker runs once the
/// claim is granted.
#[derive(Clone)]
pub(super) struct PendingAutostart {
    pub(super) instance_id: InstanceId,
    pub(super) instance_alias: String,
    /// The instance's place in the configuration, its place in the start turn.
    position: usize,
    /// emu-watch: the emulator watch requested the claim for an emulator it found stopped; the
    /// claim reads the emulator status just before its Start instead of asking whether the
    /// binding is pending.
    watch: bool,
    /// #405 O-2 (0.12.25, review M-1): an idle wake requested the claim (a `watch` claim): when it
    /// finds the emulator running, it still queues the instance's startup package.
    idle_wake: bool,
    /// EMU-BIND (0.12.27): the watch requested the claim (a `watch` claim) to recognize a running
    /// emulator whose endpoint the Runtime holds pending or bound to another port: it takes no
    /// network wait and no start turn (`position` is unused) and never launches or stops the
    /// emulator (`drive_recognition`).
    recognize: bool,
}

/// Binds the configured aliases to registered physical instances at startup, in configuration
/// order. An alias that is not registered fails the start with
/// `emulator_autostart_instance_unknown`, a fixture instance with
/// `emulator_autostart_requires_physical_instance`.
pub(super) fn resolve_emulator_autostart(
    configured: &[String],
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
) -> RuntimeHostResult<Vec<(InstanceId, String)>> {
    configured
        .iter()
        .map(|alias| {
            let refused = |code: HostCode| {
                RuntimeHostError::fatal(
                    code,
                    HostLocation::StartRuntimeHost,
                    RuntimeErrorCode::RuntimeHalted,
                )
                .with_outcome_values(|outcome| outcome.alias(alias.as_str()))
            };
            let instance = instances
                .values()
                .find(|instance| instance.instance_alias == *alias)
                .ok_or_else(|| refused(HostCode::EmulatorAutostartInstanceUnknown))?;
            if instance.provenance() != ExecutionBackendProvenance::PhysicalDevice {
                return Err(refused(HostCode::EmulatorAutostartRequiresPhysicalInstance));
            }
            Ok((instance.instance_id(), alias.clone()))
        })
        .collect()
}

/// How a granted claim's hold ends before its Start.
enum HoldEnd {
    /// A drain or a shutdown.
    Interrupted,
    /// An operator emulator control waits on the instance.
    Superseded,
    /// The instance already runs.
    Running,
    /// emu-watch: the watch's claim could not read the emulator status before its Start: the
    /// read's failure.
    Unread(Box<Outcome>),
    /// A renewal was refused (`lease_expired`, `lease_missing`).
    RenewalRefused(Box<RequestFailure>),
}

/// emu-watch: what the watch claim's status read just before its Start found.
enum WatchRead {
    /// No read (a daemon start's claim), or the emulator is stopped: the Start goes on.
    Start,
    /// The hold ends before the Start.
    End(HoldEnd),
    /// EMU-BIND (0.12.27): the emulator runs with Android and an ADB port while the Runtime holds
    /// the endpoint pending or bound to another port: it is recognized instead of answered
    /// skipped (a watch claim's or an idle wake's).
    Recognize,
}

/// The network wait (P3).
enum NetworkWait {
    /// The next read is due at this monotonic time.
    Reading { next_read_ms: u64 },
    /// Internet access, or an unknown status: the claim waits for its turn.
    Ready,
}

impl HostShared {
    /// Workflow #369 S6c (H-5): enqueues one autostart claim per configured instance whose
    /// endpoint is pending, in configuration order; it does not block. The configuration's
    /// count is reported first (`emulator_autostart_queued`); nothing is reported when no
    /// instance names `start_emulator`. A claim contends for the start turn from before its
    /// request, so a claim granted at once never lets a later one start first; one that waits
    /// queued contends once it is granted. A claim the queue refuses is recorded by the queue and
    /// reported; only a failure the Runtime does not survive is returned.
    pub(super) fn enqueue_emulator_autostart(&self) -> RuntimeHostResult<usize> {
        let configured = self.emulator_autostart()?.to_vec();
        if configured.is_empty() {
            return Ok(0);
        }
        // IDLE-1P (#405): an emulator the idle action holds stopped (restored at this start)
        // waits for its wake, which starts it through this same configured start.
        let mut held_stopped = BTreeSet::new();
        for (_, instance_alias) in &configured {
            if self.idle_holds_emulator_stopped(instance_alias)? {
                held_stopped.insert(instance_alias.clone());
            }
        }
        let pending = {
            let registry = lock(
                &self.registered_instances,
                HostLocation::EnqueueEmulatorAutostart,
            )?;
            configured
                .into_iter()
                .enumerate()
                .filter(|(_, (instance_id, instance_alias))| {
                    registry
                        .get(instance_id)
                        .is_some_and(RegisteredInstance::endpoint_pending)
                        && !held_stopped.contains(instance_alias)
                })
                .map(
                    |(position, (instance_id, instance_alias))| PendingAutostart {
                        instance_id,
                        instance_alias,
                        position,
                        watch: false,
                        idle_wake: false,
                        recognize: false,
                    },
                )
                .collect::<Vec<_>>()
        };
        self.report_emulator_autostart(
            Outcome::new(HostCode::EmulatorAutostartQueued).count(pending.len()),
        )?;
        for claim in &pending {
            self.autostart_turn(|turn| turn.enter(claim.position))?;
            match self.request_autostart_claim(claim) {
                Ok(true) => {}
                Ok(false) => self.autostart_turn(|turn| turn.withdraw(claim.position))?,
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(failure) => {
                    self.autostart_turn(|turn| turn.withdraw(claim.position))?;
                    self.report_emulator_autostart(
                        Outcome::new(HostCode::EmulatorAutostartFailed)
                            .alias(claim.instance_alias.as_str())
                            .caused_by(failure.error.outcome()),
                    )?;
                }
            }
        }
        Ok(pending.len())
    }

    /// The report handle actingd takes the lines from.
    pub(super) fn emulator_autostart_reports(&self) -> EmulatorAutostartReports {
        self.emulator_autostart_reports.clone()
    }

    /// One report line (emu-watch: the watch's lines too).
    pub(super) fn report_emulator_autostart(&self, line: Outcome) -> RuntimeHostResult<()> {
        self.emulator_autostart_reports.push(line)
    }

    /// emu-watch (R3): one autostart claim, marked `watch`, for the configured instance at
    /// `position` whose emulator the watch found stopped; it does not block. It contends for the
    /// start turn from before its request, as a daemon start's claim does. A claim the queue
    /// refuses is recorded by the queue and reported (`emulator_autostart_failed`); only a failure
    /// the Runtime does not survive is returned. #405 O-2 (review M-1): `idle_wake`, an idle wake
    /// requests it (the claim then queues the startup package even when it finds the emulator
    /// running).
    pub(super) fn request_watch_autostart(
        &self,
        position: usize,
        instance_id: InstanceId,
        instance_alias: &str,
        idle_wake: bool,
    ) -> RuntimeHostResult<()> {
        let claim = PendingAutostart {
            instance_id,
            instance_alias: instance_alias.to_owned(),
            position,
            watch: true,
            idle_wake,
            recognize: false,
        };
        self.autostart_turn(|turn| turn.enter(claim.position))?;
        match self.request_autostart_claim(&claim) {
            Ok(true) => Ok(()),
            Ok(false) => self.autostart_turn(|turn| turn.withdraw(claim.position)),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => {
                self.autostart_turn(|turn| turn.withdraw(claim.position))?;
                self.report_emulator_autostart(
                    Outcome::new(HostCode::EmulatorAutostartFailed)
                        .alias(instance_alias)
                        .caused_by(failure.error.outcome()),
                )
            }
        }
    }

    /// EMU-BIND (0.12.27): one autostart claim, marked `watch` and `recognize`, for the instance
    /// whose emulator the watch found running while the Runtime holds its endpoint pending or
    /// bound to another port; it does not block and takes no start turn. `position` is the
    /// instance's place in the configuration, if it names `start_emulator` (unused by the claim).
    /// A claim the queue refuses is recorded by the queue and reported
    /// (`emulator_autostart_failed`); only a failure the Runtime does not survive is returned.
    pub(super) fn request_watch_recognition(
        &self,
        position: Option<usize>,
        instance_id: InstanceId,
        instance_alias: &str,
    ) -> RuntimeHostResult<()> {
        let claim = PendingAutostart {
            instance_id,
            instance_alias: instance_alias.to_owned(),
            position: position.unwrap_or(usize::MAX),
            watch: true,
            idle_wake: false,
            recognize: true,
        };
        match self.request_autostart_claim(&claim) {
            Ok(_) => Ok(()),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                Err(*failure.error)
            }
            Err(failure) => self.report_emulator_autostart(
                Outcome::new(HostCode::EmulatorAutostartFailed)
                    .alias(instance_alias)
                    .caused_by(failure.error.outcome()),
            ),
        }
    }

    fn autostart_turn<T>(
        &self,
        change: impl FnOnce(&mut AutostartTurn) -> T,
    ) -> RuntimeHostResult<T> {
        let mut turn = lock(
            &self.emulator_autostart_turn,
            HostLocation::RunEmulatorAutostart,
        )?;
        Ok(change(&mut turn))
    }

    /// One autostart claim (routine, no deadline, never preempting) with the hold's TTL, under a
    /// fresh request, correlation and holder and the host's synthesized connection; its payload
    /// is registered before the claim can be granted (Q-6). `true` when it holds the key now.
    fn request_autostart_claim(&self, pending: &PendingAutostart) -> Result<bool, RequestFailure> {
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    HostLocation::EnqueueEmulatorAutostart,
                    &error,
                ))
            })?;
        let issuer = self.events.issuer();
        let identifier = || RequestFailure::poison_without_terminal(runtime_identifier_error());
        let holder_id = *issuer
            .mint_holder_id()
            .map_err(|_| identifier())?
            .transport();
        let (actor, source) = scheduled_request_transport_origin(resolved.provenance());
        let request = RuntimeRequest::new(
            issuer.mint_request_id().map_err(|_| identifier())?,
            issuer.mint_correlation_id().map_err(|_| identifier())?,
            None,
            actor,
            source,
            unix_ms_now().map_err(RequestFailure::poison_without_terminal)?,
            RuntimeOperation::AcquireLease {
                instance_alias: pending.instance_alias.clone(),
                holder_id,
            },
        )
        .map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::EnqueueEmulatorAutostart,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: &pending.instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::Autostart,
                priority: actingcommand_contract::LeasePriority::Normal,
                lease_ttl_ms: AUTOSTART_HOLD_TTL_MS,
            },
            HostClaimWork::Autostart(pending.clone()),
        )?;
        Ok(
            lock(&self.scheduler, HostLocation::EnqueueEmulatorAutostart)?
                .active_lease(pending.instance_id)
                .is_some_and(|lease| lease.token().holder_id() == holder_id),
        )
    }

    /// W-2: a granted autostart claim on its instance's worker. It contends for the start turn
    /// while it holds its key, and leaves it however it ends. Only a failure the Runtime does not
    /// survive is returned.
    pub(super) fn run_autostart_claim(
        &self,
        pending: &PendingAutostart,
        key: HostKey,
    ) -> RuntimeHostResult<()> {
        // EMU-BIND: a recognition launches nothing, so it takes no start turn.
        if pending.recognize {
            return self.hold_recognition_claim(pending, key);
        }
        let held = self
            .autostart_turn(|turn| turn.enter(pending.position))
            .and_then(|()| self.hold_autostart_claim(pending, key));
        let turn = self.autostart_turn(|turn| turn.leave(pending.position));
        held.and(turn)
    }

    /// H-5: the hold from the grant to the Start, or to the end before it.
    fn hold_autostart_claim(
        &self,
        pending: &PendingAutostart,
        mut key: HostKey,
    ) -> RuntimeHostResult<()> {
        let granted_ms = self.monotonic_ms()?;
        let mut network = NetworkWait::Reading {
            next_read_ms: granted_ms,
        };
        loop {
            if let Some(end) = self.autostart_hold_ends(pending, &mut key)? {
                return self.end_autostart_hold(pending, &key, end);
            }
            if let NetworkWait::Reading { next_read_ms } = network {
                let now = self.monotonic_ms()?;
                if now >= next_read_ms {
                    let waited_ms = now.saturating_sub(granted_ms);
                    network = match self.read_network_connectivity() {
                        NetworkConnectivity::Internet => {
                            self.report_emulator_autostart(
                                Outcome::new(HostCode::EmulatorAutostartNetworkReady)
                                    .alias(pending.instance_alias.as_str())
                                    .elapsed_ms(waited_ms),
                            )?;
                            NetworkWait::Ready
                        }
                        NetworkConnectivity::Unknown { reason, os_error } => {
                            let line =
                                Outcome::new(HostCode::EmulatorAutostartNetworkStatusUnknown)
                                    .alias(pending.instance_alias.as_str())
                                    .connectivity_reason(reason);
                            self.report_emulator_autostart(match os_error {
                                Some(os_error) => line.os_error(os_error),
                                None => line,
                            })?;
                            NetworkWait::Ready
                        }
                        NetworkConnectivity::NoInternet if waited_ms >= NETWORK_WAIT_MS => {
                            return self.refuse_autostart_network(pending, &key, waited_ms);
                        }
                        NetworkConnectivity::NoInternet => NetworkWait::Reading {
                            next_read_ms: now.saturating_add(NETWORK_READ_INTERVAL_MS),
                        },
                    };
                }
            }
            // Workflow #386 SR-4a3: a claim granted before a blackout that now covers its
            // instance does not start the emulator inside it, and leaves the turn to the others:
            // it holds on, renewing its key, until the blackout's end.
            if matches!(network, NetworkWait::Ready)
                && self
                    .blackout_covering(pending.instance_id, &pending.instance_alias)?
                    .is_none()
                && self.autostart_turn(|turn| turn.take(pending.position))?
            {
                // The final checks just before the Start; emu-watch: the watch's claim reads the
                // emulator status once more.
                if let Some(end) = self.autostart_hold_ends(pending, &mut key)? {
                    return self.end_autostart_hold(pending, &key, end);
                }
                return match self.watch_start_unneeded(pending)? {
                    WatchRead::Start => self.drive_autostart(pending, key),
                    WatchRead::End(end) => self.end_autostart_hold(pending, &key, end),
                    WatchRead::Recognize => self.drive_recognition(pending, key),
                };
            }
            thread::sleep(AUTOSTART_TICK);
        }
    }

    /// EMU-BIND (0.12.27): the hold of the watch's recognition claim, from the grant to the
    /// recognition: the checks of every tick (a drain or a shutdown, the renewal, an operator
    /// emulator control waiting on the instance), and inside a blackout of the instance it holds
    /// on, renewing its key, until the blackout's end (Workflow #386 SR-4a3). No network wait and
    /// no start turn: it launches nothing. A scheduling pause does not hold it.
    fn hold_recognition_claim(
        &self,
        pending: &PendingAutostart,
        mut key: HostKey,
    ) -> RuntimeHostResult<()> {
        loop {
            if let Some(end) = self.autostart_hold_ends(pending, &mut key)? {
                return self.end_autostart_hold(pending, &key, end);
            }
            if self
                .blackout_covering(pending.instance_id, &pending.instance_alias)?
                .is_none()
            {
                return self.drive_recognition(pending, key);
            }
            thread::sleep(AUTOSTART_TICK);
        }
    }

    /// The checks of every tick, in order: a drain or a shutdown; the renewal; an operator
    /// emulator control waiting on the instance; an instance that already runs (a daemon start's
    /// claim: its binding is no longer pending). emu-watch: a scheduling pause is no check: it
    /// never holds the emulator start.
    fn autostart_hold_ends(
        &self,
        pending: &PendingAutostart,
        key: &mut HostKey,
    ) -> RuntimeHostResult<Option<HoldEnd>> {
        if self.fatal.is_shutdown_requested() || self.lifecycle_draining()? {
            return Ok(Some(HoldEnd::Interrupted));
        }
        let left_ms = key
            .token
            .expires_at_monotonic_ms()
            .saturating_sub(self.monotonic_ms()?);
        if left_ms < AUTOSTART_RENEW_BELOW_MS
            && let Err(refused) = self.renew_host_key(key, AUTOSTART_HOLD_TTL_MS)
        {
            if refused.poison_runtime || refused.error.is_fatal() {
                return Err(*refused.error);
            }
            return Ok(Some(HoldEnd::RenewalRefused(Box::new(refused))));
        }
        let superseded = !lock(&self.scheduler, HostLocation::RunEmulatorAutostart)?
            .queued_of_kind(pending.instance_id, ClaimKind::EmulatorControl)
            .is_empty();
        if superseded {
            return Ok(Some(HoldEnd::Superseded));
        }
        if pending.watch {
            return Ok(None);
        }
        let running = lock(
            &self.registered_instances,
            HostLocation::RunEmulatorAutostart,
        )?
        .get(&pending.instance_id)
        .is_some_and(|instance| !instance.endpoint_pending());
        Ok(running.then_some(HoldEnd::Running))
    }

    /// Ends a hold before its Start: the key is released, and one report line says why. A
    /// refused renewal is the start's refusal pair (EMU-BIND: a recognition's, which drives no
    /// Start, is one connection-preparation `runtime.failed` record). #405 O-2 (review M-1): an
    /// idle wake's claim that found the emulator running first queues the instance's startup
    /// package (`queue_idle_wake_startup_package`); a failure to queue it is the line's cause.
    fn end_autostart_hold(
        &self,
        pending: &PendingAutostart,
        key: &HostKey,
        end: HoldEnd,
    ) -> RuntimeHostResult<()> {
        let alias = pending.instance_alias.as_str();
        let line = match end {
            HoldEnd::Interrupted => {
                Outcome::new(HostCode::EmulatorAutostartInterrupted).alias(alias)
            }
            HoldEnd::Superseded => Outcome::new(HostCode::EmulatorAutostartSuperseded).alias(alias),
            HoldEnd::Running if pending.idle_wake => {
                match self.queue_idle_wake_startup_package(pending, key) {
                    Ok(()) => Outcome::new(HostCode::EmulatorAutostartSkipped).alias(alias),
                    Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                        return Err(*failure.error);
                    }
                    Err(failure) => Outcome::new(HostCode::EmulatorAutostartFailed)
                        .alias(alias)
                        .caused_by(failure.error.outcome()),
                }
            }
            HoldEnd::Running => Outcome::new(HostCode::EmulatorAutostartSkipped).alias(alias),
            HoldEnd::Unread(cause) => Outcome::new(HostCode::EmulatorAutostartFailed)
                .alias(alias)
                .caused_by(*cause),
            HoldEnd::RenewalRefused(refused) => {
                let code = refused.error.projection().code;
                let mut error =
                    RuntimeHostError::request(code, HostLocation::RunEmulatorAutostart, code);
                error.lifecycle.instance_id = Some(pending.instance_id);
                // EMU-BIND: a recognition drives no Start, so it records no start intent.
                let cause = if pending.recognize {
                    self.append_lifecycle_failure(
                        RuntimeLifecycleFailureStage::ConnectionPreparation,
                        RuntimeLifecycleFailure::Host(&error),
                        self.autostart_links(pending, key)?,
                        None,
                    )?;
                    error.outcome()
                } else {
                    self.refuse_autostart(pending, key, error)?.error.outcome()
                };
                Outcome::new(HostCode::EmulatorAutostartFailed)
                    .alias(alias)
                    .caused_by(cause)
            }
        };
        self.release_host_key(key)?;
        self.report_emulator_autostart(line)
    }

    /// #405 O-2 (0.12.25, review M-1): an idle wake's claim that found the emulator already
    /// running (a restart came while the woken instance's startup package waited in memory, or a
    /// person started the emulator meanwhile) queues the instance's startup package once, through
    /// the path a wake's own startup package takes: its scheduling intent under the claim's links
    /// (`prepare_startup_package`), then its claim (`schedule_startup_package`, which cancels a
    /// still-queued one), queued while this claim still holds the key, so the instance stays held
    /// until the package has run; behind a pause or an install hold it waits queued as every
    /// startup claim does. An instance without a startup package queues nothing.
    fn queue_idle_wake_startup_package(
        &self,
        pending: &PendingAutostart,
        key: &HostKey,
    ) -> Result<(), RequestFailure> {
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        let links = self.autostart_links(pending, key)?;
        let package = self.prepare_startup_package(&resolved, links, key.request.request_id())?;
        self.schedule_startup_package(package).map(|_| ())
    }

    /// P3: no internet access was reported in time. The start is refused with the existing
    /// failure pair (`emulator_autostart_network_unready`, with the wait), the key is released,
    /// and the refusal is the report line.
    fn refuse_autostart_network(
        &self,
        pending: &PendingAutostart,
        key: &HostKey,
        waited_ms: u64,
    ) -> RuntimeHostResult<()> {
        let alias = pending.instance_alias.as_str();
        let mut error = RuntimeHostError::request(
            HostCode::EmulatorAutostartNetworkUnready,
            HostLocation::RunEmulatorAutostart,
            RuntimeErrorCode::BackendOperationFailed,
        )
        .with_outcome_values(|outcome| outcome.alias(alias).elapsed_ms(waited_ms));
        error.lifecycle.instance_id = Some(pending.instance_id);
        let failure = self.refuse_autostart(pending, key, error)?;
        self.release_host_key(key)?;
        self.report_emulator_autostart(failure.error.outcome())
    }

    /// The start's intent and the existing refusal pair under the claim's links; returns the
    /// recorded failure. Only a failure the Runtime does not survive is the error.
    fn refuse_autostart(
        &self,
        pending: &PendingAutostart,
        key: &HostKey,
        error: RuntimeHostError,
    ) -> RuntimeHostResult<RequestFailure> {
        let links = self.autostart_links(pending, key)?;
        let failure = self.refuse_configured_start(links, error);
        if failure.poison_runtime || failure.error.is_fatal() {
            return Err(*failure.error);
        }
        Ok(failure)
    }

    /// The claim's request links on its instance.
    fn autostart_links(
        &self,
        pending: &PendingAutostart,
        key: &HostKey,
    ) -> RuntimeHostResult<EventLinksDraft> {
        let validated = key.request.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RunEmulatorAutostart,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        Ok(self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None))
    }

    /// H-5 Start path: H-4's start on the claim's own key (`drive_configured_start`), then one
    /// report line: started when the backend self-check after the start passes, the preparation
    /// failure when it does not (the ladder it admitted holds the key), or the failure that ended
    /// the start.
    fn drive_autostart(&self, pending: &PendingAutostart, key: HostKey) -> RuntimeHostResult<()> {
        let alias = pending.instance_alias.as_str();
        let claim = key.request.clone();
        let validated = claim.validate().map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RunEmulatorAutostart,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
        let resolved = match self.resolve_instance(alias) {
            Ok(resolved) => resolved,
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(*failure.error);
            }
            Err(failure) => {
                self.release_host_key(&key)?;
                return self.report_emulator_autostart(
                    Outcome::new(HostCode::EmulatorAutostartFailed)
                        .alias(alias)
                        .caused_by(failure.error.outcome()),
                );
            }
        };
        let line = match self.drive_configured_start(&validated, &resolved, links, key) {
            Ok(()) if self.backend_selfcheck_passed(pending.instance_id)? => {
                Outcome::new(HostCode::EmulatorAutostartStarted).alias(alias)
            }
            Ok(()) => Outcome::new(HostCode::EmulatorAutostartPreparationFailed).alias(alias),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(*failure.error);
            }
            Err(failure) => Outcome::new(HostCode::EmulatorAutostartFailed)
                .alias(alias)
                .caused_by(failure.error.outcome()),
        };
        self.report_emulator_autostart(line)
    }

    /// emu-watch: before the Start of the watch's claim, the emulator status read once
    /// (`MuMuManager info`, bounded; it never starts anything): `Running` when the emulator
    /// process runs (a person or an operator control started it meanwhile), `Unread` when the
    /// read fails, `Start` when the emulator is stopped. A daemon start's claim reads nothing
    /// here. EMU-BIND (0.12.27): `Recognize` instead of `Running` when the emulator runs with
    /// Android and an ADB port while the Runtime holds the endpoint pending or bound to another
    /// port (the gap a skipped watch or idle-wake claim left open).
    fn watch_start_unneeded(&self, pending: &PendingAutostart) -> RuntimeHostResult<WatchRead> {
        if !pending.watch {
            return Ok(WatchRead::Start);
        }
        match self
            .execution()?
            .read_emulator_instance_state(&pending.instance_alias)
        {
            Ok(state) if self.needs_recognition(pending.instance_id, &state)? => {
                Ok(WatchRead::Recognize)
            }
            Ok(state) if state.process_started => Ok(WatchRead::End(HoldEnd::Running)),
            Ok(_) => Ok(WatchRead::Start),
            Err(error) => Ok(WatchRead::End(HoldEnd::Unread(Box::new(
                self.emulator_watch_read_failure(
                    &pending.instance_alias,
                    HostLocation::RunEmulatorAutostart,
                    error.to_string(),
                ),
            )))),
        }
    }

    /// EMU-BIND (0.12.27): the recognition of a running emulator on the claim's own `key`, which
    /// never launches or stops it: the key renewed to the hold's TTL (a refusal ends the hold as
    /// a recognition's), then `recognize_on_key`, then one report line:
    /// `emulator_autostart_skipped` (`watch_kind` `recognize`) when the self-check passed,
    /// `emulator_autostart_preparation_failed` (`watch_kind` `recognize`) when it did not (the
    /// preparation recorded the failing step on this instance; a ladder it admitted holds the
    /// key), or `emulator_autostart_failed` with the failure that ended it. Only a failure the
    /// Runtime does not survive is returned.
    fn drive_recognition(
        &self,
        pending: &PendingAutostart,
        mut key: HostKey,
    ) -> RuntimeHostResult<()> {
        let alias = pending.instance_alias.as_str();
        if let Err(refused) = self.renew_host_key(&mut key, AUTOSTART_HOLD_TTL_MS) {
            if refused.poison_runtime || refused.error.is_fatal() {
                return Err(*refused.error);
            }
            return self.end_autostart_hold(
                pending,
                &key,
                HoldEnd::RenewalRefused(Box::new(refused)),
            );
        }
        let line = match self.recognize_on_key(pending, key) {
            Ok(()) if self.backend_selfcheck_passed(pending.instance_id)? => {
                Outcome::new(HostCode::EmulatorAutostartSkipped)
                    .alias(alias)
                    .watch_kind(WatchKind::Recognize)
            }
            Ok(()) => Outcome::new(HostCode::EmulatorAutostartPreparationFailed)
                .alias(alias)
                .watch_kind(WatchKind::Recognize),
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(*failure.error);
            }
            Err(failure) => Outcome::new(HostCode::EmulatorAutostartFailed)
                .alias(alias)
                .caused_by(failure.error.outcome()),
        };
        self.report_emulator_autostart(line)
    }

    /// EMU-BIND: on the claim's `key`, the preparation of a self-check (#411 A48,
    /// `prepare_recognized_instance`: the emulator status read, the reported port bound with
    /// `runtime.instance_bound`, the ADB baseline, `device.connected`, the opens), then the hold's
    /// end as after a start (`end_emulator_control_hold`): once the endpoint is bound, the
    /// instance's startup package is queued and takes the key through the gate, or waits behind
    /// the ladder a failed preparation admitted; an endpoint still pending queues none and the key
    /// goes to that ladder or is released. The error is a failure that ended the key.
    fn recognize_on_key(
        &self,
        pending: &PendingAutostart,
        key: HostKey,
    ) -> Result<(), RequestFailure> {
        let validated = key.request.validate().map_err(|_| {
            RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                HostCode::HostKeyRequestInvalid,
                HostLocation::RunEmulatorAutostart,
                RuntimeErrorCode::RuntimeHalted,
            ))
        })?;
        let links = self
            .events
            .request_links(&validated, Some(pending.instance_id), None, None);
        let prepared = self
            .prepare_recognized_instance(
                &pending.instance_alias,
                pending.instance_id,
                links.clone(),
                &key,
            )
            .and_then(|(_, ladder)| {
                let resolved = self.resolve_instance(&pending.instance_alias)?;
                let startup_package = if resolved.endpoint_pending() {
                    None
                } else {
                    self.prepare_startup_package(&resolved, links, key.request.request_id())?
                };
                Ok((startup_package, ladder))
            });
        let ended = prepared.and_then(|(startup_package, ladder)| {
            self.end_emulator_control_hold(&validated, &key, startup_package, ladder)
        });
        match ended {
            Ok(_) => Ok(()),
            Err(failure) => {
                Err(self.cleanup_composite_failure(key.token, key.connection_id, failure))
            }
        }
    }

    /// P3: the operating system's connectivity status, read once (a test build may script it).
    fn read_network_connectivity(&self) -> NetworkConnectivity {
        #[cfg(test)]
        if let Some(read) = &self.network_connectivity_for_test {
            return read();
        }
        actingcommand_host_metrics::network_connectivity()
    }
}
