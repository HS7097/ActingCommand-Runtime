// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #361 B2: configured emulator starts at daemon start.
//!
//! An instance configured with `start_emulator` (discovery-bound instances only) whose emulator
//! is stopped when the daemon starts (its ADB endpoint is pending) is started by the Runtime
//! itself, through the path the stuck-recovery ladder's restart rung uses: a Runtime-origin
//! `command.received`, then `drive_emulator_control(Start)` (launch, binding, ADB baseline,
//! connection preparation with its self-check), then its startup package is scheduled as a
//! manual start schedules it. A running (bound) instance only gets today's start preparation.
//!
//! The daemon queues the starts once, after policy initialization succeeded, as host work on
//! the startup thread, so they never delay runtime-info publication (review H2). Before the
//! first start the thread waits, bounded, until the network answers. Instances start one after
//! another in configuration order. A start is skipped while a scheduling pause holds the
//! global gate or the instance's gate (review M4) and queued again by the resume that lifts
//! it. Every refusal is the existing `command.rejected` + `runtime.failed` pair and a report
//! line for the daemon's stdout; none stops the daemon.

use super::*;
use actingcommand_contract::EmulatorInstanceAction;
use std::net::{TcpStream, ToSocketAddrs};

const OPERATION: &str = "autostart_emulator";
/// The endpoint whose TCP answer counts as network reachability: the operating system's own
/// connectivity check host, contacted with a bare connection and no request.
const NETWORK_PROBE: &str = "www.msftconnecttest.com:80";
const NETWORK_PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the first start waits for the network before every pending start is refused.
const NETWORK_WAIT: Duration = Duration::from_secs(600);
const NETWORK_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Review L2: a start inside the takeover cooldown waits for its end instead of being refused.
const TAKEOVER_COOLDOWN_WAIT: Duration = Duration::from_secs(60);
const TAKEOVER_COOLDOWN_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The configured starts of one queue entry, in configuration order.
pub(super) struct PendingEmulatorAutostart {
    instances: Vec<InstanceId>,
}

/// Binds the configured aliases to registered physical instances at startup, in order. An
/// alias that is not registered fails with `emulator_autostart_instance_unknown`; a fixture
/// instance with `emulator_autostart_requires_physical_instance`.
pub(super) fn resolve_emulator_autostart(
    configured: &[String],
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
) -> RuntimeHostResult<Vec<InstanceId>> {
    configured
        .iter()
        .map(|alias| {
            let instance = instances
                .values()
                .find(|instance| instance.instance_alias == *alias)
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "emulator_autostart_instance_unknown",
                        "start_runtime_host",
                        RuntimeErrorCode::RuntimeFatal,
                    )
                    .with_native_detail(format!("instance_alias={alias}"))
                })?;
            if instance.provenance() != ExecutionBackendProvenance::PhysicalDevice {
                return Err(RuntimeHostError::fatal(
                    "emulator_autostart_requires_physical_instance",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_native_detail(format!("instance_alias={alias}")));
            }
            Ok(instance.instance_id())
        })
        .collect()
}

fn network_answers() -> bool {
    NETWORK_PROBE.to_socket_addrs().is_ok_and(|mut addresses| {
        addresses.any(|address| {
            TcpStream::connect_timeout(&address, NETWORK_PROBE_CONNECT_TIMEOUT).is_ok()
        })
    })
}

