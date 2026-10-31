// SPDX-License-Identifier: AGPL-3.0-only

//! Resident Runtime ownership, local IPC, lease-gated DeviceProxy, and lifecycle control.
//!
//! The UI and other clients do not own this process. They communicate through the typed
//! loopback API, and upstream/device implementations stay behind the backend-provider boundary.

#![forbid(unsafe_code)]

mod agent_dispatcher;
// UPD-1: the instance fact `health.application.update_required` and the gate it holds.
mod application_update;
mod approval;
mod catalog_plan;
pub mod codes;
// Workflow #429 R-7a2: the host's side of the one order of the control gates (R-7a §2.6).
mod control_gates;
// Workflow #392 DA-4b-D (Alice's ruling 69): the declared safe areas, read from their directory.
mod display_safe_area;
mod emulator_control;
mod error;
mod events;
mod fact_refresh;
// Workflow #384 D2-S5: the fact refresh's transition recorder.
mod fact_refresh_transitions;
mod fact_store;
mod failure_identity;
mod host;
// IDLE-1 (Alice's ruling 41): the idle planner, pure.
mod idle_action;
mod ipc;
mod ledger_maintenance;
mod monitor;
pub use ledger_maintenance::{
    LedgerMaintenanceFailure, LedgerMaintenanceOperation, LedgerMaintenanceReceipt,
    LedgerMaintenanceRequest, LedgerMaintenanceRestoredMaterial,
};
mod owner;
pub use owner::PriorOwnerReleasedByExit;
mod owner_unlock;
pub use owner_unlock::{OwnerUnlockFailure, OwnerUnlockReceipt};
mod pack_admission;
pub use pack_admission::pack_admission_notes;
mod performance;
mod performance_control;
mod planning;
mod policy_control;
mod policy_host;
mod procedure_manifest;
mod project_interface;
mod proposal;
mod provider;
mod provider_startup;
// Workflow #384 D2-S2.
mod recognition_binding;
mod recovery_read;
mod recovery_timing;
// Workflow #411 B15 R4: the host's side of declared remedies.
mod remedy;
// Workflow #411 B15 R6: the write rule of an instance's remedy choices.
mod remedy_choices;
// Workflow #411 B15 R7: what `actingd check-config` notes about declared remedies.
pub use remedy::{
    RemedyChoiceList, RemedyDeclarationNote, RemedyDeclarationNotes, remedy_declaration_notes,
    remedy_declaration_notes_compiled,
};
// Workflow #384 D2-S3.
mod resource_adjustment;
// SCH-M ED-2: the instance store record `health.run_duration` (the run-duration samples).
mod run_duration;
// Workflow #386 SR-6b, MCP-19a: the latest evaluation's not-run explanation, memory only.
mod schedule_explanation;
mod strategy;
mod suspension_lift;
mod suspension_report;
// Workflow #422 LN-4: whether a start loads the tagged state or seeds it once.
mod tagged_state;
mod time;
// Workflow #406 T12: the landing history of single-region presses (memory only).
mod touch_history;

#[cfg(feature = "test-observation")]
#[doc(hidden)]
pub mod test_observation;

pub use strategy::StrategicPlanPreparation;
pub use time::{RuntimeClock, RuntimeClockSample, SystemRuntimeClock};

pub use actingcommand_scheduler::SchedulerConfig;
// Workflow #383 AO-1: actingd's side of the ledger's write path (`contracts/ledger-observer.md`
// §9.2 step 4).
pub use actingcommand_ledger::LedgerFailureSink;
pub use agent_dispatcher::AgentDispatcherConfig;
pub use catalog_plan::{
    CatalogApprovalPlan, CatalogTransitionPlan, CatalogTransitionPlanKind, CatalogTransitionRequest,
};
pub use codes::{HostCode, HostLocation};
pub use emulator_control::admit_emulator_capabilities;
pub use error::*;
pub use fact_refresh_transitions::FactRefreshTransition;
pub use host::*;
pub use performance::{PerformanceMonitorConfig, PipelinePerformanceSignal};
// Workflow #385 P-m: how the GPU's engines make one utilisation (`PerformanceMonitorConfig`).
pub use actingcommand_host_metrics::GpuAggregate;
pub use performance_control::{
    InstanceArbitrationRank, PerformanceControlConfig, PerformanceControlDirective,
    PerformanceControlObservation, PerformanceControlWorkload,
};
pub use planning::MaintenanceLedgerQuery;
pub use policy_control::PolicyExecutionInput;
pub use policy_host::{
    CatalogGeneration, CatalogPreview, CatalogPreviewPhases, CatalogPreviewRequest,
    PolicyAdmissionContext, PolicyCadence, PolicyCycle, PolicyDispatchAdmission,
    PolicyEvaluationCost, PolicyEvaluationExecution, PolicyEvaluationMeasurement,
    PolicyRecomputeDirective, PolicyRecomputeKind, PolicyRecomputeReason, PolicyRunContext,
    PolicyTrigger, preview_policy_catalog_transition,
};
pub use procedure_manifest::{ProcedureBinding, ProcedureManifest};
pub use provider::*;
pub use provider_startup::ProviderStartup;
pub use recognition_binding::{
    missing_fact_readers, recognition_binding_ignored, recognition_binding_invalid,
    recognition_binding_key_conflict, recognition_binding_qualification,
};
pub use suspension_report::{
    ComposedSuspensionReportRequest, SuspendedReadBudget, SuspensionReportError,
    SuspensionReportRequest, composed_suspension_report, composed_suspension_report_within,
    suspension_report, suspension_report_within,
};

#[cfg(test)]
mod tests;
