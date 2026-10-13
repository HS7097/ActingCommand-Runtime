// SPDX-License-Identifier: AGPL-3.0-only

//! The startup package hook (Runtime slice #316-B3; Workflow #369 H-4, S6b).
//!
//! An instance may declare one startup package (`startup_package { package, expected_sha256 }`
//! in the `actingd` instance configuration, the same locator + digest semantics as
//! `actingctl task-run`). After a successful emulator `start` / `restart` the control request
//! only *schedules* it: one `runtime.lifecycle_observed` (phase `startup_package_scheduled`,
//! the package locator in the audit path, a fresh causation id) is appended under the control
//! request's links, the receipt says `startup_package: scheduled`, and the control's key goes
//! to the package's claim. Nothing runs on the connection thread of the start request, so the
//! control receipt returns as before.
//!
//! Workflow #369 H-4, M6 (S6b): the startup claim is a Runtime-internal routine claim (high, no
//! deadline, never preempting). The emulator control that holds the instance's key hands it to
//! the claim as it releases it, through the gate: with the gate open the claim gets the key by
//! a transfer that bypasses the queue, so no dispatch can see the instance free between the
//! start and the startup run; while a scheduling pause holds the instance or the global gate
//! (a restored pause included), the claim waits queued as a gated routine entry, the key is
//! released, and the resume's reconnect runs before the startup run (M-3). A new startup claim
//! cancels one still queued for the same instance, and so does an emulator control whose hold
//! hands no startup claim over (a stop, a failed preparation). Once granted, the package runs
//! on the instance's worker (`host_claims`) under the claim's key, as an ordinary contained
//! task with a self-minted request and correlation id, the claim's holder and synthesized
//! connection, the same causation id, hash admission and the complete `task.*` event chain
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
use crate::codes::{HostCode, HostLocation};

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
    pub(super) const fn adb_not_ready_code(self) -> HostCode {
        match self {
            Self::StartupPackage => HostCode::StartupPackageAdbNotReady,
            Self::ReturnHome => HostCode::RecoveryLadderAdbNotReady,
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
                    HostCode::StartupPackageInstanceUnknown,
                    HostLocation::StartRuntimeHost,
                    RuntimeErrorCode::RuntimeFatal,
                )
                .with_native_detail(format!("instance_alias={alias}"))
            })?;
        if instance.provenance() != ExecutionBackendProvenance::PhysicalDevice {
            return Err(RuntimeHostError::fatal(
                HostCode::StartupPackageRequiresPhysicalInstance,
                HostLocation::StartRuntimeHost,
                RuntimeErrorCode::RuntimeFatal,
            )
            .with_native_detail(format!("instance_alias={alias}")));
        }
        resolved.insert(instance.instance_id(), request.clone());
    }
    Ok(resolved)
}

/// A startup claim's own synthetic request (`AcquireLease` of its holder) and its terms.
struct StartupClaim {
    request: RuntimeRequest,
    holder_id: actingcommand_contract::HolderId,
    connection_id: ConnectionId,
    lease_ttl_ms: u64,
}

impl StartupClaim {
    fn host_claim<'a>(&'a self, instance_alias: &'a str) -> HostClaim<'a> {
        HostClaim {
            request: &self.request,
            instance_alias,
            holder_id: self.holder_id,
            connection_id: self.connection_id,
            kind: ClaimKind::StartupPackage,
            priority: actingcommand_contract::LeasePriority::High,
            lease_ttl_ms: self.lease_ttl_ms,
        }
    }
}

/// The host's synthesized connection of startup claims.
fn startup_claim_connection() -> Result<ConnectionId, RequestFailure> {
    ConnectionId::new(STARTUP_PACKAGE_CONNECTION_VALUE).map_err(|error| {
        RequestFailure::poison_without_terminal(RuntimeHostError::scheduler(
            HostLocation::BuildStartupPackageConnection,
            &error,
        ))
    })
}

impl HostShared {
    /// Queues the startup claim of a package whose scheduling intent `prepare_startup_package`
    /// recorded; `None` when the instance has no startup package configured. Workflow #369 S6b:
    /// the emulator control's end queues it this way when it hands no key to it (a scheduling
    /// pause holds the claim, M6), while it still holds the key. A claim the queue refuses is
    /// recorded as the package's failure, as a refused run was; the receipt still says
    /// `scheduled`.
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

