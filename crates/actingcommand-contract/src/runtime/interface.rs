// SPDX-License-Identifier: AGPL-3.0-only

//! The unified interface's request table (Workflow #389, model v2.2.3 section 2; slice U-1).
//!
//! Every request reaches the Runtime through one door. Which part of the interface a request
//! belongs to depends only on its kind and, for `PublishFact` and `PublishFacts`, on the family of
//! its fact keys (section 2.6). The table is static, names every kind and has no catch-all row. The
//! door that reads it is later work: U-2 binds each connection to its channel and only marks, U-6
//! refuses by channel and by the installed Lab options (section 4.2). `RuntimeRequest::validate`
//! never consults this table: the host builds internal (Agent, Adapter) lease requests and
//! validates them too (section 3.1), and they never pass the door.

use super::RuntimeOperation;
use crate::codes::ReceiptCode;
use crate::outcome::Code;
use crate::{FactKeyFamily, FactObservation, fact_key_family};
use serde::{Deserialize, Serialize};

/// The channel a connection is bound to (section 3.2): one of the three counters of the general
/// interface, or the Lab channel. Each front end names its own channel when it connects;
/// nothing infers it from a request's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeChannel {
    /// `actingctl`, and the installer and scripts that run it.
    Cli,
    /// `actingctl mcp-serve`.
    Mcp,
    /// The console.
    Ui,
    /// The Lab module, `actinglab`.
    Lab,
}

/// The counter that handed a Lab request over to the Lab module (section 3.2 rule 7, section 5.2).
/// Only the forwarding counter program sets it, and only on the Lab channel; a Lab module run
/// directly has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelVia {
    Mcp,
    Ui,
}

impl crate::outcome::VocabularyToken for RuntimeChannel {
    const VOCABULARY: &'static str = "channel";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

impl RuntimeChannel {
    /// The channel's spelling on the wire and in the ledger.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Ui => "ui",
            Self::Lab => "lab",
        }
    }
}

/// What one connection is bound to (section 3.2; U-2): its channel and, on the Lab channel only,
/// the counter that forwarded the work. The Runtime keeps it in memory for the connection's
/// lifetime and tags every request of the connection with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelBinding {
    pub channel: RuntimeChannel,
    pub via: Option<ChannelVia>,
}

impl ChannelBinding {
    /// A binding without `via`.
    pub const fn new(channel: RuntimeChannel) -> Self {
        Self { channel, via: None }
    }

    /// Whether `BindChannel` may carry this binding: `via` only on the Lab channel (section 3.2
    /// rule 7).
    pub const fn is_valid(self) -> bool {
        self.via.is_none() || matches!(self.channel, RuntimeChannel::Lab)
    }
}

/// One of the two Lab options of the installer page (section 4): option one, authoring, and option
/// two, debugging. They are independent; Lab is installed when either is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabOption {
    Authoring,
    Debugging,
}

/// The block a request kind belongs to (section 2.1): the only column the door decides by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceBlock {
    /// Accepted on every connection, bound or not, Lab installed or not.
    Handshake,
    /// What normal operation needs: the three counters accept it whether or not Lab is
    /// installed.
    General,
    /// What one Lab option opens, on the Lab channel only.
    Lab(LabOption),
    /// Accepted from no channel any more.
    Retired,
    /// Kept for older peers only.
    Compat,
}

/// What a request does (section 2.2). Descriptive only: the block decides whether it is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceClass {
    Read,
    Instruction,
    Operation,
}

/// One row of the request table (section 2.2, section 2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InterfaceClassification {
    pub block: InterfaceBlock,
    /// `None` for the handshake, which has no class.
    pub class: Option<InterfaceClass>,
    /// Whether the ledger's write-request boundary G14 (#383) records the request, and with
    /// it a refusal of it. A pure query is answered without any record.
    pub g14_copied: bool,
}

impl InterfaceClassification {
    const fn new(block: InterfaceBlock, class: Option<InterfaceClass>, g14_copied: bool) -> Self {
        Self {
            block,
            class,
            g14_copied,
        }
    }
}

const HANDSHAKE: InterfaceClassification =
    InterfaceClassification::new(InterfaceBlock::Handshake, None, false);
const GENERAL_QUERY: InterfaceClassification =
    InterfaceClassification::new(InterfaceBlock::General, Some(InterfaceClass::Read), false);
const GENERAL_RECORDED_READ: InterfaceClassification =
    InterfaceClassification::new(InterfaceBlock::General, Some(InterfaceClass::Read), true);
