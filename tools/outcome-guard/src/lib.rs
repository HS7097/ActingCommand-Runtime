// SPDX-License-Identifier: AGPL-3.0-only

//! The outcome-code drift guard (Workflow #378 model section 9).
//!
//! It reads the workspace as source and links no workspace crate. Its checks, and the slice
//! that turns each on:
//!
//! - G1 registries = catalog fragments, both ways (A1);
//! - G2 string channels, G3 `json!` keys, G4 construction, G5 stray literals: every workspace
//!   member starts on [`ALLOW_LIST`]; A2a-A2e remove members as they convert them and name the
//!   channels that stay strings as bridges;
//! - G6 persisted prose (A3) and G10 review (A8) are not implemented yet;
//! - G7 key table and vocabularies = catalog (A1);
//! - G8 naming and prefixes (A1);
//! - G9 the merged file and the released contract (A1).
//!
//! Crate roots come from each manifest (`[lib]`, `[[bin]]`) and Cargo's defaults. Code under
//! `#[cfg(test)]` or `#[cfg(all(test, ...))]`, `#[test]` functions and files reached only through
//! them are skipped. The guard cannot see source pulled in with `include!`, so a non-test
//! `include!` is itself a G2 finding.

pub mod catalog;
pub mod source;

pub use catalog::Catalog;
pub use source::Check;

use source::{ChannelScope, Finding, KeyLine, ModuleDeclaration};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One allow-list entry for G2-G5. A member entry (`item: None`) covers a whole workspace
/// member; an item entry covers one function of one file (a method named `Type::method` or
/// `Trait::method`, a free function by its name), or one struct or variant field of one file,
/// named `Type.field` or `Enum::Variant.field` (a bridge that a later slice removes).
#[derive(Debug, Clone, Copy)]
pub struct AllowEntry {
    pub path: &'static str,
    pub item: Option<&'static str>,
    pub checks: &'static [Check],
    pub reason: &'static str,
}

const CHANNELS: &[Check] = &[Check::G2, Check::G3, Check::G4, Check::G5];

