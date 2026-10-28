// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-M (board #419; Alice 10-24: one emulator's fault must not stop the whole Runtime): every
//! failure has exactly one scope, `Global`, `Instance` or `Operation` ([`FailureScope`]), and
//! only a `Global` failure stops the Runtime.
//!
//! - **The closed Global list** ([`GLOBAL_CODES`]; `contracts/runtime-outcomes.md`, "Failure
//!   scope"): the codes whose failure is the Runtime's as a whole, by family (R0.3), and every
//!   catalog code of category `fatal`. A failure with one of these codes stays `Global` even when
//!   its constructor tagged it as one instance's.
//! - **The scope rule** ([`RuntimeHostError::scope_at`]): constructors carry the scope. An
//!   untagged failure is `Global` when it is fatal (fail-safe, R0.4: an unclassified failure
//!   keeps halting) and `Operation` otherwise; the slices that own a path tag it
//!   (`RuntimeHostError::instance_fault`, `RuntimeHostError::instance_scoped`,
//!   [`lock_instance`]). The wrappers keep the scope.
//! - **One decision point** ([`HostShared::contain_failure`], R0.5): `Global` latches the
//!   Runtime, `Instance` fences the instance, `Operation` fails only the operation; a fence that
//!   fails latches (R0.7).
//! - **The five boundaries** that call it (R0.5): the request door, which is also the direct
//!   run's boundary (`requests.rs`, since ISO-1); the scheduled and host-package run boundaries
//!   (ISO-2), the instance worker (ISO-3), each per-instance step of a shared loop (ISO-4a), and
//!   actingd's driver per intent (ISO-6, through [`RuntimeHostError::halts_runtime`]).
//!
//! No source-text guard holds this (#420 S5 retired them): the fail-safe default keeps an
//! unclassified failure `Global`, and reviews catch a new path that latches outside
//! `contain_failure`.

use super::*;
use crate::FailureScope;
use crate::codes::{ConfigHostCode, HostCode, HostLocation};
use crate::error::spelled;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Category, Code, Location};
use actingcommand_execution_kernel::ExecutionKernelCode;
use actingcommand_ledger::LedgerCode;
use actingcommand_runtime_database::DatabaseCode;
use actingcommand_scheduler::codes::SchedulerCode;

/// The families of the closed Global list (ISO-M R0.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalFamily {
    /// The ledger: a write, a commit, its integrity, its format, its dump; a failed ledger
    /// append; the fact store out of step with the ledger.
    Ledger,
    /// The configuration: actingd's configuration, the effective configuration, the catalog
    /// and the approvals.
    Configuration,
    /// The owner lock and the owner journal's I/O.
    Owner,
    /// Identifier issuance and connection ids.
    Identifier,
    /// An invalid or regressed system or monotonic clock.
    Clock,
    /// A poisoned lock that every instance shares; the kernel's session registry.
    SharedLock,
    /// The process's own resources: the IPC listener, an OS thread, the state root and the
    /// runtime information file.
    ProcessResource,
    /// The scheduler's cross-instance invariants (`SchedulerError::is_fatal`).
    SchedulerInvariant,
    /// The shared policy projection out of step with the ledger (D26, D30).
    PolicyProjection,
    /// The installer's transitions.
    Installer,
    /// A code the catalog files under category `fatal`.
    CatalogFatal,
}

/// One code of the closed Global list, with its family.
pub(crate) struct GlobalCode {
    pub(crate) family: GlobalFamily,
    pub(crate) code: Code,
}

const fn global(family: GlobalFamily, code: Code) -> GlobalCode {
    GlobalCode { family, code }
}