impl HostShared {
    fn emulator_autostart(&self) -> RuntimeHostResult<&[InstanceId]> {
        self.emulator_autostart
            .get()
            .map(Vec::as_slice)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "runtime_component_not_prepared",
                    "access_prepared_runtime",
                    RuntimeErrorCode::RuntimeFatal,
                )
            })
    }

    /// Queues the configured starts once; the daemon calls this after policy initialization.
    /// Returns how many instances were queued.
    pub(super) fn queue_emulator_autostart(&self) -> RuntimeHostResult<usize> {
        let instances = self.emulator_autostart()?.to_vec();
        let queued = instances.len();
        if queued > 0 {
            lock(&self.pending_host_work, "queue_emulator_autostart")?.push_back(
                startup_package::PendingHostWork::EmulatorAutostart(PendingEmulatorAutostart {
                    instances,
                }),
            );
        }
        Ok(queued)
    }

    /// Queues the starts a scheduling pause deferred and no pause holds any more; a resume
    /// calls this after it lifted its gate.
    pub(super) fn requeue_deferred_emulator_autostart(&self) -> RuntimeHostResult<()> {
        let pauses = lock(&self.scheduling_pause, "read_autostart_pauses")?.clone();
        let aliases = lock(&self.registered_instances, "read_autostart_instances")?
            .values()
            .map(|instance| (instance.instance_id, instance.instance_alias.clone()))
            .collect::<BTreeMap<_, _>>();
        let ready = {
            let mut deferred = lock(&self.emulator_autostart_deferred, "requeue_autostart")?;
            let ready = deferred
                .iter()
                .copied()
                .filter(|instance_id| {
                    aliases
                        .get(instance_id)
                        .is_some_and(|alias| pauses.deferral(alias).is_none())
                })
                .collect::<Vec<_>>();
            for instance_id in &ready {
                deferred.remove(instance_id);
            }
            ready
        };
        if !ready.is_empty() {
            lock(&self.pending_host_work, "requeue_emulator_autostart")?.push_back(
                startup_package::PendingHostWork::EmulatorAutostart(PendingEmulatorAutostart {
                    instances: ready,
                }),
            );
        }
        Ok(())
    }

    /// Drains the report lines of the configured starts for the daemon's stdout.
    pub(super) fn take_emulator_autostart_reports(&self) -> RuntimeHostResult<Vec<String>> {
        Ok(std::mem::take(&mut *lock(
            &self.emulator_autostart_reports,
            "take_emulator_autostart_reports",
        )?))
    }

    fn report_emulator_autostart(&self, line: String) -> RuntimeHostResult<()> {
        lock(
            &self.emulator_autostart_reports,
            "report_emulator_autostart",
        )?
        .push(line);
        Ok(())
    }

    /// Runs one queue entry on the startup thread. Only a fatal error is returned.
    pub(super) fn run_emulator_autostart(
        &self,
        pending: &PendingEmulatorAutostart,
    ) -> RuntimeHostResult<()> {
        let mut network = None;
        for &instance_id in &pending.instances {
            if self.fatal.is_shutdown_requested() {
                return Ok(());
            }
            let resolved = lock(&self.registered_instances, "read_autostart_instance")?
                .get(&instance_id)
                .cloned()
                .ok_or_else(|| {
                    RuntimeHostError::fatal(
                        "emulator_autostart_instance_missing",
                        OPERATION,
                        RuntimeErrorCode::RuntimeFatal,
                    )
                })?;
            if !resolved.endpoint_pending() {
                continue;
            }
            let alias = resolved.instance_alias.clone();
            let request_id = self
                .events
                .issuer()
                .mint_request_id()
                .map_err(|_| runtime_identifier_error())?;
            let causation_id = self
                .events
                .issuer()
                .mint_causation_id()
                .map_err(|_| runtime_identifier_error())?;
            let links = self
                .events
                .system_links()?
                .with_instance_id(self.events.issuer().issue_registered_instance(instance_id))
                .with_request_id(request_id)
                .with_causation_id(causation_id);
            let start = EmulatorInstanceAction::Start.event_action();
            self.append_event_raw(
                EventSeverity::Info,
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
                links.clone(),
                CommandPayloadDraft::received(start, AuditInput::new()),
            )?;
            // Review M4: a held pause keeps its "hands off", for the emulator and its startup
            // package alike; the resume that lifts it queues the start again.
            if lock(&self.scheduling_pause, "read_autostart_pause")?
                .deferral(&alias)
                .is_some()
            {
                lock(&self.emulator_autostart_deferred, "defer_autostart")?.insert(instance_id);
                self.refuse_emulator_autostart(
                    &resolved,
                    links,
                    "emulator_autostart_scheduling_paused",
                )?;
                self.report_emulator_autostart(format!(
                    "emulator_autostart_skipped instance={alias} reason=scheduling_paused"
                ))?;
                continue;
            }
            let ready = match network {
                Some(ready) => ready,
                None => {
                    let ready = self.await_network();
                    network = Some(ready);
                    ready
                }
            };
            if !ready {
                self.refuse_emulator_autostart(
                    &resolved,
                    links,
                    "emulator_autostart_network_unready",
                )?;
                self.report_emulator_autostart(format!(
                    "emulator_autostart_failed instance={alias} code=emulator_autostart_network_unready"
                ))?;
                continue;
            }
            self.await_takeover_cooldown(instance_id)?;
            match self.drive_emulator_control(
                &resolved,
                links,
                EmulatorInstanceAction::Start,
                *request_id.transport(),
            ) {
                Ok(driven) => {
                    // Review L1: the startup package is scheduled as a manual start schedules it.
                    self.schedule_startup_package(driven.startup_package)
                        .map_err(|failure| *failure.error)?;
                    self.report_emulator_autostart(format!(
                        "emulator_autostart_started instance={alias}"
                    ))?;
                }
                Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                    return Err(*failure.error);
                }
                Err(failure) => {
                    self.report_emulator_autostart(format!(
                        "emulator_autostart_failed instance={alias} code={}",
                        failure.error.code()
                    ))?;
                }
            }
        }
        Ok(())
    }

    /// Records the refusal of a configured start (`command.rejected` + `runtime.failed`).
    fn refuse_emulator_autostart(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        code: &'static str,
    ) -> RuntimeHostResult<()> {
        let mut error =
            RuntimeHostError::request(code, OPERATION, RuntimeErrorCode::InvalidRequest)
                .with_native_detail(format!("instance_alias={}", resolved.instance_alias));
        error.lifecycle.instance_id = Some(resolved.instance_id());
        match self.emulator_control_failure(
            links,
            EmulatorInstanceAction::Start.event_action(),
            error,
            RuntimeReceiptState::Denied,
            EffectDisposition::NotPerformed,
        ) {
            Ok(_) => Ok(()),
            Err(failure) => Err(*failure.error),
        }
    }

    /// Waits until the network answers, at most `NETWORK_WAIT`; `false` on timeout or shutdown.
    fn await_network(&self) -> bool {
        let deadline = Instant::now() + NETWORK_WAIT;
        loop {
            if network_answers() {
                return true;
            }
            if self.fatal.is_shutdown_requested() || Instant::now() >= deadline {
                return false;
            }
            thread::sleep(NETWORK_POLL_INTERVAL);
        }
    }

    /// Review L2: waits out a takeover cooldown that would refuse the start, at most
    /// `TAKEOVER_COOLDOWN_WAIT`; any other fence refuses the start as usual.
    fn await_takeover_cooldown(&self, instance_id: InstanceId) -> RuntimeHostResult<()> {
        let deadline = Instant::now() + TAKEOVER_COOLDOWN_WAIT;
        while self.monitor_recovery_admission(instance_id)?.reason
            == MonitorRecoveryCoordinationReason::TakeoverCooldown
            && Instant::now() < deadline
            && !self.fatal.is_shutdown_requested()
        {
            thread::sleep(TAKEOVER_COOLDOWN_POLL_INTERVAL);
        }
        Ok(())
    }
}