/// The G2-G5 allow list. A1 seeds every workspace member; each A2 part removes the members the
/// Owners line of model section 11 assigns to it when it converts them. A channel of a converted
/// member that another owner's code fills, or that a later slice replaces, stays a string behind
/// a bridge entry (a function or a field) whose reason names the slice that removes it. The two
/// tools, MCP's JSON-RPC `error_response` and the configuration manifest's subsystem names
/// (`apps/actingd/src/config/manifest.rs` `build`, G5) are standing entries (kept after B6).
pub const ALLOW_LIST: &[AllowEntry] = &[
    member(
        "apps/actinglab",
        "A2d lists it until B4 converts the actinglab sites",
    ),
    member("apps/ledger-forensics", "A2c converts actingledger"),
    member("crates/artifact-store", "A2c converts the artifact store"),
    member(
        "crates/device",
        "A2e converts the device crate (mechanical: registry, constructors, literals)",
    ),
    member(
        "crates/execution-kernel",
        "A2d converts the execution kernel",
    ),
    member("crates/host-metrics", "A2b converts host-metrics"),
    member("crates/lab", "A2d lists it until B4 converts the lab sites"),
    member("crates/ledger", "A2c converts the ledger"),
    member("crates/ledger-forensics", "A2c converts ledger-forensics"),
    member(
        "crates/onnx-provider-support",
        "A2d converts onnx-provider-support",
    ),
    member("crates/pack-containment", "A2d converts pack-containment"),
    member("crates/page-detector", "A2d converts the page detector"),
    member("crates/policy", "A2d converts policy"),
    member("crates/recognition", "A2d converts recognition"),
    member("crates/recognition-pack", "A2d converts recognition-pack"),
    member("crates/resource-tooling", "A2d converts resource-tooling"),
    member(
        "crates/runtime-database",
        "A2c converts the runtime database",
    ),
    member("crates/runtime-host", "A2b converts the runtime host"),
    member("crates/runtime-state", "A2c converts the runtime state"),
    member("crates/scheduler", "A2b converts the scheduler"),
    member("crates/selection-policy", "A2d converts selection-policy"),
    member("crates/vision-ffi", "A2d converts vision-ffi"),
    member(
        "providers/ppocr-onnx-json",
        "A2d converts the ppocr provider",
    ),
    member(
        "tools/actinglab-architecture",
        "standing: a test tool, not a result path",
    ),
    member(
        "tools/outcome-guard",
        "standing: the guard itself, not a result path",
    ),
    // A2a part 1 bridges: channels of the contract crate and runtime-client that stay strings
    // until the slice the reason names removes the entry (brief-a2a.md A2a-9).
    bridge(
        "crates/actingcommand-contract/src/agent.rs",
        "AgentSessionResponse.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/agent.rs",
        "AgentSessionResponse::code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/agent.rs",
        "AgentSessionResponse::new",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/emulator.rs",
        "EmulatorVersionEvidence::Unavailable.reason",
        &[Check::G2],
        "A2e: carries the device layer's reason; A2e registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticRecognitionError.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticTerminalData::OperationError.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticTerminalData::TaskError.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticUnexecutedData::Guard.reason",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticUnexecutedPage.reason",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "ArtifactFailureCause.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "ArtifactFailureCause.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "CleanupCauseDraft.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "CleanupCauseRecord.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft.stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord.stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "LifecycleCauseDraft.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "LifecycleCauseRecord.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PerformanceMonitorStatePayload.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyDispatchRejection.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyDispatchRejection.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyFailureRecord.error_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyPlanningSignalEventData.fact_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyPlanningSignalPayload.fact_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyReasonRecord.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PublicPayload.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PublicPayload.policy_signal_fact_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "ResourceAuthoringDraft.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "ResourceAuthoringPayload.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureRecord.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureRecord.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureRecord.stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecyclePhase::InstancePreparationFinished.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecyclePhase::RecoveryRungFinished.reason",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "SchedulingOutcomeDeclaration.designated_operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "TaskGeometryFailure.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "TaskSemanticFact::EntryRecoveryFailed.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "TaskSemanticFact::EntryTargetDisposition.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "TaskSemanticFact::TerminalCommitted.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "TaskSemanticFact::TerminalRejected.reason",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "Wire.error_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "CleanupCauseDraft::code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "CleanupCauseRecord::code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "LifecycleCauseRecord::code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureRecord::code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "ResourceAuthoringPayloadDraft::event",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyPlanningSignalPayload::fact_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PerformanceMonitorStatePayload::failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "ResourceAuthoringPayload::failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PublicPayload::failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft::new",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "CleanupCauseDraft::new",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "LifecycleCauseDraft::new",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureDraft::new",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft::operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord::operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureRecord::operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PublicPayload::policy_signal_fact_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft::stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord::stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureRecord::stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "validate_diagnostic_detail",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "validate_resource_authoring_fields",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "RuntimeLifecycleFailureDraft::with_operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/provider.rs",
        "ProviderNativeFailure.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/recovery_ladder.rs",
        "RecoveryLadderTrigger.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/recovery_ladder.rs",
        "is_preparation_recovery_trigger",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/recovery_ladder.rs",
        "is_stuck_recovery_trigger",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/signature.rs",
        "SignatureLifecyclePredicate.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/signature.rs",
        "SignatureLifecyclePredicate.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/signature.rs",
        "SignatureLifecyclePredicate.stage",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/task_selection.rs",
        "TaskSelectionConfirmation::CaptureFailed.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/task_selection.rs",
        "TaskSelectionConfirmation::GuardFailed.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/game_engine.rs",
        "RuntimeCommand.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/installation.rs",
        "InstallTransitionStatus.failure_code",
        &[Check::G2],
        "A2b: carries the host's code; A2b registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "EnvelopeError.code",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError.code",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LedgerProjection.reason",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "NeedsDetection.reason",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError::new",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError::not_implemented",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError::safety_blocked",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LedgerProjection::skipped",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "NeedsDetection::unresolved_environment",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/page_projection.rs",
        "project_observation",
        &[Check::G3],
        "B4: the Lab page projection's json keys blocked_reason and reason hold registered PageProjectionCode spellings; B4 converts the Lab sites",
    ),
    bridge(
        "crates/actingcommand-contract/src/performance.rs",
        "PerformanceMonitorStateEventData.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/performance/capacity.rs",
        "CapacityNativeCause.operation",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/ppocr_diagnostic.rs",
        "PpocrCpuAssignedNodeDiagnostic.placement_reason",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/primitive.rs",
        "CaptureRequest.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/primitive.rs",
        "SwipeRequest.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/primitive.rs",
        "TapRequest.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/project.rs",
        "ProjectDiagnosticView.code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/proposal.rs",
        "ProposalKind::LanguageExtension.extension_code",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ResourceAuthoringEvent.failure_code",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection.host_code",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection.host_operation",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "SchedulingResumeSelfCheck.failure_code",
        &[Check::G2],
        "A2b: carries the host's code; A2b registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ResourceAuthoringEvent::failure_code",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ContainedTaskCancellationReason::from_failure_code",
        &[Check::G2],
        "A2b: carries the host's code; A2b registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection::host_code",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ResourceAuthoringEvent::new",
        &[Check::G2],
        "A2d: carries the Lab, recognition or vision owners' code; A2d registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection::with_host_failure",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime/lab_operation.rs",
        "LabOperationFailure.code",
        &[Check::G2],
        "A2b: carries the host's code; A2b registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime/material_read.rs",
        "RuntimeMaterialReadFailure.code",
        &[Check::G2],
        "A2b: carries the host's code; A2b registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime/material_read.rs",
        "RuntimeMaterialReadFailure.operation",
        &[Check::G2],
        "A2b: carries the host's code; A2b registers it and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime_fact.rs",
        "ConfigSubsystem.reason",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    bridge(
        "crates/runtime-client/src/client/run_status.rs",
        "ContainedRunTerminal.failure_code",
        &[Check::G2],
        "A3: persisted payload field or record; ledger revision 3 replaces it with an outcome",
    ),
    // A2a part 2 bridges: channels of actingd, actingctl and vision-provider-check that stay
    // strings until the slice the reason names removes the entry (brief-a2a.md A2a-9).
    bridge(
        "apps/actingd/src/main.rs",
        "ErrorCode::Host.host_code",
        &[Check::G2],
        "A2b: the runtime host's code inside an actingd error; A2b registers the host's codes",
    ),
    bridge(
        "apps/actingd/src/check_config.rs",
        "run",
        &[Check::G3],
        "A6a: check-config v1 JSON key; check-config v2 replaces it",
    ),
    bridge(
        "apps/actingd/src/check_config.rs",
        "mumu_root_report",
        &[Check::G3, Check::G5],
        "A6a: check-config v1 mumu_root_unresolved, with the provider native failure family <category>.<stage> and its fallback device_error (decision D6); A6a removes it",
    ),
    bridge(
        "apps/actingd/src/check_config.rs",
        "summarize",
        &[Check::G5],
        "A6a: check-config v1's instance key resource_package equals the check-config stage of that spelling (decision D12); A6a rewrites this v1 report builder for check-config v2 and removes the entry",
    ),
    bridge(
        "apps/actingd/src/config.rs",
        "ResourcePackageRejection::detail",
        &[Check::G3],
        "A6a: check-config v1 JSON key loader_code; check-config v2 makes the loader code a link",
    ),
    bridge(
        "apps/actingd/src/config/ac_adb.rs",
        "pinned_binaries",
        &[Check::G2],
        "A6a: prose about the embedded pin, reported only through check-config v1 and the FATAL line",
    ),
    bridge(
        "apps/actingd/src/config/manifest.rs",
        "subsystem",
        &[Check::G2],
        "A3: config_manifest.subsystems[].reason prose; A3 replaces it",
    ),
    bridge(
        "apps/actingd/src/config/provider_startup.rs",
        "bind_discovered_instances",
        &[Check::G5],
        "A3: the provider native failure family <category>.<stage> and its fallback device_error (decision D6); A3's provider_native_failed replaces it",
    ),
    bridge(
        "apps/actingd/src/owner_unlock.rs",
        "failed",
        &[Check::G3],
        "A6a: unlock-owner v1 JSON key; A6a's outcome replaces error {code, stage}",
    ),
    bridge(
        "apps/actingd/src/suspended.rs",
        "run",
        &[Check::G3],
        "A6a: suspended v1 JSON key; A6a's outcome replaces code and operation",
    ),
    bridge(
        "apps/actingctl/src/mcp/lab.rs",
        "binding_draft_result",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/lab.rs",
        "check_config",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/lab.rs",
        "trimmed_lab_error",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "config",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "flag",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "parse_tiers",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "print_text",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "registered_program",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "serve",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "value",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/observer.rs",
        "install_view",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/observer.rs",
        "instance_row",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/observer.rs",
        "overview",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runs.rs",
        "finish_suspension_report",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runs.rs",
        "start_suspension_report",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runtime.rs",
        "client_error",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runtime.rs",
        "configured_state_root",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/runtime.rs",
        "read_bounded",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/server.rs",
        "parse_call",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/server.rs",
        "serve",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/server.rs",
        "spawn",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolCode::Relayed.relayed_code",
        &[Check::G2],
        "A5: a code relayed from actinglab, a job's stored error or the Runtime's material read failure, which A5 relays as outcome links; and actingctl's own dynamic family material_<state> (a code carrying the read state, not listed in decision D6), which A5 splits into one fixed code with a material_state value instead of relaying it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::host_code",
        &[Check::G2],
        "A2b: reads the runtime host's code out of the error details; A2b registers the host's codes",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::into_value",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::into_warning",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::to_value",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "events_result",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "invalid_argument",
        &[Check::G2],
        "A5: the prose of an MCP argument refusal; A5 removes the message",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "output_schema",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "pause_input",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/process_probe.rs",
        "pid_alive",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/decide.rs",
        "Decision::key",
        &[Check::G2],
        "A5: the watchdog's decision key string; A5's last_decision {code, values} replaces it (decision D6)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/decide.rs",
        "fatal_stage",
        &[Check::G5],
        "A6a: the FATAL-line scan for the runtime host's restartable stop codes; A6a's terminal-outcome scan replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "detail",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "install",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "misconfigured_report",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "report",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "run_once",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "status",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "uninstall",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "host_probe",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "live_owner",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "owner_blocks",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "parse_host_probe",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "parse_processes",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/powershell.rs",
        "run",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/task.rs",
        "run",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/vision-provider-check/src/main.rs",
        "InvalidModelReport.reason",
        &[Check::G2],
        "A2d: carries vision-ffi's model folder reason; A2d registers it and retypes the field",
    ),
    AllowEntry {
        path: "apps/actingctl/src/mcp/protocol.rs",
        item: Some("error_response"),
        checks: &[Check::G3],
        reason: "standing: JSON-RPC requires error.code and error.message (model section 7.3)",
    },
    AllowEntry {
        path: "apps/actingd/src/config/manifest.rs",
        item: Some("build"),
        checks: &[Check::G5],
        reason: "standing: the configuration manifest's subsystem names are vocabulary tokens, not codes or locations (model section 2.1); the subsystem runtime_fact_snapshot equals the runtime client's location of that spelling",
    },
];

/// One bridge: a function (`Type::method`, `Trait::method` or a free function's name), or a
/// struct or variant field (`Type.field`, `Enum::Variant.field`), whose channel stays a string
/// until the slice its reason names converts it.
const fn bridge(
    path: &'static str,
    item: &'static str,
    checks: &'static [Check],
    reason: &'static str,
) -> AllowEntry {
    AllowEntry {
        path,
        item: Some(item),
        checks,
        reason,
    }
}

const fn member(path: &'static str, reason: &'static str) -> AllowEntry {
    AllowEntry {
        path,
        item: None,
        checks: CHANNELS,
        reason,
    }
}

/// The workspace root of a member at `tools/<name>`.
pub fn workspace_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.to_path_buf())
}

