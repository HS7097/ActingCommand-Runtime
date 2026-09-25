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
//!
//! Slice #316-B4: the same queue and thread also carry stuck-recovery ladders
//! (`recovery_ladder`), which run their rung packages through this module's runner.

use super::*;
use std::time::Duration;

const SCHEDULE_OPERATION: &str = "schedule_startup_package";
const STARTUP_PACKAGE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One unit of work for the host's own scheduling thread (slice #316-B4 generalised the
/// startup package queue).
pub(super) enum PendingHostWork {
    StartupPackage(PendingStartupPackage),
    RecoveryLadder(Box<super::recovery_ladder::PendingRecoveryLadder>),
}

/// Which host-scheduled package a run is: the typed admission codes and the failure record
/// category depend on it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HostPackageRun {
    /// The instance's startup package (#316-B3).
    StartupPackage,
    /// A failed run's bound recovery package, run standalone by the stuck-recovery ladder.
    ReturnHome,
}

impl HostPackageRun {
    pub(super) const fn adb_not_ready_code(self) -> &'static str {
        match self {
            Self::StartupPackage => "startup_package_adb_not_ready",
            Self::ReturnHome => "recovery_ladder_adb_not_ready",
        }
    }

    const fn failure_category(self) -> &'static str {
        match self {
            Self::StartupPackage => "startup_package",
            Self::ReturnHome => "recovery_ladder",
        }
    }
}

/// One host-scheduled package run: a startup package emulator control handed to the
/// scheduling thread, or a stuck-recovery rung's package (`run` tells which).
pub(super) struct PendingStartupPackage {
    pub(super) instance_id: InstanceId,
    pub(super) instance_alias: String,
    pub(super) request: ContainedTaskRequest,
    /// Shared by the scheduling event and every event of the run.
    pub(super) causation_id: actingcommand_contract::IssuedCausationId,
    /// The emulator control request, or the ladder, that scheduled the package (task timing
    /// admission id).
    pub(super) control_request_id: RequestId,
    pub(super) run: HostPackageRun,
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
        let pending = lock(&shared.pending_host_work, "read_pending_host_work")?.pop_front();
        let Some(pending) = pending else {
            thread::sleep(STARTUP_PACKAGE_POLL_INTERVAL);
            continue;
        };
        let Some(_work) = shared.begin_work()? else {
            return Ok(());
        };
        let result = match pending {
            PendingHostWork::StartupPackage(pending) => {
                shared.run_pending_startup_package(&pending).map(|_| ())
            }
            PendingHostWork::RecoveryLadder(pending) => shared.run_recovery_ladder(&pending),
        };
        if let Err(error) = result {
            shared.fatal.mark(error.clone())?;
            return Err(error);
        }
    }
    Ok(())
}

impl HostShared {
    /// Queues a package whose scheduling intent `prepare_startup_package` recorded; `None`
    /// when the instance has no startup package configured.
    pub(super) fn schedule_startup_package(
        &self,
        pending: Option<PendingStartupPackage>,
    ) -> Result<StartupPackageDisposition, RequestFailure> {
        let Some(pending) = pending else {
            return Ok(StartupPackageDisposition::None);
        };
        lock(&self.pending_host_work, SCHEDULE_OPERATION)?
            .push_back(PendingHostWork::StartupPackage(pending));
        Ok(StartupPackageDisposition::Scheduled)
    }

    /// Records the scheduling intent under `links` (a fresh causation id) and returns the
    /// package to run; `None` when the instance has no startup package configured.
    pub(super) fn prepare_startup_package(
        &self,
        resolved: &RegisteredInstance,
        links: EventLinksDraft,
        control_request_id: RequestId,
    ) -> Result<Option<PendingStartupPackage>, RequestFailure> {
        let instance_id = resolved.instance_id();
        let Some(request) = self.startup_packages.get(&instance_id) else {
            return Ok(None);
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
        Ok(Some(PendingStartupPackage {
            instance_id,
            instance_alias: resolved.instance_alias.clone(),
            request: request.clone(),
            causation_id,
            control_request_id,
            run: HostPackageRun::StartupPackage,
        }))
    }

    /// Runs one package; a non-fatal failure is recorded here and returned as its code, a
    /// fatal one is returned as the error so the thread poisons the host.
    pub(super) fn run_pending_startup_package(
        &self,
        pending: &PendingStartupPackage,
    ) -> RuntimeHostResult<Result<OperationSuccess, &'static str>> {
        match self.run_startup_package(pending) {
            Ok(success) => Ok(Ok(success)),
            Err(failure) => {
                self.record_startup_package_failure(pending, &failure.error)?;
                if failure.poison_runtime || failure.error.is_fatal() {
                    return Err(*failure.error);
                }
                Ok(Err(failure.error.code()))
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
        if error.diagnostics().recorded_event().get().is_some() {
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
                        pending.run.failure_category(),
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