const GENERAL_INSTRUCTION: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::General,
    Some(InterfaceClass::Instruction),
    true,
);
const AUTHORING_QUERY: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Lab(LabOption::Authoring),
    Some(InterfaceClass::Read),
    false,
);
const AUTHORING_RECORDED_READ: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Lab(LabOption::Authoring),
    Some(InterfaceClass::Read),
    true,
);
const AUTHORING_INSTRUCTION: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Lab(LabOption::Authoring),
    Some(InterfaceClass::Instruction),
    true,
);
const AUTHORING_OPERATION: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Lab(LabOption::Authoring),
    Some(InterfaceClass::Operation),
    true,
);
const DEBUGGING_OPERATION: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Lab(LabOption::Debugging),
    Some(InterfaceClass::Operation),
    true,
);
const RETIRED_INSTRUCTION: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Retired,
    Some(InterfaceClass::Instruction),
    false,
);
const COMPAT_INSTRUCTION: InterfaceClassification = InterfaceClassification::new(
    InterfaceBlock::Compat,
    Some(InterfaceClass::Instruction),
    true,
);

macro_rules! runtime_operation_kinds {
    ($($kind:ident),+ $(,)?) => {
        /// The kind of a [`RuntimeOperation`] without its payload, named as its variant: the
        /// key of the request table.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum RuntimeOperationKind {
            $($kind),+
        }

        impl RuntimeOperationKind {
            /// Every kind, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$kind),+];
        }
    };
}

runtime_operation_kinds! {
    Health,
    RequestShutdown,
    InstallTransition,
    Status,
    ProjectInterface,
    ProjectPolicyInputIdentity,
    MonitorStatus,
    RuntimeFactSnapshot,
    InstanceFactSnapshot,
    ConfigureMonitor,
    ClearMonitor,
    AcquireLease,
    QueueLease,
    PollQueuedLease,
    CancelQueuedLease,
    CancelContainedTask,
    RenewLease,
    ReleaseLease,
    ObserveReadonly,
    RecognizeArtifact,
    ObserveContainedPage,
    RunContainedLabOperation,
    CaptureSequence,
    SafeReset,
    ApplicationLifecycle,
    ControlEmulatorInstance,
    DiscoverInstances,
    PauseScheduling,
    ResumeScheduling,
    SelfCheckInstance,
    RunContainedTask,
    Input,
    PublishFact,
    PublishFacts,
    QueryEvents,
    ReadMaterial,
    SubscribeEvents,
    RegisterDiagnosticSignature,
    MatchDiagnosticSignatures,
    RetireDiagnosticSignature,
    DebugPackage,
    ExportEvidence,
    RecordAuthoringEvent,
    RecordDebugEvent,
    ReleaseLabPin,
    RecordClientAction,
    DeclareGovernanceIdentity,
    RecordApprovalDecision,
    StartAgentSession,
    ResumeAgentSession,
    AgentSessionStatus,
    RecordAgentResponse,
    PrepareStrategicReport,
    ProjectPolicyForward,
    AssessPredictiveMaintenance,
    CompileProposal,
    PromoteProposal,
    ApplyResourceTargets,
    ResourceTargetView,
    ConfigureSettings,
    BindChannel,
    UpgradeState,
    UpgradeStateStatus,
    LoadPack,
    UnloadPack,
}