/// One scanned source file.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub member: String,
    /// Relative to the workspace root, with `/`.
    pub path: String,
    pub scan: source::FileScan,
}

/// The workspace as the checks see it: members, catalog and scanned sources.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub members: Vec<String>,
    pub catalog: Result<Catalog, Vec<String>>,
    pub files: Vec<ScannedFile>,
    /// Files that cannot be read, parsed or found.
    pub errors: Vec<String>,
}

/// Reads the catalog and scans every member's non-test sources once.
pub fn load_workspace(root: &Path) -> Workspace {
    let mut errors = Vec::new();
    let members = workspace_members(root).unwrap_or_else(|error| {
        errors.push(error);
        Vec::new()
    });
    let catalog = catalog::load(root, &members);
    let registered = catalog
        .as_ref()
        .map(Catalog::registered_names)
        .unwrap_or_default();
    let mut files = Vec::new();
    for member in &members {
        let channels = !member_fully_allowed(member);
        scan_member(root, member, channels, &registered, &mut files, &mut errors);
    }
    Workspace {
        root: root.to_path_buf(),
        members,
        catalog,
        files,
        errors,
    }
}

/// The merged catalog text, as `outcome-guard merge` writes it.
pub fn merge(root: &Path) -> Result<String, Vec<String>> {
    let members = workspace_members(root).map_err(|error| vec![error])?;
    let catalog = catalog::load(root, &members)?;
    catalog::merged_text(&catalog).map_err(|error| vec![error])
}

