// SPDX-License-Identifier: AGPL-3.0-only

//! The typed code or location behind a spelling that a [`crate::RuntimeHostError`] relays
//! (Workflow #378 A4).
//!
//! The store, state, policy and kernel errors this host relays still hand their codes and
//! operations over as spellings (the bridges that the A2 train's host and ledger follow-up
//! removes). A receipt's outcome needs the typed code and location, which only a registry gives,
//! so the spelling is looked up in the registries the host can relay from. A spelling no registry
//! names stays unresolved: nothing builds a code or a location from text.

use actingcommand_contract::codes::{
    CandidateProjectionCode, ContractCode, ContractLocation, DeviceSelfCheckFailure,
    DiagnosticEventCode, GovernanceIdentityRefusal, IdentifierIssuanceCode, InstallationCode,
    LabErrorCode, MonitorDecisionCode, PageProjectionCode, PpocrDiagnosticCode,
    ProjectInterfaceCode, ResourceDeclarationCode, RuntimeContractCode, RuntimeErrorCode,
    SanitizationCode, SharedCode, TaskflowCode,
};
use actingcommand_contract::outcome::{Code, Location};

use crate::codes::{HostCode, HostLocation};

/// Finds the first registry entry whose spelling is `$spelling` and returns it through `$into`.
macro_rules! first_registered {
    ($spelling:expr, $into:ident; $($registry:ty),+ $(,)?) => {
        None$(.or_else(|| {
            <$registry>::ALL
                .iter()
                .find(|entry| entry.as_str() == $spelling)
                .map(|entry| entry.$into())
        }))+
    };
}

/// The registered code a relayed spelling names, if a registry the host relays from has it.
pub(crate) fn registered_code(spelling: &str) -> Option<Code> {
    first_registered!(
        spelling, code;
        HostCode,
        RuntimeErrorCode,
        SharedCode,
        RuntimeContractCode,
        ContractCode,
        GovernanceIdentityRefusal,
        DeviceSelfCheckFailure,
        DiagnosticEventCode,
        SanitizationCode,
        ProjectInterfaceCode,
        CandidateProjectionCode,
        MonitorDecisionCode,
        IdentifierIssuanceCode,
        TaskflowCode,
        ResourceDeclarationCode,
        InstallationCode,
        LabErrorCode,
        PpocrDiagnosticCode,
        PageProjectionCode,
        actingcommand_scheduler::codes::SchedulerCode,
        actingcommand_ledger::LedgerCode,
        actingcommand_runtime_state::StateCode,
        actingcommand_artifact_store::StoreCode,
        actingcommand_runtime_database::DatabaseCode,
        actingcommand_execution_kernel::ExecutionKernelCode,
        actingcommand_policy::PolicyCode,
        actingcommand_pack_containment::PackContainmentCode,
        actingcommand_page_detector::PageDetectorCode,
        actingcommand_recognition_pack::RecognitionPackCode,
        actingcommand_host_metrics::codes::HostMetricsCode,
        actingcommand_device::NemuResolutionReason,
    )
}

/// The registered location a relayed operation spelling names, if a registry the host relays
/// from has it.
pub(crate) fn registered_location(spelling: &str) -> Option<Location> {
    first_registered!(
        spelling, location;
        HostLocation,
        ContractLocation,
        actingcommand_ledger::LedgerLocation,
        actingcommand_runtime_state::StateLocation,
        actingcommand_artifact_store::StoreLocation,
        actingcommand_runtime_database::DatabaseLocation,
        actingcommand_execution_kernel::ExecutionKernelLocation,
        actingcommand_host_metrics::codes::HostMetricsLocation,
        actingcommand_device::DeviceLocation,
    )
}