/// ISO-M (#419, R0.3): the closed Global list. Every catalog code of category `fatal` is on it
/// too ([`global_family`]). `RuntimeHostError::fatal` is meant for these codes only; the ISO
/// slices convert the other fatal constructors of their paths.
pub(crate) const GLOBAL_CODES: &[GlobalCode] = &[
    // The ledger.
    global(GlobalFamily::Ledger, RuntimeErrorCode::LedgerFailure.code()),
    global(GlobalFamily::Ledger, HostCode::AgentLedgerFailure.code()),
    global(GlobalFamily::Ledger, HostCode::MonitorLedgerFailure.code()),
    global(
        GlobalFamily::Ledger,
        HostCode::CatalogFailureFactUndurable.code(),
    ),
    global(
        GlobalFamily::Ledger,
        HostCode::ReleaseFailureFactUndurable.code(),
    ),
    global(
        GlobalFamily::Ledger,
        HostCode::RuntimeFactStoreDesync.code(),
    ),
    global(GlobalFamily::Ledger, HostCode::FactStoreSyncFailed.code()),
    global(
        GlobalFamily::Ledger,
        HostCode::FactStoreRecoveryFailed.code(),
    ),
    global(
        GlobalFamily::Ledger,
        SharedCode::RuntimeFactReplayFailed.code(),
    ),
    global(
        GlobalFamily::Ledger,
        SharedCode::LedgerMigrationRequired.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::LedgerWriteFailed.code()),
    global(
        GlobalFamily::Ledger,
        LedgerCode::LedgerInsertIncomplete.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::LedgerIo.code()),
    global(GlobalFamily::Ledger, LedgerCode::LedgerSqliteFailed.code()),
    global(
        GlobalFamily::Ledger,
        LedgerCode::CriticalEventNotDurable.code(),
    ),
    global(
        GlobalFamily::Ledger,
        LedgerCode::DeferredAppendUnconfirmed.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::WriterUnavailable.code()),
    global(GlobalFamily::Ledger, LedgerCode::WriterPanicked.code()),
    global(GlobalFamily::Ledger, LedgerCode::CorruptLedgerRecord.code()),
    global(GlobalFamily::Ledger, LedgerCode::CorruptSegment.code()),
    // #422 LN-9: what a full verification, and since LN-9 the ledger's scrub, finds in a row,
    // the head or the file.
    global(
        GlobalFamily::Ledger,
        LedgerCode::LedgerRecordMismatch.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::LedgerMetaMismatch.code()),
    global(GlobalFamily::Ledger, LedgerCode::DuplicateEventId.code()),
    global(
        GlobalFamily::Ledger,
        DatabaseCode::StateDatabaseCorrupt.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::LedgerIndexMismatch.code()),
    global(
        GlobalFamily::Ledger,
        LedgerCode::LedgerTagIndexMismatch.code(),
    ),
    global(
        GlobalFamily::Ledger,
        LedgerCode::SequenceDiscontinuity.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::SequenceExhausted.code()),
    global(
        GlobalFamily::Ledger,
        LedgerCode::LedgerFormatUnsupported.code(),
    ),
    global(
        GlobalFamily::Ledger,
        LedgerCode::LedgerFormatMarkerMismatch.code(),
    ),
    global(GlobalFamily::Ledger, LedgerCode::LedgerDumpFailed.code()),
    global(
        GlobalFamily::Ledger,
        LedgerCode::LedgerDumpUnsupported.code(),
    ),
    // The configuration, the catalog and the approvals.
    global(
        GlobalFamily::Configuration,
        HostCode::InvalidRuntimeHostConfig.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::InvalidRuntimeConfigManifest.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::AgentDispatcherConfigMissing.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::PolicyInputsUnconfigured.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::ProcedureManifestUnconfigured.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::PerformanceConfigInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::PerformanceControlConfigInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::ConfigReadFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::ConfigDecodeFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::EffectiveConfigurationTooLarge.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::AgentDispatcherConfigInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::InvalidCapacityThresholds.code(),
    ),
    global(
        GlobalFamily::Configuration,
        ConfigHostCode::OverridesFactInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        ConfigHostCode::OverridesPersistFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        ConfigHostCode::ValueTypeInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogCompileFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogManifestInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogPointerInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogGenerationInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogPlanGenerationMismatch.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogMigrationSourceMissing.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::CatalogStateCreateFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::PolicyCatalogMismatch.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::PolicyCatalogApprovalConflict.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::PolicyCatalogUnavailable.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::PolicyCatalogCompileFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::PolicyCatalogApprovalInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::ApprovalReplayConflict.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::ApprovalReplayPayloadMismatch.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::ApprovalProjectionQueryFailed.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::ApprovalProjectionPositionInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        HostCode::ApprovalProjectionCapacityExceeded.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::ApprovalProjectionOrderInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::ApprovalProjectionOriginInvalid.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::ApprovalProjectionPayloadMismatch.code(),
    ),
    global(
        GlobalFamily::Configuration,
        SharedCode::ApprovalProjectionRecordInvalid.code(),
    ),
    // The owner lock and the owner journal's I/O.
    global(GlobalFamily::Owner, HostCode::OwnerLockFailed.code()),
    global(GlobalFamily::Owner, HostCode::OwnerFileOpenFailed.code()),
    global(GlobalFamily::Owner, HostCode::OwnerFileMissing.code()),
    global(GlobalFamily::Owner, HostCode::OwnerWriteFailed.code()),
    global(GlobalFamily::Owner, HostCode::OwnerSyncFailed.code()),
    global(
        GlobalFamily::Owner,
        HostCode::OwnerRecordEncodeFailed.code(),
    ),
    global(GlobalFamily::Owner, HostCode::OwnerRevisionOverflow.code()),
    global(GlobalFamily::Owner, HostCode::OwnerEpochIssueFailed.code()),
    global(
        GlobalFamily::Owner,
        HostCode::OwnerCompactionCleanupFailed.code(),
    ),
    global(
        GlobalFamily::Owner,
        HostCode::OwnerCompactionRewriteFailed.code(),
    ),
    global(
        GlobalFamily::Owner,
        HostCode::OwnerCompactionSideReadFailed.code(),
    ),
    global(
        GlobalFamily::Owner,
        HostCode::OwnerCompactionStageFailed.code(),
    ),
    global(GlobalFamily::Owner, HostCode::OwnerUnlockFailed.code()),
    global(GlobalFamily::Owner, SharedCode::OwnerSeekFailed.code()),
    global(
        GlobalFamily::Owner,
        SharedCode::OwnerCompactionUnrecoverable.code(),
    ),
    // Identifier issuance.
    global(
        GlobalFamily::Identifier,
        SharedCode::RuntimeIdentifierIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        SharedCode::EventIssuerFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::EventIdIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::ActionIdIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::PolicyIdentifierIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::MonitorProbeIdIssueFailed.code(),
    ),
    global(GlobalFamily::Identifier, HostCode::RunIdIssueFailed.code()),
    global(GlobalFamily::Identifier, HostCode::TaskIdIssueFailed.code()),
    global(
        GlobalFamily::Identifier,
        HostCode::AgentWakeIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::ReadonlyCapabilityIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::LabProjectionIdIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::LabFrameIdIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::LabRecognitionIdIssueFailed.code(),
    ),
    global(
        GlobalFamily::Identifier,
        HostCode::ObservationRecognitionIdIssueFailed.code(),
    ),
    // The clocks.
    global(GlobalFamily::Clock, SharedCode::SystemClockInvalid.code()),
    global(GlobalFamily::Clock, HostCode::SystemClockOverflow.code()),
    global(
        GlobalFamily::Clock,
        HostCode::MonotonicClockRegressed.code(),
    ),
    global(GlobalFamily::Clock, HostCode::MonotonicClockOverflow.code()),
    global(
        GlobalFamily::Clock,
        HostCode::PolicyAdmissionClockRegressed.code(),
    ),
    global(
        GlobalFamily::Clock,
        HostCode::PolicyAdmissionClockOverflow.code(),
    ),
    global(
        GlobalFamily::Clock,
        HostCode::PolicyDispatchClockRegressed.code(),
    ),
    global(
        GlobalFamily::Clock,
        HostCode::PolicyExecutionClockRegressed.code(),
    ),
    global(GlobalFamily::Clock, LedgerCode::ClockBeforeEpoch.code()),
    // Locks every instance shares, and the kernel's session registry.
    global(
        GlobalFamily::SharedLock,
        HostCode::RuntimeStatePoisoned.code(),
    ),
    global(
        GlobalFamily::SharedLock,
        HostCode::FatalStatePoisoned.code(),
    ),
    global(
        GlobalFamily::SharedLock,
        HostCode::CapacityProjectionPoisoned.code(),
    ),
    global(
        GlobalFamily::SharedLock,
        LedgerCode::DeferredRepliesPoisoned.code(),
    ),
    global(
        GlobalFamily::SharedLock,
        ExecutionKernelCode::ExecutionKernelStatePoisoned.code(),
    ),
    // ISO-2 (#419 R1): the kernel's session registry closed, or out of session generations.
    global(
        GlobalFamily::SharedLock,
        ExecutionKernelCode::ExecutionKernelClosed.code(),
    ),
    global(
        GlobalFamily::SharedLock,
        ExecutionKernelCode::ExecutionSessionGenerationExhausted.code(),
    ),
    // The process's own resources.
    // ISO-2 (#419 R1): the OS refused a session thread.
    global(
        GlobalFamily::ProcessResource,
        ExecutionKernelCode::ExecutionSessionSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeBindFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeListenerAddressFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeListenerConfigFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeSocketModeFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeAcceptFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeAcceptSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeConnectionSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeSweeperSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeMonitorSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimePerformanceSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimePackAdmissionSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::InstanceWorkerSpawnFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeThreadPanicked.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        SharedCode::StateRootCreateFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        SharedCode::StateRootInspectFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        SharedCode::RuntimeInfoInvalid.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeInfoCreateFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeInfoWriteFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeInfoPublishFailed.code(),
    ),
    global(
        GlobalFamily::ProcessResource,
        HostCode::RuntimeInfoReplaceFailed.code(),
    ),
    // The scheduler's cross-instance invariants.
    global(
        GlobalFamily::SchedulerInvariant,
        SchedulerCode::InvalidSchedulerConfig.code(),
    ),
    global(
        GlobalFamily::SchedulerInvariant,
        SchedulerCode::InvalidConnectionId.code(),
    ),
    global(
        GlobalFamily::SchedulerInvariant,
        SchedulerCode::IdentifierIssuanceFailed.code(),
    ),
    global(
        GlobalFamily::SchedulerInvariant,
        SchedulerCode::LeaseExpiryOverflow.code(),
    ),
    global(
        GlobalFamily::SchedulerInvariant,
        SchedulerCode::LeaseQueueSequenceOverflow.code(),
    ),
    global(
        GlobalFamily::SchedulerInvariant,
        SchedulerCode::LeaseStepSequenceOverflow.code(),
    ),
    // The shared policy projection and the ledger.
    global(
        GlobalFamily::PolicyProjection,
        HostCode::PolicyAdmissionEventInvalid.code(),
    ),
    global(
        GlobalFamily::PolicyProjection,
        HostCode::PolicyStateLedgerIncomplete.code(),
    ),
    // The installer's transitions.
    global(
        GlobalFamily::Installer,
        HostCode::RuntimeComponentNotPrepared.code(),
    ),
    global(
        GlobalFamily::Installer,
        HostCode::RuntimeComponentAlreadyPrepared.code(),
    ),
    global(
        GlobalFamily::Installer,
        HostCode::InstallAbortStateInvalid.code(),
    ),
    global(
        GlobalFamily::Installer,
        HostCode::InstallReleaseStateInvalid.code(),
    ),
];