/// G1: every registry entry is an active fragment entry of the registry's own crate, with the
/// same category; every active fragment entry is registered; no spelling is registered twice.
pub fn check_registries(workspace: &Workspace) -> Vec<String> {
    let catalog = match &workspace.catalog {
        Ok(catalog) => catalog,
        Err(errors) => return prefixed("G1", errors),
    };
    let mut violations = Vec::new();
    for file in &workspace.files {
        for error in &file.scan.facts.errors {
            violations.push(format!("G1 {}: {error}", file.path));
        }
    }
    let mut places = BTreeMap::<String, String>::new();
    let mut codes = BTreeSet::new();
    let mut locations = BTreeSet::new();
    for file in &workspace.files {
        let owner = catalog.owner_of(&file.member);
        for registry in &file.scan.facts.registries {
            for entry in &registry.entries {
                let place = format!("{}:{}", file.path, entry.line);
                let spelling = &entry.spelling;
                if let Some(first) = places.insert(spelling.clone(), place.clone()) {
                    violations.push(format!(
                        "G1 {place}: {spelling} is registered twice (also at {first})"
                    ));
                }
                let (table, label) = if registry.locations {
                    locations.insert(spelling.clone());
                    (&catalog.locations, "location")
                } else {
                    codes.insert(spelling.clone());
                    (&catalog.codes, "code")
                };
                let Some(listed) = table.get(spelling) else {
                    violations.push(format!(
                        "G1 {place}: {label} {spelling} is in no fragment's {label}s"
                    ));
                    continue;
                };
                if catalog::text(listed, "status") != Some("active") {
                    violations.push(format!("G1 {place}: {label} {spelling} is retired"));
                }
                if catalog::text(listed, "owner") != owner {
                    violations.push(format!(
                        "G1 {place}: {label} {spelling} belongs to fragment owner {}, not the \
                         registry's crate {}",
                        catalog::text(listed, "owner").unwrap_or("?"),
                        file.member
                    ));
                }
                if !registry.locations
                    && catalog::text(listed, "category") != entry.category.as_deref()
                {
                    violations.push(format!(
                        "G1 {place}: code {spelling} has category {} in its registry and {} in \
                         its fragment",
                        entry.category.as_deref().unwrap_or("?"),
                        catalog::text(listed, "category").unwrap_or("?")
                    ));
                }
            }
        }
    }
    for (table, registered, label) in [
        (&catalog.codes, &codes, "code"),
        (&catalog.locations, &locations, "location"),
    ] {
        for (name, entry) in table {
            if catalog::text(entry, "status") == Some("active") && !registered.contains(name) {
                violations.push(format!(
                    "G1 {label} {name} (owner {}) has no registry entry",
                    catalog::text(entry, "owner").unwrap_or("?")
                ));
            }
        }
    }
    violations
}

