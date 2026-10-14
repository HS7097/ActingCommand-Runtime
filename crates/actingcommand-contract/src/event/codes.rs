// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{DiagnosticEventCode, RuntimeErrorCode, SharedCode};
use crate::outcome::Code;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

pub use crate::codes::{DeviceSelfCheckFailure, GovernanceIdentityRefusal};

macro_rules! closed_code {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl $name {
            /// Every token of this set, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct CodeVisitor;

                impl Visitor<'_> for CodeVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a schema-owned code")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($wire => Ok($name::$variant)),+,
                            _ => Err(E::custom("invalid schema-owned code")),
                        }
                    }
                }

                deserializer.deserialize_str(CodeVisitor)
            }
        }
    };
}

closed_code!(OriginModule {
    Provider => "provider",
    Actingctl => "actingctl",
    Actinglab => "actinglab",
    Runtime => "runtime",
    Scheduler => "scheduler",
    Policy => "policy",
    DeviceProxy => "device-proxy",
    Capture => "capture",
    CapturePipeline => "capture-pipeline",
    Recognition => "recognition",
    ResourceTooling => "resource-tooling",
    ArtifactStore => "artifact-store",
    EvidenceExporter => "evidence-exporter",
    GlobalLedger => "global-ledger",
    PerformanceMonitor => "performance-monitor",
    FactStore => "fact-store",
    RuntimeFacts => "runtime-facts",
    Governance => "governance",
    AgentDispatcher => "agent-dispatcher",
    ProcessTest => "process-test",
});

closed_code!(EventAction {
    ProviderStartup => "provider.startup",
    RuntimeAction => "runtime.action",
    RuntimeStart => "runtime.start",
    RuntimeTakeover => "runtime.takeover",
    RuntimeStatus => "runtime.status",
    RuntimeQuery => "runtime.query",
    RuntimeReadonlyAdmit => "runtime.readonly_admit",
    RuntimeReadonlyObserve => "runtime.readonly_observe",
    RuntimeRecognizeArtifact => "runtime.recognize_artifact",
    RuntimeCaptureSequence => "runtime.capture_sequence",
    RuntimeTaskRun => "runtime.task_run",
    RuntimeDebugPackage => "runtime.debug_package",
    RuntimeDebugLabRun => "runtime.debug_lab_run",
    RuntimeDebugObserve => "runtime.debug_observe",
    RuntimeDebugDo => "runtime.debug_do",
    RuntimeDebugEnsure => "runtime.debug_ensure",
    PolicyDispatch => "policy.dispatch",
    PolicyExecution => "policy.execution",
    PolicyPlanning => "policy.planning",
    // Workflow #383 (register L7, L14; D1): the decision-memory and fact-refresh value records.
    PolicyControlState => "policy.control_state",
    PolicyFactRefresh => "policy.fact_refresh",
    CatalogActivate => "catalog.activate",
    CatalogRollback => "catalog.rollback",
    RuntimeDebugWait => "runtime.debug_wait",
    MonitorConfigure => "monitor.configure",
    MonitorClear => "monitor.clear",
    MonitorProbe => "monitor.probe",
    MonitorRecovery => "monitor.recovery",
    PerformanceObserve => "performance.observe",
    FactPublish => "fact.publish",
    FactInvalidate => "fact.invalidate",
    FactSnapshot => "fact.snapshot",
    ClientAction => "client.action",
    ApprovalDecision => "approval.decision",
    GovernanceIdentityDeclare => "governance.identity_declare",
    StateMigrate => "state.migrate",
    ReleaseStage => "release.stage",
    ReleaseActivate => "release.activate",
    ReleaseRollback => "release.rollback",
    AgentWake => "agent.wake",
    AgentSessionStart => "agent.session_start",
    AgentSessionResume => "agent.session_resume",
    AgentSessionRespond => "agent.session_respond",
    AgentSessionComplete => "agent.session_complete",
    AgentSessionEscalate => "agent.session_escalate",
    ProcessAcceptance => "process.acceptance",
    ScheduleAdmit => "schedule.admit",
    LeaseAcquire => "lease.acquire",
    LeaseRenew => "lease.renew",
    LeaseRelease => "lease.release",
    LeaseExpire => "lease.expire",
    InputTap => "input.tap",
    InputLongTap => "input.long_tap",
    InputSwipe => "input.swipe",
    InputKey => "input.key",
    InputText => "input.text",
    InputReset => "input.reset",
    ApplicationLaunch => "application.launch",
    ApplicationStop => "application.stop",
    ApplicationRestart => "application.restart",
    EmulatorInstanceStart => "emulator.instance.start",
    EmulatorInstanceStop => "emulator.instance.stop",
    EmulatorInstanceRestart => "emulator.instance.restart",
    CaptureObserve => "capture.observe",
    CapturePolicy => "capture.policy",
    CaptureDedup => "capture.dedup",
    CapturePressure => "capture.pressure",
    CaptureSummaryCommit => "capture.summary_commit",
    RecognitionObserve => "recognition.observe",
    ArtifactRetention => "artifact.retention",
    ArtifactStore => "artifact.store",
    ArtifactVerify => "artifact.verify",
    ArtifactExport => "artifact.export",
    ResourceAuthoringStart => "resource.authoring_start",
    ResourceDraftBuild => "resource.draft_build",
    ResourceValidation => "resource.validation",
    ResourcePromote => "resource.promote",
    CriticalTest => "critical.test",
    LedgerRecovery => "ledger.recovery",
    // Workflow #383 (register L2): a probe finding of the ledger.
    LedgerProbe => "ledger.probe",
    SignatureRegister => "signature.register",
    SignatureMatch => "signature.match",
    SignatureRetire => "signature.retire",
    OwnerUnlock => "owner.unlock",
    DeviceSelfCheck => "device.self_check",
});