    /// Workflow #369 H-4, M6 (S6b): the end of an operator emulator control's hold (`key`, its
    /// release `request_id`) after a start whose preparation passed, on an instance whose
    /// startup package's scheduling intent `prepare_startup_package` recorded. One startup claim
    /// per instance: a still-queued one is cancelled first. The key then goes to the new claim
    /// as the control releases it (`release_lease_to_claim`): with the gate open the claim gets
    /// it by a transfer that bypasses the queue, so no dispatch sees the instance free between
    /// the start and the startup run. Behind a closed gate the claim is queued as a gated
    /// routine entry (`schedule_startup_package`) and the key is released; the resume's
    /// reconnect then runs first, and its release hands the key on (M-3). A claim the admission
    /// checks refuse is recorded as the package's failure and the key is released; the receipt
    /// still says `scheduled`.
    pub(super) fn hand_off_startup_package(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
        request_id: RequestId,
        key: &HostKey,
        pending: PendingStartupPackage,
    ) -> Result<StartupPackageDisposition, RequestFailure> {
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        self.cancel_queued_startup_claims(resolved.instance_id())?;
        let claim = self.new_startup_claim(&pending, &resolved)?;
        let handed = self.release_lease_to_claim(
            request,
            request_id,
            &key.token,
            key.connection_id,
            claim.host_claim(&pending.instance_alias),
            HostClaimWork::StartupPackage(pending.clone()),
        );
        match handed {
            Ok(true) => return Ok(StartupPackageDisposition::Scheduled),
            Ok(false) => {
                self.schedule_startup_package(Some(pending))?;
            }
            Err(failure) if failure.poison_runtime || failure.error.is_fatal() => {
                return Err(failure);
            }
            Err(failure) => {
                self.record_startup_package_failure(&pending, &failure.error)
                    .map_err(RequestFailure::poison_without_terminal)?;
            }
        }
        self.release_lease(request, request_id, &key.token, key.connection_id, None)?;
        Ok(StartupPackageDisposition::Scheduled)
    }

    /// H-6: one startup claim per instance, then the new claim is queued (`new_startup_claim`).
    fn enqueue_startup_claim(&self, pending: PendingStartupPackage) -> Result<(), RequestFailure> {
        let resolved = self.resolve_instance(&pending.instance_alias)?;
        self.cancel_queued_startup_claims(resolved.instance_id())?;
        let claim = self.new_startup_claim(&pending, &resolved)?;
        let instance_alias = pending.instance_alias.clone();
        self.request_host_claim_work(
            claim.host_claim(&instance_alias),
            HostClaimWork::StartupPackage(pending),
        )
    }

    /// H-6: one startup claim per instance: a still-queued one is cancelled
    /// (`scheduler.denied lease.queue_cancelled`, its request links only, Warning). An emulator
    /// control whose hold hands no startup claim over (a stop, a failed preparation) cancels it
    /// too (coordinator ruling on review #716).
    pub(super) fn cancel_queued_startup_claims(
        &self,
        instance_id: InstanceId,
    ) -> Result<(), RequestFailure> {
        let connection_id = startup_claim_connection()?;
        let queued = lock(&self.scheduler, HostLocation::ReadQueuedStartupClaims)?
            .queued_of_kind(instance_id, ClaimKind::StartupPackage);
        for request_id in queued {
            self.cancel_host_claim(instance_id, request_id, connection_id)
                .map_err(RequestFailure::poison_without_terminal)?;
        }
        Ok(())
    }

    /// A new startup claim (high, no deadline) with a TTL of the package's response deadline
    /// plus the heartbeat reserve, under a fresh request, correlation and holder, the package's
    /// causation id and the host's synthesized connection.
    fn new_startup_claim(
        &self,
        pending: &PendingStartupPackage,
        resolved: &RegisteredInstance,
    ) -> Result<StartupClaim, RequestFailure> {
        let connection_id = startup_claim_connection()?;
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
                HostCode::StartupPackageClaimRequestInvalid,
                HostLocation::ScheduleStartupPackage,
                RuntimeErrorCode::RuntimeFatal,
            ))
        })?;
        Ok(StartupClaim {
            request,
            holder_id,
            connection_id,
            lease_ttl_ms: self.contained_task_lease_ttl(&pending.request)?,
        })
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
                // Ledger revision 3: the failure is the record's outcome; the former
                // `key=value` summary is not recorded (the instance is the record's link).
                None => RuntimePayloadDraft::failed(
                    DiagnosticCode::RuntimeDiagnostic,
                    EffectDisposition::NotPerformed,
                    error.record_outcome(),
                    DiagnosticDetailDraft::new(
                        pending.run.failure_category(),
                        RuntimeLifecycleFailureStage::OperationCleanup.as_str(),
                        "runtime_host",
                        error.operation(),
                    ),
                    AuditInput::new(),
                ),
            },
        )?;
        self.record_required_failure(error, &failure, links)
    }
}