/// G2-G5: findings outside the allow list.
pub fn check_channels(workspace: &Workspace) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &workspace.files {
        for finding in &file.scan.findings {
            if !finding_allowed(&file.member, &file.path, finding) {
                violations.push(format!(
                    "{:?} {}:{}: {}",
                    finding.check, file.path, finding.line, finding.detail
                ));
            }
        }
    }
    violations
}

/// G7: `outcome_keys!` equals the catalog key table (names, types, record fields; UI-only
/// `setup_`/`console_` keys are ignored), and every vocabulary built from an enum equals its
/// catalog vocabulary.
pub fn check_keys(workspace: &Workspace) -> Vec<String> {
    let catalog = match &workspace.catalog {
        Ok(catalog) => catalog,
        Err(errors) => return prefixed("G7", errors),
    };
    let mut violations = Vec::new();
    let mut tables = Vec::new();
    for file in &workspace.files {
        for table in &file.scan.facts.key_tables {
            if file.member == CONTRACT_MEMBER {
                tables.push(table);
            } else {
                violations.push(format!(
                    "G7 {}: only {CONTRACT_MEMBER} holds the key table",
                    file.path
                ));
            }
        }
    }
    match tables.as_slice() {
        [table] => compare_key_table(catalog, table, &mut violations),
        _ => violations.push(format!(
            "G7 {CONTRACT_MEMBER} must hold exactly one outcome_keys! table, found {}",
            tables.len()
        )),
    }

    let mut built = BTreeMap::<String, String>::new();
    for file in &workspace.files {
        for vocabulary in &file.scan.facts.vocabularies {
            let place = format!("{} enum {}", file.path, vocabulary.name);
            if let Some(first) = built.insert(vocabulary.vocabulary.clone(), place.clone()) {
                violations.push(format!(
                    "G7 {place}: vocabulary {} is already built by {first}",
                    vocabulary.vocabulary
                ));
            }
            compare_tokens(
                catalog,
                &vocabulary.vocabulary,
                &vocabulary.tokens,
                &place,
                &mut violations,
            );
        }
    }
    for (name, entry) in &catalog.vocabularies {
        let Some(origin) = entry.get("source").and_then(|origin| origin.as_object()) else {
            continue;
        };
        let (Some(file), Some(enum_name)) =
            (catalog::text(origin, "file"), catalog::text(origin, "enum"))
        else {
            continue;
        };
        let place = format!("vocabulary {name} source {file} enum {enum_name}");
        let tokens = fs::read_to_string(workspace.root.join(file))
            .map_err(|error| error.to_string())
            .and_then(|text| source::enum_tokens(&text, enum_name));
        match tokens {
            Ok(tokens) => compare_tokens(catalog, name, &tokens, &place, &mut violations),
            Err(error) => violations.push(format!("G7 {place}: {error}")),
        }
    }
    violations
}

