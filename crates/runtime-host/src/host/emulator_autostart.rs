// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #369 S6c (model-369-queue v3.1 H-5, §7 row 8): the configured emulator start at
//! daemon start, rebuilt from #643 as one claim per instance in the instance's one queue.
//!
//! An instance configured with `start_emulator` (S6a; off unless the configuration names it, and
//! allowed only on a discovery-bound instance) whose emulator is stopped when the daemon starts
//! (its endpoint is pending) gets one autostart claim (`ClaimKind::Autostart`, a routine claim
//! with no deadline that never preempts). actingd enqueues the claims once, after policy
//! initialization and before `runtime_ready`; the enqueue does not block. Every claim is visible:
//! a holder from the enqueue, or a gated entry while a scheduling pause holds the instance or the
//! global gate (a restored pause included). A gated entry is no waiting work (Workflow #411 A3):
//! it refuses neither `request-shutdown` nor `restart`, an accepted shutdown disconnects it
//! (`lease.queue_disconnected`) with no report line, and the next start enqueues it again under
//! the restored pause. The claim takes neither the bound-endpoint check nor the
//! performance-control gate and is granted with drain capacity (Q-2a).
//!
//! Once granted, the claim runs on its instance's worker and holds the key throughout. It reads
//! the operating system's connectivity status (`actingcommand_host_metrics`, P3; the Runtime
//! opens no connection and resolves no name) at once and every 5 s, for at most 600 s: internet
//! access goes on, an unknown status goes on at once with one warning line, and no internet in
//! time refuses the start with `emulator_autostart_network_unready`. It then waits for the start
//! turn, which the claims of one daemon start take one at a time in configuration order. The
//! wait ticks every 500 ms; at each tick it renews the key to 120 s whenever less than 30 s is
//! left (B1, review M-4), and ends the claim on a drain or a shutdown, yields to a pause (it
//! queues again as a gated entry and releases), yields to an operator emulator control waiting
//! on the instance (superseded: a release and a report line, no failure pair), and ends without
//! action on an instance that already runs. The Start is H-4's start path on the claim's own
//! key; its `command.received` is written only then, so a claim that yields leaves no open
//! command. A passing preparation hands the key to the startup package through the gate, a
//! failing one to the ladder it admitted. Every outcome is one report line, which actingd writes
//! as a daemon line; a failed write never stops the daemon.

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
    /// A scheduling pause holds the instance or the global gate: the claim queues again.
    Yielded,
    /// An operator emulator control waits on the instance.
    Superseded,
    /// The instance already runs.
    Running,
    /// A renewal was refused (`lease_expired`, `lease_missing`).
    RenewalRefused(Box<RequestFailure>),
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
        let pending = {
            let registry = lock(
                &self.registered_instances,
                HostLocation::EnqueueEmulatorAutostart,
            )?;
            configured
                .into_iter()
                .enumerate()
                .filter(|(_, (instance_id, _))| {
                    registry
                        .get(instance_id)
                        .is_some_and(RegisteredInstance::endpoint_pending)
                })
                .map(
                    |(position, (instance_id, instance_alias))| PendingAutostart {
                        instance_id,
                        instance_alias,
                        position,
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

    fn report_emulator_autostart(&self, line: Outcome) -> RuntimeHostResult<()> {
        self.emulator_autostart_reports.push(line)
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
            if matches!(network, NetworkWait::Ready)
                && self.autostart_turn(|turn| turn.take(pending.position))?
            {
                // The final checks just before the Start (P3: the pause is checked again right
                // before the emulator is driven).
                if let Some(end) = self.autostart_hold_ends(pending, &mut key)? {
                    return self.end_autostart_hold(pending, &key, end);
                }
                return self.drive_autostart(pending, key);
            }
            thread::sleep(AUTOSTART_TICK);
        }
    }

    /// The checks of every tick, in order: a drain or a shutdown; the renewal; an operator
    /// emulator control waiting on the instance; a pause; an instance that already runs.
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
        if self.routine_gate(pending.instance_id)? == ClaimGate::RoutineHeld {
            return Ok(Some(HoldEnd::Yielded));
        }
        let running = lock(
            &self.registered_instances,
            HostLocation::RunEmulatorAutostart,
        )?
        .get(&pending.instance_id)
        .is_some_and(|instance| !instance.endpoint_pending());
        Ok(running.then_some(HoldEnd::Running))
    }

    /// Ends a hold before its Start: the key is released (after the claim queued again, for a
    /// pause), and one report line says why. A refused renewal is the start's refusal pair.
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
            HoldEnd::Running => Outcome::new(HostCode::EmulatorAutostartSkipped).alias(alias),
            HoldEnd::Yielded => match self.request_autostart_claim(pending) {
                Ok(_) => Outcome::new(HostCode::EmulatorAutostartYielded).alias(alias),
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(failure) => Outcome::new(HostCode::EmulatorAutostartFailed)
                    .alias(alias)
                    .caused_by(failure.error.outcome()),
            },
            HoldEnd::RenewalRefused(refused) => {
                let code = refused.error.projection().code;
                let mut error =
                    RuntimeHostError::request(code, HostLocation::RunEmulatorAutostart, code);
                error.lifecycle.instance_id = Some(pending.instance_id);
                let failure = self.refuse_autostart(pending, key, error)?;
                Outcome::new(HostCode::EmulatorAutostartFailed)
                    .alias(alias)
                    .caused_by(failure.error.outcome())
            }
        };
        self.release_host_key(key)?;
        self.report_emulator_autostart(line)
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

    /// P3: the operating system's connectivity status, read once (a test build may script it).
    fn read_network_connectivity(&self) -> NetworkConnectivity {
        #[cfg(test)]
        if let Some(read) = &self.network_connectivity_for_test {
            return read();
        }
        actingcommand_host_metrics::network_connectivity()
    }
}