impl RuntimeOperationKind {
    /// The kind's row of the request table: section 2.2 for the kinds of v0.11.6, section 2.3 for
    /// the new ones, with the v2.2.2 correction (section 14.2): writing resource targets and every
    /// fact key family are general instructions. `PublishFact` and `PublishFacts` take their block
    /// from their keys' family ([`RuntimeOperation::interface_class`]); every family is general.
    pub const fn interface_class(self) -> InterfaceClassification {
        match self {
            // The handshake: `Health` and `BindChannel`; the identity card retires at U-6.
            Self::Health | Self::DeclareGovernanceIdentity | Self::BindChannel => HANDSHAKE,
            // General reads answered without any record.
            Self::Status
            | Self::MonitorStatus
            | Self::RuntimeFactSnapshot
            | Self::InstanceFactSnapshot
            | Self::ProjectInterface
            | Self::QueryEvents
            | Self::SubscribeEvents
            | Self::ReadMaterial
            | Self::ResourceTargetView
            | Self::UpgradeStateStatus => GENERAL_QUERY,
            // General reads that record: looking at frames, instance discovery.
            Self::ObserveReadonly | Self::CaptureSequence | Self::DiscoverInstances => {
                GENERAL_RECORDED_READ
            }
            // General instructions: the Runtime itself does the work.
            Self::PauseScheduling
            | Self::ResumeScheduling
            | Self::ConfigureMonitor
            | Self::ClearMonitor
            | Self::RequestShutdown
            | Self::RecordClientAction
            | Self::PublishFact
            | Self::PublishFacts
            | Self::ApplyResourceTargets
            | Self::ConfigureSettings
            | Self::UpgradeState => GENERAL_INSTRUCTION,
            // Option one, authoring: the agent planning session and the packaging tools.
            Self::StartAgentSession
            | Self::ResumeAgentSession
            | Self::RecordAgentResponse
            | Self::PromoteProposal
            | Self::PrepareStrategicReport
            | Self::AssessPredictiveMaintenance => AUTHORING_INSTRUCTION,
            Self::ProjectPolicyForward
            | Self::ObserveContainedPage
            | Self::RecognizeArtifact
            | Self::MatchDiagnosticSignatures
            | Self::DebugPackage
            | Self::ExportEvidence => AUTHORING_RECORDED_READ,
            Self::AgentSessionStatus | Self::ProjectPolicyInputIdentity | Self::CompileProposal => {
                AUTHORING_QUERY
            }
            Self::RecordAuthoringEvent => AUTHORING_OPERATION,
            // Option two, debugging: what only the Runtime itself could do before.
            Self::RunContainedTask
            | Self::Input
            | Self::AcquireLease
            | Self::QueueLease
            | Self::PollQueuedLease
            | Self::RenewLease
            | Self::ReleaseLease
            | Self::CancelQueuedLease
            | Self::SafeReset
            | Self::ApplicationLifecycle
            | Self::RunContainedLabOperation
            | Self::CancelContainedTask
            | Self::ControlEmulatorInstance
            | Self::SelfCheckInstance
            | Self::RegisterDiagnosticSignature
            | Self::RetireDiagnosticSignature
            | Self::RecordDebugEvent
            | Self::ReleaseLabPin
            | Self::LoadPack
            | Self::UnloadPack => DEBUGGING_OPERATION,
            // The foreign approval request: approvals stay in the configuration.
            Self::RecordApprovalDecision => RETIRED_INSTRUCTION,
            // install-control revision 1, until the upgrade state replaces it.
            Self::InstallTransition => COMPAT_INSTRUCTION,
        }
    }
}