/// G8: names, prefixes and reserved prefixes.
pub fn check_naming(workspace: &Workspace) -> Vec<String> {
    match &workspace.catalog {
        Ok(catalog) => catalog::check_naming(catalog),
        Err(errors) => prefixed("G8", errors),
    }
}

/// G9: the committed merged file is the merge, and released settled entries hold.
pub fn check_contract(workspace: &Workspace) -> Vec<String> {
    match &workspace.catalog {
        Ok(catalog) => catalog::check_contract(&workspace.root, catalog),
        Err(errors) => prefixed("G9", errors),
    }
}

const CONTRACT_MEMBER: &str = "crates/actingcommand-contract";

fn compare_key_table(catalog: &Catalog, table: &[KeyLine], violations: &mut Vec<String>) {
    let lines = table
        .iter()
        .map(|line| (line.name.as_str(), line))
        .collect::<BTreeMap<_, _>>();
    if lines.len() != table.len() {
        violations.push("G7 outcome_keys! lists a key twice".to_owned());
    }
    for (name, entry) in &catalog.keys {
        if is_ui_key(name) {
            continue;
        }
        let Some(line) = lines.get(name.as_str()) else {
            violations.push(format!(
                "G7 catalog key {name} is missing from outcome_keys!"
            ));
            continue;
        };
        let kind = entry
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");
        if kind != line.kind {
            violations.push(format!(
                "G7 key {name} is {kind} in the catalog and {} in outcome_keys!",
                line.kind
            ));
        }
        let fields = entry
            .get("fields")
            .and_then(|fields| fields.as_object())
            .map(|fields| {
                fields
                    .iter()
                    .map(|(field, presence)| (field.clone(), presence.as_str() == Some("required")))
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        if fields != line.fields.iter().cloned().collect::<BTreeSet<_>>() {
            violations.push(format!(
                "G7 key {name} has other record fields in the catalog than in outcome_keys!"
            ));
        }
    }
    for name in lines.keys() {
        if !is_ui_key(name) && !catalog.keys.contains_key(*name) {
            violations.push(format!("G7 outcome_keys! key {name} is not in the catalog"));
        }
    }
}

fn compare_tokens(
    catalog: &Catalog,
    vocabulary: &str,
    tokens: &[String],
    place: &str,
    violations: &mut Vec<String>,
) {
    let Some(listed) = catalog
        .vocabularies
        .get(vocabulary)
        .and_then(|entry| entry.get("tokens"))
        .and_then(|tokens| tokens.as_object())
    else {
        violations.push(format!("G7 {place}: no catalog vocabulary {vocabulary}"));
        return;
    };
    let built = tokens.iter().cloned().collect::<BTreeSet<_>>();
    let listed = listed.keys().cloned().collect::<BTreeSet<_>>();
    if built.len() != tokens.len() {
        violations.push(format!("G7 {place}: a token is listed twice"));
    }
    for token in built.difference(&listed) {
        violations.push(format!(
            "G7 {place}: token {token} is not in catalog vocabulary {vocabulary}"
        ));
    }
    for token in listed.difference(&built) {
        violations.push(format!(
            "G7 {place}: catalog vocabulary {vocabulary} token {token} is not in the enum"
        ));
    }
}

fn is_ui_key(name: &str) -> bool {
    name.starts_with("setup_") || name.starts_with("console_")
}

fn prefixed(check: &str, errors: &[String]) -> Vec<String> {
    errors
        .iter()
        .map(|error| format!("{check} catalog: {error}"))
        .collect()
}

fn member_fully_allowed(member: &str) -> bool {
    ALLOW_LIST.iter().any(|entry| {
        entry.item.is_none()
            && entry.path == member
            && CHANNELS.iter().all(|check| entry.checks.contains(check))
    })
}

fn finding_allowed(member: &str, path: &str, finding: &Finding) -> bool {
    ALLOW_LIST.iter().any(|entry| {
        entry.checks.contains(&finding.check)
            && match entry.item {
                None => entry.path == member,
                Some(item) => entry.path == path && finding.item.as_deref() == Some(item),
            }
    })
}

/// The `members` of the root `Cargo.toml`.
pub fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("Cargo.toml: cannot read: {error}"))?;
    let start = manifest
        .find("members = [")
        .ok_or_else(|| "Cargo.toml: no workspace members list".to_owned())?;
    let rest = &manifest[start + "members = [".len()..];
    let end = rest
        .find(']')
        .ok_or_else(|| "Cargo.toml: the members list does not end".to_owned())?;
    Ok(rest[..end]
        .split(',')
        .map(|member| member.trim().trim_matches('"').to_owned())
        .filter(|member| !member.is_empty())
        .collect())
}

