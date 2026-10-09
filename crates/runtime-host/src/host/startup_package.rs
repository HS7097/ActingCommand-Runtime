// SPDX-License-Identifier: AGPL-3.0-only

//! The startup package hook (Runtime slice #316-B3; Workflow #369 H-6).
//!
//! An instance may declare one startup package (`startup_package { package, expected_sha256 }`
//! in the `actingd` instance configuration, the same locator + digest semantics as
//! `actingctl task-run`). After a successful emulator `start` / `restart` the control request
//! only *schedules* it: one `runtime.lifecycle_observed` (phase `startup_package_scheduled`,
//! the package locator in the audit path, a fresh causation id) is appended under the control
//! request's links, the receipt says `startup_package: scheduled`, and the package's claim is
//! queued. Nothing runs on the connection thread of the start request, so the control receipt
//! returns as before.
//!
//! Workflow #369 H-6: the startup claim is a Runtime-internal claim (high, no deadline, never
//! preempting), queued while the control still holds the instance's admission guard; the
//! guard's drop pumps it, so no dispatch can see the instance free in between. Until #369 S6b
//! it is not gated by a scheduling pause (`scheduling-pause.md`, "not gated"). A new startup
//! claim cancels one still queued for the same instance. Once granted, the package runs on the
//! instance's worker (`host_claims`) under the claim's key, as an ordinary contained task with
//! a self-minted request and correlation id, the claim's holder and synthesized connection, the
//! same causation id, hash admission and the complete `task.*` event chain
//! (`HostShared::run_package_on_key`). A package that cannot be admitted fails typed before its
//! run: `startup_package_missing` (the locator does not open) or
//! `startup_package_admission_failed` (every other admission refusal, the underlying code
//! attached as related failure). Failures are recorded as `runtime.failed` under the instance
//! and the causation id; a fatal one poisons the host like any other.
//!
//! A configured package is scheduled after `start` / `restart` when connection preparation
//! succeeds; an instance without one has no package scheduled. A daemon already running at
//! startup schedules nothing: only the two control actions do.
//!
//! The stuck-recovery ladder (`recovery_ladder`) runs its rung packages through the same
//! runner on its own key.

use super::host_claims::{HostClaimWork, HostKey};
use super::lease::HostClaim;
use super::recovery_ladder::PendingRecoveryLadder;
use super::*;

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

/// One host-scheduled package run: a startup package emulator control queued as a claim, or a
/// stuck-recovery rung's package (`run` tells which).
#[derive(Clone)]
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
    pub(super) recovery_rung: bool,
    /// Workflow #336 L2d: on a ladder rung run of the configured return-home package, the
    /// failed package it must match.
    pub(super) configured_return_home: Option<Box<super::contained_task::PackageIdentity>>,
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