/// ISO-2b (#419 R1, ISO-M rows E11, E12): the kernel codes of an instance's device session that
/// died: its thread panicked, ended or lost its reply, or its state lock was poisoned. A capture
/// that meets one fails only its work and retires the session; the instance's second such death
/// in a row fences it (`HostShared::session_died_again`).
pub(crate) const SESSION_DEATH_CODES: &[Code] = &[
    ExecutionKernelCode::ExecutionSessionPanicked.code(),
    ExecutionKernelCode::ExecutionSessionStatePoisoned.code(),
    ExecutionKernelCode::ExecutionSessionClosed.code(),
    ExecutionKernelCode::ExecutionSessionUnavailable.code(),
    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
];

/// The Global family of the code spelled `spelling`: its family on [`GLOBAL_CODES`], else
/// `CatalogFatal` when the embedded catalog files the code under category `fatal`; `None` for a
/// code off the list.
pub(crate) fn global_family(spelling: &'static str) -> Option<GlobalFamily> {
    GLOBAL_CODES
        .iter()
        .find(|entry| entry.code.as_str() == spelling)
        .map(|entry| entry.family)
        .or_else(|| {
            (Code::relayed(spelling).map(Code::category) == Some(Category::Fatal))
                .then_some(GlobalFamily::CatalogFatal)
        })
}