fn is_outcome_module(path: &str) -> bool {
    path == "crates/actingcommand-contract/src/outcome.rs"
        || path.starts_with("crates/actingcommand-contract/src/outcome/")
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The crate roots of a member: the `[lib]` and `[[bin]]` paths its manifest names (else
/// `src/lib.rs`), plus `src/main.rs` and the `src/bin` targets Cargo discovers.
fn crate_roots(root: &Path, member: &str, errors: &mut Vec<String>) -> Vec<PathBuf> {
    let directory = root.join(member);
    let manifest = match fs::read_to_string(directory.join("Cargo.toml")) {
        Ok(manifest) => manifest,
        Err(error) => {
            errors.push(format!("{member}/Cargo.toml: cannot read: {error}"));
            return Vec::new();
        }
    };
    let (library, binaries) = manifest_target_paths(&manifest);
    let source = directory.join("src");
    let mut declared = binaries
        .iter()
        .map(|path| directory.join(path))
        .collect::<Vec<_>>();
    if let Some(path) = &library {
        declared.push(directory.join(path));
    }
    for path in &declared {
        if !path.is_file() {
            errors.push(format!(
                "{member}/Cargo.toml: target path {} is not a file",
                relative_path(root, path)
            ));
        }
    }
    let mut roots = declared
        .into_iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    let defaults = if library.is_some() {
        vec![source.join("main.rs")]
    } else {
        vec![source.join("lib.rs"), source.join("main.rs")]
    };
    roots.extend(defaults.into_iter().filter(|path| path.is_file()));
    if let Ok(entries) = fs::read_dir(source.join("bin")) {
        let mut bins = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter_map(|path| {
                if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
                    Some(path)
                } else {
                    Some(path.join("main.rs")).filter(|main| main.is_file())
                }
            })
            .collect::<Vec<_>>();
        bins.sort();
        roots.extend(bins);
    }
    roots
}

/// The `path` of the `[lib]` section and of each `[[bin]]` section of a manifest.
fn manifest_target_paths(manifest: &str) -> (Option<String>, Vec<String>) {
    let mut section = "";
    let mut library = None;
    let mut binaries = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some(value) = line
            .strip_prefix("path")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
        else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_owned();
        match section {
            "[lib]" => library = Some(value),
            "[[bin]]" => binaries.push(value),
            _ => {}
        }
    }
    (library, binaries)
}