impl RuntimeOperationKind {
    /// Workflow #378 A4 (model section 7.2, review L1): the success code a completed receipt of
    /// this kind carries, one per kind. A kind that never answers completed has none: its
    /// receipt carries the info code of its state instead (a granted lease or an accepted
    /// shutdown is admitted, a waiting lease is queued, a cancelled queue entry is cancelled, a
    /// contained page observation is observed). The kinds of section 2.3 get theirs from the
    /// slices that add their behaviour: `BindChannel` from U-2; the other four from U-3 and U-4,
    /// refused until then.
    pub const fn success_code(self) -> Option<Code> {
        match self {
            Self::Health => Some(ReceiptCode::HealthCompleted.code()),
            Self::InstallTransition => Some(ReceiptCode::InstallTransitionCompleted.code()),
            Self::Status => Some(ReceiptCode::StatusCompleted.code()),
            Self::ProjectInterface => Some(ReceiptCode::ProjectInterfaceCompleted.code()),
            Self::ProjectPolicyInputIdentity => {
                Some(ReceiptCode::ProjectPolicyInputIdentityCompleted.code())
            }
            Self::MonitorStatus => Some(ReceiptCode::MonitorStatusCompleted.code()),
            Self::RuntimeFactSnapshot => Some(ReceiptCode::RuntimeFactSnapshotCompleted.code()),
            Self::InstanceFactSnapshot => Some(ReceiptCode::InstanceFactSnapshotCompleted.code()),
            Self::ConfigureMonitor => Some(ReceiptCode::ConfigureMonitorCompleted.code()),
            Self::ClearMonitor => Some(ReceiptCode::ClearMonitorCompleted.code()),
            Self::CancelContainedTask => Some(ReceiptCode::CancelContainedTaskCompleted.code()),
            Self::RenewLease => Some(ReceiptCode::RenewLeaseCompleted.code()),
            Self::ReleaseLease => Some(ReceiptCode::ReleaseLeaseCompleted.code()),
            Self::ObserveReadonly => Some(ReceiptCode::ObserveReadonlyCompleted.code()),
            Self::RecognizeArtifact => Some(ReceiptCode::RecognizeArtifactCompleted.code()),
            Self::RunContainedLabOperation => {
                Some(ReceiptCode::RunContainedLabOperationCompleted.code())
            }
            Self::CaptureSequence => Some(ReceiptCode::CaptureSequenceCompleted.code()),
            Self::SafeReset => Some(ReceiptCode::SafeResetCompleted.code()),
            Self::ApplicationLifecycle => Some(ReceiptCode::ApplicationLifecycleCompleted.code()),
            Self::ControlEmulatorInstance => {
                Some(ReceiptCode::ControlEmulatorInstanceCompleted.code())
            }
            Self::DiscoverInstances => Some(ReceiptCode::DiscoverInstancesCompleted.code()),
            Self::PauseScheduling => Some(ReceiptCode::PauseSchedulingCompleted.code()),
            Self::ResumeScheduling => Some(ReceiptCode::ResumeSchedulingCompleted.code()),
            Self::SelfCheckInstance => Some(ReceiptCode::SelfCheckInstanceCompleted.code()),
            Self::RunContainedTask => Some(ReceiptCode::RunContainedTaskCompleted.code()),
            Self::Input => Some(ReceiptCode::InputCompleted.code()),
            Self::PublishFact => Some(ReceiptCode::PublishFactCompleted.code()),
            Self::PublishFacts => Some(ReceiptCode::PublishFactsCompleted.code()),
            Self::QueryEvents => Some(ReceiptCode::QueryEventsCompleted.code()),
            Self::ReadMaterial => Some(ReceiptCode::ReadMaterialCompleted.code()),
            Self::SubscribeEvents => Some(ReceiptCode::SubscribeEventsCompleted.code()),
            Self::RegisterDiagnosticSignature => {
                Some(ReceiptCode::RegisterDiagnosticSignatureCompleted.code())
            }
            Self::MatchDiagnosticSignatures => {
                Some(ReceiptCode::MatchDiagnosticSignaturesCompleted.code())
            }
            Self::RetireDiagnosticSignature => {
                Some(ReceiptCode::RetireDiagnosticSignatureCompleted.code())
            }
            Self::DebugPackage => Some(ReceiptCode::DebugPackageCompleted.code()),
            Self::ExportEvidence => Some(ReceiptCode::ExportEvidenceCompleted.code()),
            Self::RecordAuthoringEvent => Some(ReceiptCode::RecordAuthoringEventCompleted.code()),
            Self::RecordDebugEvent => Some(ReceiptCode::RecordDebugEventCompleted.code()),
            Self::ReleaseLabPin => Some(ReceiptCode::ReleaseLabPinCompleted.code()),
            Self::RecordClientAction => Some(ReceiptCode::RecordClientActionCompleted.code()),
            Self::DeclareGovernanceIdentity => {
                Some(ReceiptCode::DeclareGovernanceIdentityCompleted.code())
            }
            Self::RecordApprovalDecision => {
                Some(ReceiptCode::RecordApprovalDecisionCompleted.code())
            }
            Self::StartAgentSession => Some(ReceiptCode::StartAgentSessionCompleted.code()),
            Self::ResumeAgentSession => Some(ReceiptCode::ResumeAgentSessionCompleted.code()),
            Self::AgentSessionStatus => Some(ReceiptCode::AgentSessionStatusCompleted.code()),
            Self::RecordAgentResponse => Some(ReceiptCode::RecordAgentResponseCompleted.code()),
            Self::PrepareStrategicReport => {
                Some(ReceiptCode::PrepareStrategicReportCompleted.code())
            }
            Self::ProjectPolicyForward => Some(ReceiptCode::ProjectPolicyForwardCompleted.code()),
            Self::AssessPredictiveMaintenance => {
                Some(ReceiptCode::AssessPredictiveMaintenanceCompleted.code())
            }
            Self::CompileProposal => Some(ReceiptCode::CompileProposalCompleted.code()),
            Self::PromoteProposal => Some(ReceiptCode::PromoteProposalCompleted.code()),
            Self::ApplyResourceTargets => Some(ReceiptCode::ApplyResourceTargetsCompleted.code()),
            Self::ResourceTargetView => Some(ReceiptCode::ResourceTargetViewCompleted.code()),
            Self::ConfigureSettings => Some(ReceiptCode::ConfigureSettingsCompleted.code()),
            // Workflow #389 U-2.
            Self::BindChannel => Some(ReceiptCode::BindChannelCompleted.code()),
            Self::RequestShutdown
            | Self::AcquireLease
            | Self::QueueLease
            | Self::PollQueuedLease
            | Self::CancelQueuedLease
            | Self::ObserveContainedPage => None,
            Self::UpgradeState | Self::UpgradeStateStatus | Self::LoadPack | Self::UnloadPack => {
                None
            }
        }
    }
}