/// The closed set of codes a ledger diagnostic names (Workflow #378 A2a, decision D4). Each
/// variant is one registered contract code and travels as that code's spelling; the set is a
/// closed type, not a registry of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticCode {
    ProviderStartupFailed,
    RuntimeDiagnostic,
    RuntimeOwnerConflict,
    RuntimeProtocolInvalid,
    LeaseBusy,
    LeaseCooldown,
    LeaseExpired,
    LeaseFencingDenied,
    LeaseQueueCancelled,
    LeaseQueueExpired,
    LeaseQueueDisconnected,
    BackendOpenFailed,
    BackendOperationFailed,
    CaptureFailed,
    ArtifactWriteFailed,
    ArtifactVerifyFailed,
    ArtifactExportFailed,
    PinnedFrameMissing,
    RecognitionFailed,
    InputFailed,
    ApplicationFailed,
    CommandRejected,
    PolicyRejected,
    CatalogTransitionFailed,
    ReleaseTransitionFailed,
}

impl DiagnosticCode {
    /// Every diagnostic code, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::ProviderStartupFailed,
        Self::RuntimeDiagnostic,
        Self::RuntimeOwnerConflict,
        Self::RuntimeProtocolInvalid,
        Self::LeaseBusy,
        Self::LeaseCooldown,
        Self::LeaseExpired,
        Self::LeaseFencingDenied,
        Self::LeaseQueueCancelled,
        Self::LeaseQueueExpired,
        Self::LeaseQueueDisconnected,
        Self::BackendOpenFailed,
        Self::BackendOperationFailed,
        Self::CaptureFailed,
        Self::ArtifactWriteFailed,
        Self::ArtifactVerifyFailed,
        Self::ArtifactExportFailed,
        Self::PinnedFrameMissing,
        Self::RecognitionFailed,
        Self::InputFailed,
        Self::ApplicationFailed,
        Self::CommandRejected,
        Self::PolicyRejected,
        Self::CatalogTransitionFailed,
        Self::ReleaseTransitionFailed,
    ];

    /// The registered contract code this variant names.
    pub const fn code(self) -> Code {
        match self {
            Self::ProviderStartupFailed => DiagnosticEventCode::ProviderStartupFailed.code(),
            Self::RuntimeDiagnostic => DiagnosticEventCode::RuntimeDiagnostic.code(),
            Self::RuntimeOwnerConflict => RuntimeErrorCode::OwnerConflict.code(),
            Self::RuntimeProtocolInvalid => RuntimeErrorCode::ProtocolInvalid.code(),
            Self::LeaseBusy => RuntimeErrorCode::LeaseBusy.code(),
            Self::LeaseCooldown => RuntimeErrorCode::LeaseCooldown.code(),
            Self::LeaseExpired => RuntimeErrorCode::LeaseExpired.code(),
            Self::LeaseFencingDenied => DiagnosticEventCode::LeaseFencingDenied.code(),
            Self::LeaseQueueCancelled => DiagnosticEventCode::LeaseQueueCancelled.code(),
            Self::LeaseQueueExpired => SharedCode::LeaseQueueExpired.code(),
            Self::LeaseQueueDisconnected => DiagnosticEventCode::LeaseQueueDisconnected.code(),
            Self::BackendOpenFailed => RuntimeErrorCode::BackendOpenFailed.code(),
            Self::BackendOperationFailed => RuntimeErrorCode::BackendOperationFailed.code(),
            Self::CaptureFailed => RuntimeErrorCode::CaptureFailed.code(),
            Self::ArtifactWriteFailed => SharedCode::ArtifactWriteFailed.code(),
            Self::ArtifactVerifyFailed => SharedCode::ArtifactVerifyFailed.code(),
            Self::ArtifactExportFailed => DiagnosticEventCode::ArtifactExportFailed.code(),
            Self::PinnedFrameMissing => DiagnosticEventCode::ArtifactPinnedFrameMissing.code(),
            Self::RecognitionFailed => RuntimeErrorCode::RecognitionFailed.code(),
            Self::InputFailed => DiagnosticEventCode::InputFailed.code(),
            Self::ApplicationFailed => DiagnosticEventCode::ApplicationFailed.code(),
            Self::CommandRejected => DiagnosticEventCode::CommandRejected.code(),
            Self::PolicyRejected => DiagnosticEventCode::PolicyRejected.code(),
            Self::CatalogTransitionFailed => DiagnosticEventCode::CatalogTransitionFailed.code(),
            Self::ReleaseTransitionFailed => DiagnosticEventCode::ReleaseTransitionFailed.code(),
        }
    }

    /// The registered spelling, which is also the wire spelling.
    pub const fn as_str(self) -> &'static str {
        self.code().as_str()
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for DiagnosticCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DiagnosticCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DiagnosticCodeVisitor;

        impl Visitor<'_> for DiagnosticCodeVisitor {
            type Value = DiagnosticCode;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a registered diagnostic code")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                DiagnosticCode::ALL
                    .iter()
                    .copied()
                    .find(|code| code.as_str() == value)
                    .ok_or_else(|| E::custom("invalid schema-owned code"))
            }
        }

        deserializer.deserialize_str(DiagnosticCodeVisitor)
    }
}

