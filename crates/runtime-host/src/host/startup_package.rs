// SPDX-License-Identifier: AGPL-3.0-only

//! The startup package hook (Runtime slice #316-B3).
//!
//! An instance may declare one startup package (`startup_package { package, expected_sha256 }`
//! in the `actingd` instance configuration, the same locator + digest semantics as
//! `actingctl task-run`). After a successful emulator `start` / `restart` the control request
//! only *schedules* it: one `runtime.lifecycle_observed` (phase `startup_package_scheduled`,
//! the package locator in the audit path, a fresh causation id) is appended under the control
//! request's links, the receipt says `startup_package: scheduled`, and the entry is queued for
//! the host's own scheduling thread. Nothing runs on the connection thread of the start
//! request, so the control receipt returns as before.
//!
//! The scheduling thread runs the package as an ordinary contained task with a self-minted
//! request / correlation / holder id, a synthesized connection, the same causation id, hash
//! admission, its own lease and the complete `task.*` event chain
//! (`HostShared::run_startup_package`). A package that cannot be admitted fails typed before
//! any lease: `startup_package_missing` (the locator does not open) or
//! `startup_package_admission_failed` (every other admission refusal, the underlying code
//! attached as related failure). Failures are recorded as `runtime.failed` under the
//! instance and the causation id; a fatal one poisons the host like any other.
//!
//! A configured package is always invoked after `start` / `restart`; an instance without one
//! never has anything pulled. A daemon that finds the instance already running at startup
//! schedules nothing: only the two control actions do.

use super::*;
use std::time::Duration;

const SCHEDULE_OPERATION: &str = "schedule_startup_package";
const STARTUP_PACKAGE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One startup package emulator control handed to the scheduling thread.
pub(super) struct PendingStartupPackage {
    pub(super) instance_id: InstanceId,
    pub(super) instance_alias: String,
    pub(super) request: ContainedTaskRequest,
    /// Shared by the scheduling event and every event of the run.
    pub(super) causation_id: actingcommand_contract::IssuedCausationId,
    /// The emulator control request that scheduled the package (task timing admission id).
    pub(super) control_request_id: RequestId,
}

/// Binds the configured startup packages to registered physical instances at startup. An
/// alias that is not registered fails with `startup_package_instance_unknown`; a fixture
/// instance with `startup_package_requires_physical_instance`.
pub(super) fn resolve_startup_packages(
    configured: &BTreeMap<String, ContainedTaskRequest>,
    instances: &BTreeMap<InstanceId, RegisteredInstance>,
) -> RuntimeHostResult<BTreeMap<InstanceId, ContainedTaskRequest>> {
    let mut resolved = BTreeMap::new();
    for (alias, request) in configured {
        let instance = instances
            .values()
            .find(|instance| instance.instance_alias == *alias)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    "startup_package_instance_unknown",
                    "start_runtime_host",
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_native_detail(format!("instance_alias={alias}"))
            })?;
        if instance.provenance() != ExecutionBackendProvenance::PhysicalDevice {
            return Err(RuntimeHostError::fatal(
                "startup_package_requires_physical_instance",
                "start_runtime_host",
                RuntimeErrorCode::RuntimeFatal,
            )
            .with_native_detail(format!("instance_alias={alias}")));
        }
        resolved.insert(instance.instance_id(), request.clone());
    }
    Ok(resolved)
}

/// The host's own scheduling point for startup packages: drains the queue one package at a
/// time under the work guard, like the monitor thread drains its probes.
pub(super) fn startup_package_loop(shared: Arc<HostShared>) -> RuntimeHostResult<()> {
    while !shared.fatal.is_shutdown_requested() {
        let pending = lock(
            &shared.pending_startup_packages,
            "read_pending_startup_packages",
        )?
        .pop_front();
        let Some(pending) = pending else {
            thread::sleep(STARTUP_PACKAGE_POLL_INTERVAL);
            continue;
        };
        let Some(_work) = shared.begin_work()? else {
            return Ok(());
        };
        if let Err(error) = shared.run_pending_startup_package(pending) {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
    }
    Ok(())
}

impl HostShared {
    /// Records the scheduling intent under the control request and queues the package.
    /// `None` when the instance has no startup package configured.
    pub(super) fn schedule_startup_package(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        control_request_id: RequestId,
    ) -> Result<StartupPackageDisposition, RequestFailure> {
        let instance_id = resolved.instance_id();
        let Some(request) = self.startup_packages.get(&instance_id) else {
            return Ok(StartupPackageDisposition::None);
        };
        let causation_id = self
            .events
            .issuer()
            .mint_causation_id()
            .map_err(|_| RequestFailure::poison_without_terminal(runtime_identifier_error()))?;
        self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.with_causation_id(causation_id),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner_epoch,
                RuntimeLifecyclePhase::StartupPackageScheduled { instance_id },
                audit_path(Path::new(request.package_path())),
            ),
        )?;
        lock(&self.pending_startup_packages, SCHEDULE_OPERATION)?.push_back(
            PendingStartupPackage {
                instance_id,
                instance_alias: resolved.instance_alias.clone(),
                request: request.clone(),
                causation_id,
                control_request_id,
            },
        );
        Ok(StartupPackageDisposition::Scheduled)
    }

    /// Runs one queued package; a non-fatal failure is recorded and consumed here, a fatal
    /// one is returned so the thread poisons the host.
    fn run_pending_startup_package(&self, pending: PendingStartupPackage) -> RuntimeHostResult<()> {
        match self.run_startup_package(&pending) {
            Ok(_) => Ok(()),
            Err(failure) => {
                self.record_startup_package_failure(&pending, &failure.error)?;
                if failure.poison_runtime || failure.error.is_fatal() {
                    return Err(*failure.error);
                }
                Ok(())
            }
        }
    }

    /// `runtime.failed` for a startup package that did not complete, linked to the instance
    /// and the causation id of its scheduling event. An error whose event was already
    /// recorded (a task terminal, a rejection) only gets the lifecycle failure record.
    fn record_startup_package_failure(
        &self,
        pending: &PendingStartupPackage,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        if error.projection().code == RuntimeErrorCode::LedgerFailure {
            return Err(error.clone());
        }
        let links = self
            .events
            .system_links()?
            .with_instance_id(
                self.events
                    .issuer()
                    .issue_registered_instance(pending.instance_id),
            )
            .with_causation_id(pending.causation_id);
        if error.lifecycle.recorded_event.get().is_some() {
            return self.append_lifecycle_failure(
                RuntimeLifecycleFailureStage::OperationCleanup,
                RuntimeLifecycleFailure::Host(error),
                links,
                None,
            );
        }
        let failure = self.append_event_raw(
            if error.is_fatal() {
                EventSeverity::Fatal
            } else {
                EventSeverity::Warning
            },
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            match error.resource_declaration() {
                Some(rejection) => {
                    RuntimePayloadDraft::resource_declaration_rejected(rejection.clone())
                }
                None => RuntimePayloadDraft::failed(
                    DiagnosticCode::RuntimeDiagnostic,
                    EffectDisposition::NotPerformed,
                    DiagnosticDetailDraft::new(
                        "startup_package",
                        RuntimeLifecycleFailureStage::OperationCleanup.as_str(),
                        "runtime_host",
                        error.operation(),
                        format!(
                            "host_code={} fatal={} instance_alias={}",
                            error.code(),
                            error.is_fatal(),
                            pending.instance_alias
                        ),
                        Sensitivity::Internal,
                    ),
                    AuditInput::new(),
                ),
            },
        )?;
        self.record_required_failure(error, &failure, links)
    }
}