impl FactKeyFamily {
    /// The block a publication of this family belongs to (section 2.6 with section 14.2): every
    /// family is a general instruction, which the three counters send.
    pub const fn interface_class(self) -> InterfaceClassification {
        match self {
            Self::PriorityOffset | Self::InstanceInformation => GENERAL_INSTRUCTION,
        }
    }
}

impl RuntimeOperation {
    /// This operation's kind.
    pub const fn kind(&self) -> RuntimeOperationKind {
        use RuntimeOperationKind as Kind;
        match self {
            Self::Health => Kind::Health,
            Self::RequestShutdown { .. } => Kind::RequestShutdown,
            Self::InstallTransition { .. } => Kind::InstallTransition,
            Self::Status => Kind::Status,
            Self::ProjectInterface { .. } => Kind::ProjectInterface,
            Self::ProjectPolicyInputIdentity { .. } => Kind::ProjectPolicyInputIdentity,
            Self::MonitorStatus => Kind::MonitorStatus,
            Self::RuntimeFactSnapshot => Kind::RuntimeFactSnapshot,
            Self::InstanceFactSnapshot { .. } => Kind::InstanceFactSnapshot,
            Self::ConfigureMonitor { .. } => Kind::ConfigureMonitor,
            Self::ClearMonitor { .. } => Kind::ClearMonitor,
            Self::AcquireLease { .. } => Kind::AcquireLease,
            Self::QueueLease { .. } => Kind::QueueLease,
            Self::PollQueuedLease { .. } => Kind::PollQueuedLease,
            Self::CancelQueuedLease { .. } => Kind::CancelQueuedLease,
            Self::CancelContainedTask { .. } => Kind::CancelContainedTask,
            Self::RenewLease { .. } => Kind::RenewLease,
            Self::ReleaseLease { .. } => Kind::ReleaseLease,
            Self::ObserveReadonly { .. } => Kind::ObserveReadonly,
            Self::RecognizeArtifact { .. } => Kind::RecognizeArtifact,
            Self::ObserveContainedPage { .. } => Kind::ObserveContainedPage,
            Self::RunContainedLabOperation { .. } => Kind::RunContainedLabOperation,
            Self::CaptureSequence { .. } => Kind::CaptureSequence,
            Self::SafeReset { .. } => Kind::SafeReset,
            Self::ApplicationLifecycle { .. } => Kind::ApplicationLifecycle,
            Self::ControlEmulatorInstance { .. } => Kind::ControlEmulatorInstance,
            Self::DiscoverInstances => Kind::DiscoverInstances,
            Self::PauseScheduling { .. } => Kind::PauseScheduling,
            Self::ResumeScheduling { .. } => Kind::ResumeScheduling,
            Self::SelfCheckInstance { .. } => Kind::SelfCheckInstance,
            Self::RunContainedTask { .. } => Kind::RunContainedTask,
            Self::Input { .. } => Kind::Input,
            Self::PublishFact { .. } => Kind::PublishFact,
            Self::PublishFacts { .. } => Kind::PublishFacts,
            Self::QueryEvents { .. } => Kind::QueryEvents,
            Self::ReadMaterial { .. } => Kind::ReadMaterial,
            Self::SubscribeEvents { .. } => Kind::SubscribeEvents,
            Self::RegisterDiagnosticSignature { .. } => Kind::RegisterDiagnosticSignature,
            Self::MatchDiagnosticSignatures { .. } => Kind::MatchDiagnosticSignatures,
            Self::RetireDiagnosticSignature { .. } => Kind::RetireDiagnosticSignature,
            Self::DebugPackage { .. } => Kind::DebugPackage,
            Self::ExportEvidence { .. } => Kind::ExportEvidence,
            Self::RecordAuthoringEvent { .. } => Kind::RecordAuthoringEvent,
            Self::RecordDebugEvent { .. } => Kind::RecordDebugEvent,
            Self::ReleaseLabPin { .. } => Kind::ReleaseLabPin,
            Self::RecordClientAction { .. } => Kind::RecordClientAction,
            Self::DeclareGovernanceIdentity { .. } => Kind::DeclareGovernanceIdentity,
            Self::RecordApprovalDecision { .. } => Kind::RecordApprovalDecision,
            Self::StartAgentSession { .. } => Kind::StartAgentSession,
            Self::ResumeAgentSession { .. } => Kind::ResumeAgentSession,
            Self::AgentSessionStatus { .. } => Kind::AgentSessionStatus,
            Self::RecordAgentResponse { .. } => Kind::RecordAgentResponse,
            Self::PrepareStrategicReport { .. } => Kind::PrepareStrategicReport,
            Self::ProjectPolicyForward { .. } => Kind::ProjectPolicyForward,
            Self::AssessPredictiveMaintenance { .. } => Kind::AssessPredictiveMaintenance,
            Self::CompileProposal { .. } => Kind::CompileProposal,
            Self::PromoteProposal { .. } => Kind::PromoteProposal,
            Self::ApplyResourceTargets { .. } => Kind::ApplyResourceTargets,
            Self::ResourceTargetView { .. } => Kind::ResourceTargetView,
            Self::ConfigureSettings { .. } => Kind::ConfigureSettings,
            Self::BindChannel { .. } => Kind::BindChannel,
            Self::UpgradeState { .. } => Kind::UpgradeState,
            Self::UpgradeStateStatus { .. } => Kind::UpgradeStateStatus,
            Self::LoadPack { .. } => Kind::LoadPack,
            Self::UnloadPack { .. } => Kind::UnloadPack,
        }
    }