impl HostShared {
    /// Workflow #369 H-6: queues the startup claim of a package whose scheduling intent
    /// `prepare_startup_package` recorded; `None` when the instance has no startup package
    /// configured. The caller holds the instance's admission guard and pumps once it lets the
    /// guard go. A claim the queue refuses is recorded as the package's failure, as a refused
    /// run was; the receipt still says `scheduled`.
    pub(super) fn schedule_startup_package(
        &self,
        pending: Option<PendingStartupPackage>,
    ) -> Result<StartupPackageDisposition, RequestFailure> {
        let Some(pending) = pending else {
            return Ok(StartupPackageDisposition::None);
        };
        match self.enqueue_startup_claim(pending.clone()) {
            Ok(()) => {}
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(failure);
            }
            Err(failure) => {
                self.record_startup_package_failure(&pending, &failure.error)
                    .map_err(RequestFailure::poison_without_terminal)?;
            }
        }
        Ok(StartupPackageDisposition::Scheduled)
    }

    /// H-6: one startup claim per instance. A still-queued one is cancelled
    /// (`scheduler.denied lease.queue_cancelled`, its request links only, Warning), then the new
    /// claim is queued (high, no deadline) with a TTL of the package's response deadline plus
    /// the heartbeat reserve, under a fresh request, correlation and holder, the package's
    /// causation id and the host's synthesized connection.
    fn enqueue_startup_claim(&self, pending: PendingStartupPackage) -> Result<(), RequestFailure> {
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        let connection_id =
            ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
                RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
                    "build_startup_package_connection",
                    &error,
                ))
            })?;
        let queued = lock(&self.scheduler, "read_queued_startup_claims")?
            .queued_of_kind(resolved.instance_id(), ClaimKind::StartupPackage);
        for request_id in queued {
            self.cancel_host_claim(resolved.instance_id(), request_id, connection_id)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
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
            Some(pending.causation_id),
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
                "startup_package_claim_request_invalid",
                "schedule_startup_package",
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        let lease_ttl_ms = self.contained_task_lease_ttl(&pending.request)?;
        let instance_alias = pending.instance_alias.clone();
        self.request_host_claim_work(
            HostClaim {
                request: &request,
                instance_alias: &instance_alias,
                holder_id,
                connection_id,
                kind: ClaimKind::StartupPackage,
                priority: actingcommand_contract::LeasePriority::High,
                lease_ttl_ms,
            },
            HostClaimWork::StartupPackage(pending),
        )
    }

    /// Workflow #369 W-2: a granted startup claim on its instance's worker: the package runs
    /// on the claim's key, then the key is released if the run did not take it over.
    pub(super) fn run_startup_claim(
        &self,
        pending: &PendingStartupPackage,
        mut key: HostKey,
    ) -> RuntimeHostResult<()> {
        let ran = self.run_package_on_key(pending, &mut key, None).map(|_| ());
        let released = self.release_host_key(&key);
        ran.and(released)
    }

    /// Workflow #369 S3a, H-3, H-6: runs one host package on the held `key`: the checks before
    /// its run (`prepare_package_run`), a renewal to its response deadline plus the heartbeat
    /// reserve, then the run, which releases the key once under its own run links. For a
    /// ladder's rung (`ladder`) the ladder's continuation is queued just before the run, so the
    /// run's release hands the key back; `key` then holds the continuation's token. A
    /// non-fatal failure is recorded here and returned as its code; a fatal one is the error.
    pub(super) fn run_package_on_key(
        &self,
        pending: &PendingStartupPackage,
        key: &mut HostKey,
        ladder: Option<&PendingRecoveryLadder>,
    ) -> RuntimeHostResult<Result<OperationSuccess, &'static str>> {
        let run = match self.prepare_package_run(pending, key.token.holder_id()) {
            Ok(run) => run,
            Err(failure) => return self.host_package_failure(pending, failure).map(Err),
        };
        let renewed = self
            .contained_task_lease_ttl(&pending.request)
            .and_then(|lease_ttl_ms| self.renew_host_key(key, lease_ttl_ms));
        if let Err(failure) = renewed {
            drop(run);
            return self.host_package_failure(pending, failure).map(Err);
        }
        let continuation = match ladder {
            Some(ladder) => match self.queue_ladder_continuation(ladder, key) {
                Ok(continuation) => Some(continuation),
                Err(failure) => {
                    drop(run);
                    return self.host_package_failure(pending, failure).map(Err);
                }
            },
            None => None,
        };
        let executed = self.run_prepared_package(
            run,
            super::contained_task::HeldPackageLease {
                token: key.token.clone(),
                connection_id: key.connection_id,
            },
        );
        if let Some(continuation) = continuation {
            self.take_ladder_continuation(key, continuation)?;
        }
        match executed {
            Ok(success) => Ok(Ok(success)),
            Err(failure) => self.host_package_failure(pending, failure).map(Err),
        }
    }

    /// Records a host package's non-fatal failure and returns its code; a fatal one is the
    /// error, after its record.
    fn host_package_failure(
        &self,
        pending: &PendingStartupPackage,
        failure: RequestFailure,
    ) -> RuntimeHostResult<&'static str> {
        self.record_startup_package_failure(pending, &failure.error)?;
        if failure.poison_runtime || failure.error.is_fatal() {
            return Err(*failure.error);
        }
        Ok(failure.error.code())
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
        let Some(request) = self.startup_packages()?.get(&instance_id) else {
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
            recovery_rung: false,
            configured_return_home: None,
        }))
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