/// Walks one member's module tree from its crate roots, skipping test modules.
fn scan_member(
    root: &Path,
    member: &str,
    channels: bool,
    registered: &BTreeSet<String>,
    files: &mut Vec<ScannedFile>,
    errors: &mut Vec<String>,
) {
    let roots = crate_roots(root, member, errors);
    if roots.is_empty() {
        errors.push(format!("{member}: no crate root under src"));
        return;
    }
    let mut seen = BTreeSet::new();
    let mut queue = roots
        .into_iter()
        .map(|path| (path, true))
        .collect::<Vec<_>>();
    while let Some((path, crate_root)) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let relative = relative_path(root, &path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{relative}: cannot read: {error}"));
                continue;
            }
        };
        let scope = channels.then_some(ChannelScope {
            registered,
            outcome_module: is_outcome_module(&relative),
        });
        let scan = match source::scan_file(&text, scope) {
            Ok(scan) => scan,
            Err(error) => {
                errors.push(format!("{relative}: does not parse: {error}"));
                continue;
            }
        };
        for declaration in scan.modules.iter().filter(|declaration| !declaration.test) {
            match module_file(&path, crate_root, declaration) {
                Some(child) => queue.push((child, false)),
                None => errors.push(format!(
                    "{relative}: no file for module {}",
                    declaration.names.join("::")
                )),
            }
        }
        files.push(ScannedFile {
            member: member.to_owned(),
            path: relative,
            scan,
        });
    }
}

/// The file of an out-of-line module declared in `file`.
fn module_file(file: &Path, crate_root: bool, declaration: &ModuleDeclaration) -> Option<PathBuf> {
    let directory = file.parent()?;
    let (name, inline) = declaration.names.split_last()?;
    let mod_rs = file
        .file_name()
        .is_some_and(|file_name| file_name == "mod.rs");
    if let Some(path) = &declaration.path {
        let mut base = directory.to_path_buf();
        if !inline.is_empty() {
            if !crate_root && !mod_rs {
                base.push(file.file_stem()?);
            }
            base.extend(inline);
        }
        let candidate = base.join(path);
        return candidate.is_file().then_some(candidate);
    }
    let mut base = directory.to_path_buf();
    if !crate_root && !mod_rs {
        base.push(file.file_stem()?);
    }
    base.extend(inline);
    [
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}
