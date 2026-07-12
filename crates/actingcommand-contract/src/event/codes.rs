// SPDX-License-Identifier: AGPL-3.0-only

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

macro_rules! closed_code {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        pub enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl $name {
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
    Actingctl => "actingctl",
    Actinglab => "actinglab",
    Runtime => "runtime",
    Scheduler => "scheduler",
    DeviceProxy => "device-proxy",
    Capture => "capture",
    CapturePipeline => "capture-pipeline",
    Recognition => "recognition",
    ResourceTooling => "resource-tooling",
    ArtifactStore => "artifact-store",
    EvidenceExporter => "evidence-exporter",
    GlobalLedger => "global-ledger",
    ProcessTest => "process-test",
});

closed_code!(EventAction {
    RuntimeAction => "runtime.action",
    RuntimeStart => "runtime.start",
    RuntimeTakeover => "runtime.takeover",
    RuntimeStatus => "runtime.status",
    RuntimeQuery => "runtime.query",
    RuntimeReadonlyAdmit => "runtime.readonly_admit",
    RuntimeReadonlyObserve => "runtime.readonly_observe",
    RuntimeCaptureSequence => "runtime.capture_sequence",
    RuntimeDebugPackage => "runtime.debug_package",
    MonitorConfigure => "monitor.configure",
    MonitorClear => "monitor.clear",
    MonitorProbe => "monitor.probe",
    MonitorRecovery => "monitor.recovery",
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
    CaptureObserve => "capture.observe",
    CapturePolicy => "capture.policy",
    CaptureDedup => "capture.dedup",
    CapturePressure => "capture.pressure",
    RecognitionObserve => "recognition.observe",
    ArtifactStore => "artifact.store",
    ArtifactVerify => "artifact.verify",
    ArtifactExport => "artifact.export",
    ResourceAuthoringStart => "resource.authoring_start",
    ResourceDraftBuild => "resource.draft_build",
    ResourceValidation => "resource.validation",
    ResourcePromote => "resource.promote",
    CriticalTest => "critical.test",
    LedgerRecovery => "ledger.recovery",
});

closed_code!(DiagnosticCode {
    RuntimeDiagnostic => "runtime.diagnostic",
    RuntimeOwnerConflict => "runtime.owner_conflict",
    RuntimeProtocolInvalid => "runtime.protocol_invalid",
    LeaseBusy => "lease.busy",
    LeaseCooldown => "lease.cooldown",
    LeaseExpired => "lease.expired",
    LeaseFencingDenied => "lease.fencing_denied",
    LeaseQueueCancelled => "lease.queue_cancelled",
    LeaseQueueExpired => "lease.queue_expired",
    LeaseQueueDisconnected => "lease.queue_disconnected",
    BackendOpenFailed => "backend.open_failed",
    BackendOperationFailed => "backend.operation_failed",
    CaptureFailed => "capture.failed",
    ArtifactWriteFailed => "artifact.write_failed",
    ArtifactVerifyFailed => "artifact.verify_failed",
    ArtifactExportFailed => "artifact.export_failed",
    PinnedFrameMissing => "artifact.pinned_frame_missing",
    RecognitionFailed => "recognition.failed",
    InputFailed => "input.failed",
    CommandRejected => "command.rejected",
});

closed_code!(RecognitionVerdict {
    FrameDecoded => "frame_decoded",
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

closed_code!(EvidenceCompleteness {
    Complete => "complete",
    Partial => "partial",
    Failed => "failed",
});

closed_code!(RecoveryReason {
    StaleOwner => "stale_owner",
    TruncatedFinalTail => "truncated_final_tail",
});

closed_code!(ResourceAuthoringPhase {
    AuthoringStarted => "authoring_started",
    DraftBuilt => "draft_built",
    ValidationCompleted => "validation_completed",
    PromoteIntent => "promote_intent",
    Promoted => "promoted",
    PromoteFailed => "promote_failed",
});