    /// Workflow #389 section 2: this request's row of the request table. A fact publication takes
    /// the block of its keys' family (section 2.6): `RuntimeRequest::validate` refuses an
    /// observation that mixes families, so the first key decides; every other request takes its
    /// kind's row.
    pub fn interface_class(&self) -> InterfaceClassification {
        if let Self::PublishFact { record } = self {
            return fact_key_family(&record.key).interface_class();
        }
        if let Self::PublishFacts { observation } = self {
            return observation_key_family(observation).interface_class();
        }
        self.kind().interface_class()
    }
}

/// The key family of a publication: its first key's. `FactObservation::validate` refuses an
/// observation without a record; such an observation counts as "every other key".
fn observation_key_family(observation: &FactObservation) -> FactKeyFamily {
    observation
        .records
        .first()
        .map_or(FactKeyFamily::InstanceInformation, |record| {
            fact_key_family(&record.key)
        })
}

#[cfg(test)]
mod tests {
    use super::InterfaceBlock::{Compat, General, Handshake, Lab, Retired};
    use super::InterfaceClass::{Instruction, Operation, Read};
    use super::LabOption::{Authoring, Debugging};
    use super::*;
    use crate::{
        EventActor, EventSource, FactRecord, FactScope, IdentifierIssuer, LeaseToken,
        PackLoadEntry, PackRegistryKey, RuntimeRequest, RuntimeShutdownTarget, UpgradeStateAction,
        priority_offset_fact_key,
    };