// Workflow #317 sc3: one `device.self_check` status hint per recorded backend open.
closed_code!(DeviceSelfCheckEntry {
    Input => "input",
    Capture => "capture",
    Nemu => "nemu",
});

closed_code!(DeviceSelfCheckStatus {
    Passed => "passed",
    Failed => "failed",
    Unknown => "unknown",
});

closed_code!(RecognitionVerdict {
    FrameDecoded => "frame_decoded",
    PageMatched => "page_matched",
    PageUnmatched => "page_unmatched",
});

closed_code!(CapturePressureState {
    Tier1Dedup => "tier1_dedup",
    Tier2Flush => "tier2_flush",
    Tier3Paused => "tier3_paused",
    Tier3Resumed => "tier3_resumed",
});

closed_code!(CapturePolicyReason {
    Default => "default",
    RequestOverride => "request_override",
    PressureRecovery => "pressure_recovery",
});

closed_code!(PinnedFrameReason {
    PreInput => "pre_input",
    PostInput => "post_input",
    RecognitionEvidence => "recognition_evidence",
    StateTransition => "state_transition",
    Failure => "failure",
    Fallback => "fallback",
    GuardRejection => "guard_rejection",
    Terminal => "terminal",
    Explicit => "explicit",
});

closed_code!(TaskOutcome {
    Success => "success",
    Failure => "failure",
    Cancelled => "cancelled",
});

closed_code!(PolicyFailureClass {
    Recoverable => "recoverable",
    Severe => "severe",
});

closed_code!(PolicyFailureDisposition {
    Continue => "continue",
    RetryScheduled => "retry_scheduled",
    PausedTask => "paused_task",
});

closed_code!(PolicyPlanningSignalKind {
    GoalMissed => "goal_missed",
    FeasibilityRed => "feasibility_red",
    DriftPredicted => "drift_predicted",
    TimelineReached => "timeline_reached",
    DetectionReserved => "detection_reserved",
    DetectionQuotaExhausted => "detection_quota_exhausted",
});

closed_code!(EvidenceCompleteness {
    Complete => "complete",
    Partial => "partial",
    Failed => "failed",
});

closed_code!(RecoveryReason {
    StaleOwner => "stale_owner",
    TruncatedFinalTail => "truncated_final_tail",
    StorageCutover => "storage_cutover",
});

closed_code!(ResourceAuthoringPhase {
    AuthoringStarted => "authoring_started",
    DraftBuilt => "draft_built",
    ValidationCompleted => "validation_completed",
    PromoteIntent => "promote_intent",
    Promoted => "promoted",
    PromoteFailed => "promote_failed",
});