/// Whether the code spelled `spelling` is on the closed Global list ([`global_family`]).
pub(crate) fn is_global_code(spelling: &'static str) -> bool {
    global_family(spelling).is_some()
}

/// What [`HostShared::contain_failure`] did with a failure (ISO-M §6).
#[derive(Debug)]
pub(crate) enum Contained {
    /// `Global`: the Runtime latched this failure (the first latched condition stays the
    /// Runtime's).
    Latched(RuntimeHostError),
    /// `Instance`: the instance is fenced; the work ends with the fence's non-fatal failure.
    Fenced(RuntimeHostError),
    /// `Operation`: only the operation fails, with this non-fatal failure.
    Failed(RuntimeHostError),
}

impl HostShared {
    /// ISO-M (#419, R0.2, R0.5): the one decision point for `error` at a boundary whose work
    /// met `boundary_instance` (when the boundary knows one):
    /// - `Global`: latches the Runtime ([`FatalState::mark`]);
    /// - `Instance`: fences the instance (`fence_contained_failure`) and returns the fence's
    ///   non-fatal failure; a failure that no registered instance owns latches;
    /// - `Operation`: returns the failure, which ends only its operation.
    ///
    /// A fence that fails latches its own failure (R0.7: containment never fails silently).
    /// `links` are the boundary's, which the fence record carries. `Err` only when the latch
    /// itself fails.
    pub(super) fn contain_failure(
        &self,
        error: RuntimeHostError,
        boundary_instance: Option<InstanceId>,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<Contained> {
        // ISO-2: a Runtime that latched already (an owner retained for unconfirmed resources
        // no single instance owns, `retain_owner_unconfirmed`) keeps its first condition; a
        // failure met after it fences nothing.
        if self.fatal.current()?.is_some() {
            return self.latch_failure(error);
        }
        match error.scope_at(boundary_instance) {
            FailureScope::Global => self.latch_failure(error),
            FailureScope::Instance(instance_id) => {
                match self.fence_contained_failure(&error, instance_id, links) {
                    Ok(Some(fence)) => Ok(Contained::Fenced(fence)),
                    Ok(None) => self.latch_failure(error),
                    Err(fence_failure) => self.latch_failure(fence_failure),
                }
            }
            FailureScope::Operation => Ok(Contained::Failed(error)),
        }
    }

    /// Latches the Runtime with `error` (a `Global` failure is fatal).
    fn latch_failure(&self, error: RuntimeHostError) -> RuntimeHostResult<Contained> {
        let error = error.into_fatal();
        self.fatal.mark(error.clone())?;
        Ok(Contained::Latched(error))
    }

    /// ISO-M (#419, R0.5): the instance a request's operation is bound to, which the request
    /// door contains its failure against: the instance of its lease token, else the registered
    /// instance its alias names. `None` for an operation of no single instance, or an alias no
    /// instance is registered under.
    pub(super) fn request_instance(
        &self,
        operation: &RuntimeOperation,
    ) -> RuntimeHostResult<Option<InstanceId>> {
        if let Some(token) = operation.lease_token() {
            return Ok(Some(token.instance_id()));
        }
        let alias = match operation {
            RuntimeOperation::PauseScheduling { scope, .. }
            | RuntimeOperation::ResumeScheduling { scope, .. } => scope.instance_alias(),
            RuntimeOperation::SelfCheckInstance { instance_alias, .. }
            | RuntimeOperation::DrillRecoveryLadder { instance_alias, .. } => {
                Some(instance_alias.as_str())
            }
            operation => operation.instance_alias(),
        };
        let Some(alias) = alias else {
            return Ok(None);
        };
        Ok(lock(
            &self.registered_instances,
            HostLocation::ReadInstanceRegistry,
        )?
        .values()
        .find(|instance| instance.instance_alias == alias)
        .map(|instance| instance.instance_id))
    }

    /// ISO-2b (#419 R1; outcome-code spec §7, amendment 10-24: a per-instance unit that dies
    /// again before it finishes one unit of work stops only its instance): a capture of
    /// `instance` failed with `error`; a death of its device session is one of
    /// [`SESSION_DEATH_CODES`]. Returns whether this is the session's second death in a row,
    /// that is, no capture of the instance succeeded since its previous death
    /// ([`Self::session_alive`]). The second death ends the count: its caller fences the
    /// instance, and the next death after its recovery is a first one again. Any other failure
    /// is no death and changes nothing.
    pub(super) fn session_died_again(
        &self,
        instance: InstanceId,
        error: &ExecutionKernelError,
        operation: HostLocation,
    ) -> RuntimeHostResult<bool> {
        if !spelled(error.code().as_str(), SESSION_DEATH_CODES) {
            return Ok(false);
        }
        let mut deaths = lock(&self.session_deaths, operation)?;
        if deaths.remove(&instance) {
            return Ok(true);
        }
        deaths.insert(instance);
        Ok(false)
    }

    /// ISO-2b (#419 R1): a capture of `instance` succeeded, so its session finished a unit of
    /// work; the instance's next session death is a first one.
    pub(super) fn session_alive(
        &self,
        instance: InstanceId,
        operation: HostLocation,
    ) -> RuntimeHostResult<()> {
        lock(&self.session_deaths, operation)?.remove(&instance);
        Ok(())
    }

    /// ISO-2b (#419 R1): `failure`, a capture of `instance` whose session died for the second
    /// time in a row, holds only that instance. It goes to the one decision point at once
    /// ([`Self::contain_failure`]), whatever boundary its work has: the instance is fenced and the
    /// work ends with the fence's non-fatal failure, the capture's own record and terminal kept;
    /// every other instance goes on. A Runtime that latched already keeps its first condition.
    pub(super) fn contain_session_death(
        &self,
        failure: RequestFailure,
        instance: InstanceId,
        links: EventLinksDraft,
    ) -> RuntimeHostResult<RequestFailure> {
        Ok(
            match self.contain_failure((*failure.error).clone(), Some(instance), links)? {
                Contained::Fenced(fence) | Contained::Failed(fence) => failure.fenced(fence),
                Contained::Latched(error) => RequestFailure {
                    error: Box::new(error),
                    poison_runtime: true,
                    ..failure
                },
            },
        )
    }
}

/// ISO-M (#419, R8): locks `instance_lock`, a lock of `instance` alone that guards no data (its
/// admission guard, its queue-order lock). When a panic left it poisoned, the poison is cleared
/// and the work fails with `instance_lock_poisoned`, an `Instance` failure of `instance`: its
/// boundary fences the instance, and every other instance goes on. A poisoned lock that every
/// instance shares stays `Global` (`lock`).
#[allow(
    dead_code,
    reason = "ISO-3 takes the per-instance locks through it (ISO-M section 8)"
)]
pub(crate) fn lock_instance(
    instance_lock: &Mutex<()>,
    operation: impl Into<Location>,
    instance: InstanceId,
) -> RuntimeHostResult<MutexGuard<'_, ()>> {
    instance_lock.lock().map_err(|_| {
        instance_lock.clear_poison();
        RuntimeHostError::instance_fault(
            HostCode::InstanceLockPoisoned,
            operation,
            RuntimeErrorCode::RuntimeHalted,
            instance,
        )
    })
}

impl RuntimeHost {
    /// ISO-M (#419), tests: [`HostShared::contain_failure`] of `error` at a boundary whose work
    /// met `boundary_instance`.
    #[cfg(test)]
    pub(crate) fn contain_failure_for_test(
        &self,
        error: RuntimeHostError,
        boundary_instance: Option<InstanceId>,
    ) -> RuntimeHostResult<Contained> {
        self.shared_ref(crate::codes::test_spellings::TestLocation::ContainFailureForTest)?
            .contain_failure(error, boundary_instance, EventLinksDraft::default())
    }

    /// ISO-M (#419) R0.7, tests: the next fence of a contained failure fails as a failed ledger
    /// append of its record would.
    #[cfg(test)]
    pub(crate) fn fail_next_fence_for_test(&self) -> RuntimeHostResult<()> {
        self.shared_ref(crate::codes::test_spellings::TestLocation::InjectTestFenceFailure)?
            .fence_failure_for_test
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}