    /// TU-1's expected row of each kind, from section 2.2 (the 58 kinds of v0.11.6) and section 2.3
    /// (the new kinds), with section 14.2: `ApplyResourceTargets` and every fact key family are
    /// general.
    fn expected(kind: RuntimeOperationKind) -> (InterfaceBlock, Option<InterfaceClass>, bool) {
        use super::RuntimeOperationKind as K;
        match kind {
            // section 2.2 handshake.
            K::Health => (Handshake, None, false),
            K::DeclareGovernanceIdentity => (Handshake, None, false),
            // section 2.2 general reads.
            K::Status => (General, Some(Read), false),
            K::MonitorStatus => (General, Some(Read), false),
            K::RuntimeFactSnapshot => (General, Some(Read), false),
            // Workflow #389 MCP-4 (register W2): one instance's facts, a general read that
            // records nothing.
            K::InstanceFactSnapshot => (General, Some(Read), false),
            K::ProjectInterface => (General, Some(Read), false),
            K::QueryEvents => (General, Some(Read), false),
            K::SubscribeEvents => (General, Some(Read), false),
            K::ReadMaterial => (General, Some(Read), false),
            K::ResourceTargetView => (General, Some(Read), false),
            K::ObserveReadonly => (General, Some(Read), true),
            K::CaptureSequence => (General, Some(Read), true),
            K::DiscoverInstances => (General, Some(Read), true),
            // section 2.2 general instructions.
            K::PauseScheduling => (General, Some(Instruction), true),
            K::ResumeScheduling => (General, Some(Instruction), true),
            K::ConfigureMonitor => (General, Some(Instruction), true),
            K::ClearMonitor => (General, Some(Instruction), true),
            K::RequestShutdown => (General, Some(Instruction), true),
            K::RecordClientAction => (General, Some(Instruction), true),
            // section 2.2 by key family, every family general since section 14.2.
            K::PublishFact => (General, Some(Instruction), true),
            K::PublishFacts => (General, Some(Instruction), true),
            // section 14.2: writing resource targets is a general instruction.
            K::ApplyResourceTargets => (General, Some(Instruction), true),
            // Workflow #387 D1b (model v1.2 §5.6): changing a setting while running is a general
            // instruction the three counters send, recorded through G14.
            K::ConfigureSettings => (General, Some(Instruction), true),
            // section 2.2 option one, authoring.
            K::StartAgentSession => (Lab(Authoring), Some(Instruction), true),
            K::ResumeAgentSession => (Lab(Authoring), Some(Instruction), true),
            K::RecordAgentResponse => (Lab(Authoring), Some(Instruction), true),
            K::PromoteProposal => (Lab(Authoring), Some(Instruction), true),
            K::PrepareStrategicReport => (Lab(Authoring), Some(Instruction), true),
            K::AssessPredictiveMaintenance => (Lab(Authoring), Some(Instruction), true),
            K::ProjectPolicyForward => (Lab(Authoring), Some(Read), true),
            K::AgentSessionStatus => (Lab(Authoring), Some(Read), false),
            K::ProjectPolicyInputIdentity => (Lab(Authoring), Some(Read), false),
            K::CompileProposal => (Lab(Authoring), Some(Read), false),
            K::ObserveContainedPage => (Lab(Authoring), Some(Read), true),
            K::RecognizeArtifact => (Lab(Authoring), Some(Read), true),
            K::MatchDiagnosticSignatures => (Lab(Authoring), Some(Read), true),
            K::DebugPackage => (Lab(Authoring), Some(Read), true),
            K::ExportEvidence => (Lab(Authoring), Some(Read), true),
            K::RecordAuthoringEvent => (Lab(Authoring), Some(Operation), true),
            // section 2.2 option two, debugging.
            K::RunContainedTask => (Lab(Debugging), Some(Operation), true),
            K::Input => (Lab(Debugging), Some(Operation), true),
            K::AcquireLease => (Lab(Debugging), Some(Operation), true),
            K::QueueLease => (Lab(Debugging), Some(Operation), true),
            K::PollQueuedLease => (Lab(Debugging), Some(Operation), true),
            K::RenewLease => (Lab(Debugging), Some(Operation), true),
            K::ReleaseLease => (Lab(Debugging), Some(Operation), true),
            K::CancelQueuedLease => (Lab(Debugging), Some(Operation), true),
            K::SafeReset => (Lab(Debugging), Some(Operation), true),
            K::ApplicationLifecycle => (Lab(Debugging), Some(Operation), true),
            K::RunContainedLabOperation => (Lab(Debugging), Some(Operation), true),
            K::CancelContainedTask => (Lab(Debugging), Some(Operation), true),
            K::ControlEmulatorInstance => (Lab(Debugging), Some(Operation), true),
            K::SelfCheckInstance => (Lab(Debugging), Some(Operation), true),
            K::RegisterDiagnosticSignature => (Lab(Debugging), Some(Operation), true),
            K::RetireDiagnosticSignature => (Lab(Debugging), Some(Operation), true),
            K::RecordDebugEvent => (Lab(Debugging), Some(Operation), true),
            K::ReleaseLabPin => (Lab(Debugging), Some(Operation), true),
            // section 2.2 retired and compatibility.
            K::RecordApprovalDecision => (Retired, Some(Instruction), false),
            K::InstallTransition => (Compat, Some(Instruction), true),
            // section 2.3 new kinds.
            K::BindChannel => (Handshake, None, false),
            K::UpgradeState => (General, Some(Instruction), true),
            K::UpgradeStateStatus => (General, Some(Read), false),
            K::LoadPack => (Lab(Debugging), Some(Operation), true),
            K::UnloadPack => (Lab(Debugging), Some(Operation), true),
        }
    }

    fn row(
        block: InterfaceBlock,
        class: Option<InterfaceClass>,
        g14_copied: bool,
    ) -> InterfaceClassification {
        InterfaceClassification {
            block,
            class,
            g14_copied,
        }
    }

    #[test]
    fn tu1_every_request_kind_has_its_block_class_and_g14_copy() {
        for kind in RuntimeOperationKind::ALL {
            let (block, class, g14_copied) = expected(*kind);
            assert_eq!(
                kind.interface_class(),
                row(block, class, g14_copied),
                "{kind:?}"
            );
        }

        // The new kinds' operations take their kinds' rows.
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let target = RuntimeShutdownTarget {
            owner_epoch: *ids.mint_owner_epoch().expect("owner epoch").transport(),
            pid: 4_242,
            started_at_unix_ms: 1,
        };
        for (operation, kind) in [
            (
                RuntimeOperation::BindChannel {
                    channel: RuntimeChannel::Lab,
                    via: Some(ChannelVia::Mcp),
                },
                RuntimeOperationKind::BindChannel,
            ),
            (
                RuntimeOperation::UpgradeState {
                    target,
                    action: UpgradeStateAction::Enter {
                        transition_id: "transition-1".to_owned(),
                        timeout_ms: 60_000,
                    },
                },
                RuntimeOperationKind::UpgradeState,
            ),
            (
                RuntimeOperation::UpgradeStateStatus {
                    transition_id: "transition-1".to_owned(),
                },
                RuntimeOperationKind::UpgradeStateStatus,
            ),
            (
                RuntimeOperation::LoadPack {
                    entry: PackLoadEntry::Restore {
                        key: PackRegistryKey::Procedure {
                            package_id: "package.example".to_owned(),
                        },
                    },
                },
                RuntimeOperationKind::LoadPack,
            ),
            (
                RuntimeOperation::UnloadPack {
                    key: PackRegistryKey::StartupPackage {
                        instance_alias: "node.b".to_owned(),
                    },
                    wait_ms: 0,
                },
                RuntimeOperationKind::UnloadPack,
            ),
        ] {
            assert_eq!(operation.kind(), kind);
            let (block, class, g14_copied) = expected(kind);
            assert_eq!(
                operation.interface_class(),
                row(block, class, g14_copied),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn tu1_fact_key_families_are_general_instructions() {
        let offset = priority_offset_fact_key("task.a");
        assert_eq!(fact_key_family(&offset), FactKeyFamily::PriorityOffset);
        assert_eq!(
            fact_key_family(&offset).interface_class(),
            row(General, Some(Instruction), true)
        );
        assert_eq!(
            fact_key_family("display.instance_label"),
            FactKeyFamily::InstanceInformation
        );
        assert_eq!(
            fact_key_family("display.instance_label").interface_class(),
            row(General, Some(Instruction), true)
        );

        let record = FactRecord::priority_offset(
            FactScope::Instance {
                instance_id: "node.b".to_owned(),
            },
            "task.a",
            250,
            1,
            "actingctl.task-offset",
        )
        .expect("priority offset record");
        assert_eq!(
            RuntimeOperation::PublishFact {
                record: record.clone()
            }
            .interface_class(),
            row(General, Some(Instruction), true)
        );
        assert_eq!(
            RuntimeOperation::PublishFacts {
                observation: FactObservation {
                    records: vec![record],
                },
            }
            .interface_class(),
            row(General, Some(Instruction), true)
        );
    }

    #[test]
    fn tu1_host_built_agent_adapter_leases_pass_validate_without_a_channel_judgement() {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let token = LeaseToken::new(
            *ids.mint_owner_epoch().expect("owner epoch").transport(),
            *ids.mint_lease_id().expect("lease id").transport(),
            *ids.mint_instance_id().expect("instance id").transport(),
            *ids.mint_holder_id().expect("holder id").transport(),
            100,
        )
        .expect("lease token");
        for operation in [
            RuntimeOperation::acquire_lease("node.b", ids.mint_holder_id().expect("holder id")),
            RuntimeOperation::ReleaseLease { token },
        ] {
            // Option two, which no counter may send once U-6 closes the door ...
            assert_eq!(operation.interface_class().block, Lab(Debugging));
            // ... and the host's own (Agent, Adapter) request still validates.
            let request = RuntimeRequest::new(
                ids.mint_request_id().expect("request id"),
                ids.mint_correlation_id().expect("correlation id"),
                None,
                EventActor::Agent,
                EventSource::Adapter,
                1,
                operation,
            )
            .expect("host-built request");
            assert!(request.validate().is_ok());
        }
    }
}
