// SPDX-License-Identifier: AGPL-3.0-only

//! Typed local Runtime IPC contract shared by resident hosts and disposable clients.
//!
//! Read-only capture authority can only be issued by `IdentifierIssuer`:
//!
//! ```compile_fail
//! use actingcommand_contract::{InstanceId, OwnerEpoch, ReadOnlyCaptureCapability};
//!
//! fn forge(epoch: OwnerEpoch, instance: InstanceId) {
//!     let _ = ReadOnlyCaptureCapability::new(epoch, instance);
//! }
//! ```

use crate::codes::{ReceiptCode, RuntimeContractCode, SharedCode};
use crate::outcome::{Category, Code, Detail, Location, Outcome, OutcomeEnvelope};
use crate::{
    ActionId, AgentSessionContext, AgentSessionId, AgentSessionResponse, AgentSessionStatus,
    AgentWakeId, ApprovalDecisionRecord, ApprovalDisposition, ArtifactKind, ArtifactLinksDraft,
    ArtifactMediaType, ArtifactRedactionState, CatalogProposal, CausationId, ClientActionRecord,
    CorrelationId, EffectDisposition, EventActor, EventId, EventLinksDraft, EventQuery,
    EventSource, EventType, EvidenceCompleteness, FactRecord, FactScope, FrameId, HolderId,
    IdentifierIssuanceError, IdentifierIssuer, InstanceId, IssuedCausationId, IssuedCorrelationId,
    IssuedFrameId, IssuedHolderId, IssuedRecognitionId, IssuedRequestId, IssuedRunId, IssuedTaskId,
    LeaseId, OwnerEpoch, ProjectInterfaceRequest, ProjectInterfaceResponse,
    ProjectedArtifactReference, ProjectedEvent, ProjectionProfile, ProposalPreview,
    ProposalPromotion, RecognitionId, RecognitionVerdict, RequestId, ResourceAuthoringPhase, RunId,
    RuntimeMonitorInstanceStatus, RuntimeMonitorPolicy, RuntimeMonitorRegistryStatus,
    SubscriptionCursor, TaskOutcome,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

mod online_observation;
pub use online_observation::*;
mod lab_operation;
pub use lab_operation::*;
mod lab_operation_evidence;
pub use lab_operation_evidence::*;
mod saved_artifact_ocr;
pub use saved_artifact_ocr::*;
mod material_read;
pub use material_read::*;
// Workflow #389 U-1: the request table and the package load/unload shapes.
mod interface;
pub use interface::*;
mod pack_registry;
pub use pack_registry::*;
// Workflow #385 P-m: the shadow measurement's status read (register §1.61 W15).
mod performance_status;
pub use performance_status::*;
// Workflow #384 D2-S5: the fact refresh's transition recorder read (register §1.68).
mod fact_refresh_status;
pub use fact_refresh_status::*;
// Workflow #386 SR-6b, MCP-19a: the latest evaluation's not-run explanation (register §1.143).
mod schedule_explanation;
pub use schedule_explanation::*;
use std::net::{IpAddr, SocketAddr};

// Interface anchor `runtime-client` (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when older peers can no longer use these two.
// Revision 2 (Workflow #378 A4 with #389 U-1): request v4 adds the optional `detail`, receipt v2
// carries `outcome` on every receipt instead of the error projection, and the request kinds of
// #389 section 2.3 (`BindChannel`, `UpgradeState`, `UpgradeStateStatus`, `LoadPack`,
// `UnloadPack`) belong to it. Declared once for v0.12.0 as [2, 2] (#378 A8).
pub const RUNTIME_REQUEST_SCHEMA_VERSION: &str = "actingcommand.runtime.request.v4";
pub const RUNTIME_RECEIPT_SCHEMA_VERSION: &str = "actingcommand.runtime.receipt.v2";
pub const RUNTIME_INFO_SCHEMA_VERSION: &str = "actingcommand.runtime.info.v1";
pub const RUNTIME_INFO_FILE: &str = "runtime-info.json";
pub const MAX_INSTANCE_ALIAS_BYTES: usize = 256;
pub const MAX_INPUT_TEXT_BYTES: usize = 4096;
pub const MAX_INPUT_KEY_BYTES: usize = 64;
pub const MAX_INPUT_DURATION_MS: u64 = 60_000;
pub const SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS: u64 = 200;
pub const SEGMENTED_SWIPE_CORNER_HOLD_MS: u64 = 150;
pub const SEGMENTED_SWIPE_BRAKE_DISTANCE_PX: i32 = 100;
pub const SEGMENTED_SWIPE_BRAKE_DURATION_MS: u64 = 200;
pub const SEGMENTED_SWIPE_SLOPE_IN: u8 = 2;
pub const SEGMENTED_SWIPE_SLOPE_OUT: u8 = 0;
/// The most points one single-touch path drag passes through (Workflow #406, register §1.18).
pub const MAX_PATH_DRAG_POINTS: usize = 32;
pub const MAX_LEASE_QUEUE_TIMEOUT_MS: u64 = 3_600_000;
pub const MAX_READONLY_OBSERVATION_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES: u16 = 60;
pub const MAX_RUNTIME_CAPTURE_SEQUENCE_INTERVAL_MS: u64 = 5_000;
pub const MAX_RUNTIME_CAPTURE_SEQUENCE_WAIT_MS: u64 = 60_000;
pub const MAX_DEBUG_PACKAGE_PATH_BYTES: usize = 32 * 1024;
pub const MAX_CONTAINED_TASK_PATH_BYTES: usize = 32 * 1024;
pub const MAX_EVIDENCE_OUTPUT_PATH_BYTES: usize = 32 * 1024;
pub const MAX_RUNTIME_SUBSCRIPTION_WAIT_MS: u64 = 30_000;
pub const MAX_RUNTIME_SUBSCRIPTION_EVENTS: u16 = 256;
pub const DEFAULT_RUNTIME_EVENT_QUERY_EVENTS: u16 = 128;
pub const MAX_RUNTIME_EVENT_QUERY_EVENTS: u16 = 256;
pub const MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES: usize = 768 * 1024;
pub const MAX_GOVERNANCE_CLIENT_BYTES: usize = 64;
pub const MAX_GOVERNANCE_CLIENT_VERSION_BYTES: usize = 32;
/// Closed bounds of `PauseScheduling.drain_timeout_ms` (Workflow #191 ps1).
pub const MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS: u64 = 1_000;
pub const MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS: u64 = 600_000;
/// Bound of a scheduling pause reason `pause_reason` (`[a-z0-9_.-]`).
pub const MAX_SCHEDULING_PAUSE_REASON_BYTES: usize = 64;
/// How long an instance pause waits, once its drain timeout expired, for the runs it asked to
/// stop to reach their next checkpoint; past it the pause fails and its gate is lifted.
pub const SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS: u64 = 30_000;
/// The bound of a device reconnect's backend opens (the touch handshake, 8 s by default, and the
/// capture prime): a client waits this long plus its IO timeout for the receipt of an instance
/// `ResumeScheduling` or a `SelfCheckInstance` (Workflow #191 h2).
pub const CONNECTION_PREPARATION_WAIT_MS: u64 = 30_000;
/// Workflow #369 Q-2: how long the reconnect of an instance `ResumeScheduling`, and a
/// `SelfCheckInstance`, wait for their turn in the instance's queue before the request is
/// refused `lease_queue_expired`. A client adds it to `CONNECTION_PREPARATION_WAIT_MS` and its
/// IO timeout for the receipt of either (W-3).
pub const CONNECTION_PREPARATION_CLAIM_WAIT_MS: u64 = 3_000;
/// Workflow #369 Q-2: how long a `SafeReset` or an `ApplicationLifecycle` command waits for its
/// turn in the instance's queue before it is refused `lease_queue_expired`. A client adds it to
/// the receipt wait of either (W-3).
pub const INPUT_LIFECYCLE_CLAIM_WAIT_MS: u64 = 3_000;
pub const RUNTIME_PLANNING_DOCUMENT_SCHEMA_VERSION: &str =
    "actingcommand.runtime.planning-document.v1";
pub const MAX_RUNTIME_PLANNING_DOCUMENT_BYTES: usize = 512 * 1024;
pub const MAX_RUNTIME_PLANNING_REQUEST_BYTES: usize = 768 * 1024;
pub const MAX_RUNTIME_PLANNING_RESPONSE_BYTES: usize = 768 * 1024;
pub const MAX_RUNTIME_STRATEGIC_EVIDENCE: usize = 64;

pub type RuntimeContractResult<T> = Result<T, RuntimeContractError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeContractError {
    code: Code,
}

impl RuntimeContractError {
    pub(crate) fn new(code: impl Into<Code>) -> Self {
        Self { code: code.into() }
    }

    pub const fn code(self) -> Code {
        self.code
    }
}

impl fmt::Display for RuntimeContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "runtime contract validation failed with {}",
            self.code
        )
    }
}

impl Error for RuntimeContractError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePlanningDocumentKind {
    StrategicReport,
    StrategicProjection,
    EvaluationFacts,
    EvaluationResources,
    EvaluationTime,
    ForwardProjectionConfig,
    ForwardProjection,
    MaintenanceTrendPolicy,
    MaintenanceAssessment,
    MaintenanceAssessmentV2,
}

/// Content-addressed transport envelope for policy types owned by the policy crate.
///
/// The IPC contract intentionally does not depend on the policy implementation crate. Both the
/// Runtime host and typed client decode this bounded document at their explicit domain boundary.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePlanningDocument {
    schema_version: String,
    kind: RuntimePlanningDocumentKind,
    sha256: String,
    document: serde_json::Value,
}

impl RuntimePlanningDocument {
    pub fn encode<T>(kind: RuntimePlanningDocumentKind, value: &T) -> RuntimeContractResult<Self>
    where
        T: Serialize,
    {
        let document = serde_json::to_value(value).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::PlanningDocumentEncodeFailed)
        })?;
        let bytes = planning_document_bytes(&document)?;
        let envelope = Self {
            schema_version: RUNTIME_PLANNING_DOCUMENT_SCHEMA_VERSION.to_owned(),
            kind,
            sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
            document,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn decode<T>(&self, expected: RuntimePlanningDocumentKind) -> RuntimeContractResult<T>
    where
        T: DeserializeOwned,
    {
        self.validate_kind(expected)?;
        serde_json::from_value(self.document.clone()).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::PlanningDocumentDecodeFailed)
        })
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_PLANNING_DOCUMENT_SCHEMA_VERSION {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::UnsupportedPlanningDocumentSchema,
            ));
        }
        let bytes = planning_document_bytes(&self.document)?;
        let expected = format!("sha256:{:x}", Sha256::digest(bytes));
        if self.sha256 != expected {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::PlanningDocumentHashMismatch,
            ));
        }
        Ok(())
    }

    pub fn validate_kind(
        &self,
        expected: RuntimePlanningDocumentKind,
    ) -> RuntimeContractResult<()> {
        self.validate()?;
        if self.kind != expected {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::PlanningDocumentKindMismatch,
            ));
        }
        Ok(())
    }

    pub const fn kind(&self) -> RuntimePlanningDocumentKind {
        self.kind
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    fn byte_count(&self) -> RuntimeContractResult<usize> {
        planning_document_bytes(&self.document).map(|bytes| bytes.len())
    }
}

impl fmt::Debug for RuntimePlanningDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimePlanningDocument")
            .field("kind", &self.kind)
            .field("sha256", &self.sha256)
            .field("document", &"<redacted-policy-document>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStrategicReportRequest {
    report: RuntimePlanningDocument,
    evidence: Vec<ProjectedArtifactReference>,
}

impl RuntimeStrategicReportRequest {
    pub fn new(
        report: RuntimePlanningDocument,
        evidence: Vec<ProjectedArtifactReference>,
    ) -> RuntimeContractResult<Self> {
        let request = Self { report, evidence };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.report
            .validate_kind(RuntimePlanningDocumentKind::StrategicReport)?;
        if self.evidence.is_empty() || self.evidence.len() > MAX_RUNTIME_STRATEGIC_EVIDENCE {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidStrategicEvidence,
            ));
        }
        for reference in &self.evidence {
            reference.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidStrategicEvidence)
            })?;
        }
        Ok(())
    }

    pub const fn report(&self) -> &RuntimePlanningDocument {
        &self.report
    }

    pub fn evidence(&self) -> &[ProjectedArtifactReference] {
        &self.evidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePolicyInputIdentity {
    ledger_position: u64,
    fact_snapshot_id: String,
}

impl RuntimePolicyInputIdentity {
    pub fn new(
        ledger_position: u64,
        fact_snapshot_id: impl Into<String>,
    ) -> RuntimeContractResult<Self> {
        let identity = Self {
            ledger_position,
            fact_snapshot_id: fact_snapshot_id.into(),
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        let valid_snapshot_id = self
            .fact_snapshot_id
            .strip_prefix("snapshot:policy-fact:")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            });
        if self.ledger_position == 0 || !valid_snapshot_id {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidPolicyInputIdentity,
            ));
        }
        Ok(())
    }

    pub const fn ledger_position(&self) -> u64 {
        self.ledger_position
    }

    pub fn fact_snapshot_id(&self) -> &str {
        &self.fact_snapshot_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeForwardProjectionRequest {
    facts: RuntimePlanningDocument,
    resources: RuntimePlanningDocument,
    time: RuntimePlanningDocument,
    seed: u64,
    config: RuntimePlanningDocument,
}

impl RuntimeForwardProjectionRequest {
    pub fn new(
        facts: RuntimePlanningDocument,
        resources: RuntimePlanningDocument,
        time: RuntimePlanningDocument,
        seed: u64,
        config: RuntimePlanningDocument,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            facts,
            resources,
            time,
            seed,
            config,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_planning_documents(&[
            (&self.facts, RuntimePlanningDocumentKind::EvaluationFacts),
            (
                &self.resources,
                RuntimePlanningDocumentKind::EvaluationResources,
            ),
            (&self.time, RuntimePlanningDocumentKind::EvaluationTime),
            (
                &self.config,
                RuntimePlanningDocumentKind::ForwardProjectionConfig,
            ),
        ])
    }

    pub const fn facts(&self) -> &RuntimePlanningDocument {
        &self.facts
    }

    pub const fn resources(&self) -> &RuntimePlanningDocument {
        &self.resources
    }

    pub const fn time(&self) -> &RuntimePlanningDocument {
        &self.time
    }

    pub const fn seed(&self) -> u64 {
        self.seed
    }

    pub const fn config(&self) -> &RuntimePlanningDocument {
        &self.config
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMaintenanceQuery {
    instance_id: String,
    task_id: String,
    fact_scope: FactScope,
    fact_key: String,
    as_of_ledger_position: u64,
    as_of_unix_ms: u64,
    trend_policy: RuntimePlanningDocument,
}

impl RuntimeMaintenanceQuery {
    pub fn new(
        instance_id: impl Into<String>,
        task_id: impl Into<String>,
        fact_scope: FactScope,
        fact_key: impl Into<String>,
        as_of_ledger_position: u64,
        as_of_unix_ms: u64,
        trend_policy: RuntimePlanningDocument,
    ) -> RuntimeContractResult<Self> {
        let query = Self {
            instance_id: instance_id.into(),
            task_id: task_id.into(),
            fact_scope,
            fact_key: fact_key.into(),
            as_of_ledger_position,
            as_of_unix_ms,
            trend_policy,
        };
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        for value in [&self.instance_id, &self.task_id, &self.fact_key] {
            validate_bounded_text(value, 512, RuntimeContractCode::InvalidMaintenanceQuery)?;
        }
        if self.as_of_ledger_position == 0
            || self.as_of_unix_ms == 0
            || self.fact_scope.validate().is_err()
            || matches!(
                &self.fact_scope,
                FactScope::Instance { instance_id } if instance_id != &self.instance_id
            )
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidMaintenanceQuery,
            ));
        }
        self.trend_policy
            .validate_kind(RuntimePlanningDocumentKind::MaintenanceTrendPolicy)
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub const fn fact_scope(&self) -> &FactScope {
        &self.fact_scope
    }

    pub fn fact_key(&self) -> &str {
        &self.fact_key
    }

    pub const fn as_of_ledger_position(&self) -> u64 {
        self.as_of_ledger_position
    }

    pub const fn as_of_unix_ms(&self) -> u64 {
        self.as_of_unix_ms
    }

    pub const fn trend_policy(&self) -> &RuntimePlanningDocument {
        &self.trend_policy
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStrategicPlanResult {
    report: ProjectedArtifactReference,
    projection: RuntimePlanningDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal: Option<Box<CatalogProposal>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<ProposalPreview>,
}

impl RuntimeStrategicPlanResult {
    pub fn new(
        report: ProjectedArtifactReference,
        projection: RuntimePlanningDocument,
        proposal: Option<CatalogProposal>,
        preview: Option<ProposalPreview>,
    ) -> RuntimeContractResult<Self> {
        let plan = Self {
            report,
            projection,
            proposal: proposal.map(Box::new),
            preview,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.report.validate().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidStrategicPlanResult)
        })?;
        if self.report.kind() != ArtifactKind::StrategyReport
            || self.report.object_key().is_none()
            || self.report.redaction_state() == ArtifactRedactionState::Pending
            || self.proposal.is_some() != self.preview.is_some()
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidStrategicPlanResult,
            ));
        }
        self.projection
            .validate_kind(RuntimePlanningDocumentKind::StrategicProjection)?;
        if let Some(proposal) = &self.proposal {
            proposal.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidStrategicPlanResult)
            })?;
        }
        if let Some(preview) = &self.preview {
            preview.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidStrategicPlanResult)
            })?;
        }
        if let (Some(proposal), Some(preview)) = (&self.proposal, &self.preview)
            && proposal.proposal_id() != preview.proposal_id()
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidStrategicPlanResult,
            ));
        }
        let mut byte_count = self.projection.byte_count()?;
        if let Some(proposal) = &self.proposal {
            byte_count = byte_count
                .checked_add(planning_result_bytes(proposal.as_ref())?)
                .ok_or_else(|| {
                    RuntimeContractError::new(RuntimeContractCode::PlanningResponseSizeInvalid)
                })?;
        }
        if let Some(preview) = &self.preview {
            byte_count = byte_count
                .checked_add(planning_result_bytes(preview)?)
                .ok_or_else(|| {
                    RuntimeContractError::new(RuntimeContractCode::PlanningResponseSizeInvalid)
                })?;
        }
        if byte_count > MAX_RUNTIME_PLANNING_RESPONSE_BYTES {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::PlanningResponseSizeInvalid,
            ));
        }
        Ok(())
    }

    pub fn into_parts(
        self,
    ) -> (
        ProjectedArtifactReference,
        RuntimePlanningDocument,
        Option<CatalogProposal>,
        Option<ProposalPreview>,
    ) {
        (
            self.report,
            self.projection,
            self.proposal.map(|value| *value),
            self.preview,
        )
    }
}

fn planning_document_bytes(document: &serde_json::Value) -> RuntimeContractResult<Vec<u8>> {
    if !document.is_object() {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::PlanningDocumentNotObject,
        ));
    }
    let bytes = serde_json::to_vec(document).map_err(|_| {
        RuntimeContractError::new(RuntimeContractCode::PlanningDocumentEncodeFailed)
    })?;
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_PLANNING_DOCUMENT_BYTES {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::PlanningDocumentSizeInvalid,
        ));
    }
    Ok(bytes)
}

fn planning_result_bytes(value: &impl Serialize) -> RuntimeContractResult<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| RuntimeContractError::new(SharedCode::PlanningResultEncodeFailed))
}

fn validate_planning_documents(
    documents: &[(&RuntimePlanningDocument, RuntimePlanningDocumentKind)],
) -> RuntimeContractResult<()> {
    let mut total = 0_usize;
    for (document, kind) in documents {
        document.validate_kind(*kind)?;
        total = total.checked_add(document.byte_count()?).ok_or_else(|| {
            RuntimeContractError::new(RuntimeContractCode::PlanningRequestSizeInvalid)
        })?;
    }
    if total > MAX_RUNTIME_PLANNING_REQUEST_BYTES {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::PlanningRequestSizeInvalid,
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Request data identifying the actual source frame; the Runtime resolves its committed binding.
pub struct InputFrameReference {
    pub frame_id: FrameId,
    pub width: u32,
    pub height: u32,
}

impl InputFrameReference {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.width == 0
            || self.height == 0
            || self.width > i32::MAX as u32
            || self.height > i32::MAX as u32
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InputFrameDimensionsInvalid,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputAction {
    Tap {
        x: i32,
        y: i32,
    },
    LongTap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Swipe {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    },
    SingleTouchDragWithVerticalBrakeV1 {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        x3: i32,
        y3: i32,
        horizontal_duration_ms: u64,
        corner_hold_ms: u64,
        brake_distance_px: i32,
        brake_duration_ms: u64,
        slope_in: u8,
        slope_out: u8,
    },
    /// Workflow #406 (register §1.18): one press that passes through `points` in order and lifts
    /// at the last, inside the box `box_x`, `box_y`, `box_width`, `box_height`; segment `i` (from
    /// point `i` to point `i + 1`) takes `segment_duration_ms[i]`, and the touch holds
    /// `corner_hold_ms` at every inner point. Decodable since v0.12.0; since #406 T-3 (register
    /// §1.137) the kernel generates it from a `single_touch_path_drag_v1` click and every backend
    /// that performs segmented swipes performs it.
    SingleTouchPathDragV1 {
        box_x: i32,
        box_y: i32,
        box_width: i32,
        box_height: i32,
        points: Vec<InputPathPoint>,
        segment_duration_ms: Vec<u64>,
        corner_hold_ms: u64,
    },
    /// Workflow #308 SEL-L3b (register §1.58): one press at (`x1`, `y1`) dragged to (`x2`, `y2`)
    /// over `drag_duration_ms`, held still there for `pause_ms`, then moved to (`x3`, `y3`) over
    /// `brake_duration_ms` (the brake), and released. A drag without a brake has
    /// `brake_duration_ms` 0 and (`x3`, `y3`) equal to (`x2`, `y2`); one without a pause has
    /// `pause_ms` 0; it has at least one of the two, which end the drag without fling. Every
    /// point lies on the screen, `drag_duration_ms` is 1 to 60 000 and the other two durations are
    /// at most 60 000.
    SingleTouchDragWithPauseAndBrakeV1 {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        drag_duration_ms: u64,
        pause_ms: u64,
        x3: i32,
        y3: i32,
        brake_duration_ms: u64,
    },
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Reset,
}

/// One point of a single-touch path drag (Workflow #406).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputPathPoint {
    pub x: i32,
    pub y: i32,
}

/// A single-touch path drag (Workflow #406, register §1.18): the box lies on the screen and is
/// not empty; two to [`MAX_PATH_DRAG_POINTS`] points, each on the screen and inside the box;
/// one duration per segment, each within the input duration bound, and a corner hold within the
/// same bound.
fn validate_path_drag(
    (box_x, box_y, box_width, box_height): (i32, i32, i32, i32),
    points: &[InputPathPoint],
    segment_duration_ms: &[u64],
    corner_hold_ms: u64,
) -> RuntimeContractResult<()> {
    let (Some(box_end_x), Some(box_end_y)) =
        (box_x.checked_add(box_width), box_y.checked_add(box_height))
    else {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidSegmentedSwipeContract,
        ));
    };
    if box_x < 0
        || box_y < 0
        || box_width <= 0
        || box_height <= 0
        || !(2..=MAX_PATH_DRAG_POINTS).contains(&points.len())
        || segment_duration_ms.len() + 1 != points.len()
    {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidSegmentedSwipeContract,
        ));
    }
    for point in points {
        validate_point(point.x, point.y)?;
        if !(box_x..box_end_x).contains(&point.x) || !(box_y..box_end_y).contains(&point.y) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidInputCoordinate,
            ));
        }
    }
    for duration_ms in segment_duration_ms {
        validate_duration(*duration_ms)?;
    }
    if corner_hold_ms > MAX_INPUT_DURATION_MS {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidInputDuration,
        ));
    }
    Ok(())
}

impl InputAction {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Tap { x, y } => validate_point(*x, *y),
            Self::LongTap { x, y, duration_ms } => {
                validate_point(*x, *y)?;
                validate_duration(*duration_ms)
            }
            Self::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                validate_point(*x1, *y1)?;
                validate_point(*x2, *y2)?;
                validate_duration(*duration_ms)
            }
            Self::SingleTouchDragWithVerticalBrakeV1 {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
                horizontal_duration_ms,
                corner_hold_ms,
                brake_distance_px,
                brake_duration_ms,
                slope_in,
                slope_out,
            } => {
                validate_point(*x1, *y1)?;
                validate_point(*x2, *y2)?;
                validate_point(*x3, *y3)?;
                if *horizontal_duration_ms != SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS
                    || *corner_hold_ms != SEGMENTED_SWIPE_CORNER_HOLD_MS
                    || *brake_distance_px != SEGMENTED_SWIPE_BRAKE_DISTANCE_PX
                    || *brake_duration_ms != SEGMENTED_SWIPE_BRAKE_DURATION_MS
                    || *slope_in != SEGMENTED_SWIPE_SLOPE_IN
                    || *slope_out != SEGMENTED_SWIPE_SLOPE_OUT
                    || *x3 != *x2
                    || y2.checked_sub(*brake_distance_px) != Some(*y3)
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSegmentedSwipeContract,
                    ));
                }
                Ok(())
            }
            Self::SingleTouchPathDragV1 {
                box_x,
                box_y,
                box_width,
                box_height,
                points,
                segment_duration_ms,
                corner_hold_ms,
            } => validate_path_drag(
                (*box_x, *box_y, *box_width, *box_height),
                points,
                segment_duration_ms,
                *corner_hold_ms,
            ),
            Self::SingleTouchDragWithPauseAndBrakeV1 {
                x1,
                y1,
                x2,
                y2,
                drag_duration_ms,
                pause_ms,
                x3,
                y3,
                brake_duration_ms,
            } => {
                validate_point(*x1, *y1)?;
                validate_point(*x2, *y2)?;
                validate_point(*x3, *y3)?;
                validate_duration(*drag_duration_ms)?;
                if *pause_ms > MAX_INPUT_DURATION_MS || *brake_duration_ms > MAX_INPUT_DURATION_MS {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidInputDuration,
                    ));
                }
                // A brake moves the touch and takes time, or there is none; a drag with neither
                // a pause nor a brake is a plain swipe.
                let braked = *brake_duration_ms > 0;
                let brake_moves = (*x3, *y3) != (*x2, *y2);
                if braked != brake_moves || (*pause_ms == 0 && !braked) {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSegmentedSwipeContract,
                    ));
                }
                Ok(())
            }
            Self::Key { key } => {
                validate_bounded_text(key, MAX_INPUT_KEY_BYTES, RuntimeContractCode::InvalidKey)
            }
            Self::Text { text } => validate_bounded_text(
                text,
                MAX_INPUT_TEXT_BYTES,
                RuntimeContractCode::InvalidInputText,
            ),
            Self::Reset => Ok(()),
        }
    }

    pub const fn effect(&self) -> EffectDisposition {
        EffectDisposition::Performed
    }

    pub const fn event_action(&self) -> crate::EventAction {
        match self {
            Self::Tap { .. } => crate::EventAction::InputTap,
            Self::LongTap { .. } => crate::EventAction::InputLongTap,
            Self::Swipe { .. } => crate::EventAction::InputSwipe,
            Self::SingleTouchDragWithVerticalBrakeV1 { .. } => crate::EventAction::InputSwipe,
            Self::SingleTouchPathDragV1 { .. } => crate::EventAction::InputSwipe,
            Self::SingleTouchDragWithPauseAndBrakeV1 { .. } => crate::EventAction::InputSwipe,
            Self::Key { .. } => crate::EventAction::InputKey,
            Self::Text { .. } => crate::EventAction::InputText,
            Self::Reset => crate::EventAction::InputReset,
        }
    }
}

impl fmt::Debug for InputAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Tap { .. } => "InputAction::Tap(<redacted-coordinates>)",
            Self::LongTap { .. } => "InputAction::LongTap(<redacted-coordinates>)",
            Self::Swipe { .. } => "InputAction::Swipe(<redacted-coordinates>)",
            Self::SingleTouchDragWithVerticalBrakeV1 { .. } => {
                "InputAction::SingleTouchDragWithVerticalBrakeV1(<redacted-coordinates>)"
            }
            Self::SingleTouchPathDragV1 { .. } => {
                "InputAction::SingleTouchPathDragV1(<redacted-coordinates>)"
            }
            Self::SingleTouchDragWithPauseAndBrakeV1 { .. } => {
                "InputAction::SingleTouchDragWithPauseAndBrakeV1(<redacted-coordinates>)"
            }
            Self::Key { .. } => "InputAction::Key(<redacted-key>)",
            Self::Text { .. } => "InputAction::Text(<redacted-text>)",
            Self::Reset => "InputAction::Reset",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationLifecycleAction {
    Launch,
    Stop,
    Restart,
}

impl ApplicationLifecycleAction {
    pub const fn event_action(self) -> crate::EventAction {
        match self {
            Self::Launch => crate::EventAction::ApplicationLaunch,
            Self::Stop => crate::EventAction::ApplicationStop,
            Self::Restart => crate::EventAction::ApplicationRestart,
        }
    }
}

/// What emulator instance control did with the instance's configured startup package
/// (slice #316-B3): `none` when no package is configured for the instance or the action was
/// `stop`, `scheduled` when the host queued it for its own scheduling point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupPackageDisposition {
    #[default]
    None,
    Scheduled,
}

/// One lifecycle action on the emulator instance itself (the provider's `control` surface),
/// distinct from the application lifecycle inside a running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorInstanceAction {
    Start,
    Stop,
    Restart,
}

impl EmulatorInstanceAction {
    pub const fn event_action(self) -> crate::EventAction {
        match self {
            Self::Start => crate::EventAction::EmulatorInstanceStart,
            Self::Stop => crate::EventAction::EmulatorInstanceStop,
            Self::Restart => crate::EventAction::EmulatorInstanceRestart,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseToken {
    owner_epoch: OwnerEpoch,
    lease_id: LeaseId,
    instance_id: InstanceId,
    holder_id: HolderId,
    expires_at_monotonic_ms: u64,
}

impl LeaseToken {
    pub fn new(
        owner_epoch: OwnerEpoch,
        lease_id: LeaseId,
        instance_id: InstanceId,
        holder_id: HolderId,
        expires_at_monotonic_ms: u64,
    ) -> RuntimeContractResult<Self> {
        let token = Self {
            owner_epoch,
            lease_id,
            instance_id,
            holder_id,
            expires_at_monotonic_ms,
        };
        token.validate()?;
        Ok(token)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.expires_at_monotonic_ms == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLeaseExpiry,
            ));
        }
        Ok(())
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub const fn lease_id(&self) -> LeaseId {
        self.lease_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn holder_id(&self) -> HolderId {
        self.holder_id
    }

    pub const fn expires_at_monotonic_ms(&self) -> u64 {
        self.expires_at_monotonic_ms
    }
}

/// The original scheduler admission carried through one in-flight execution step.
/// The issuing bridge is constrained by the workspace issuer guard; Rust privacy
/// prevents field construction, but does not make an external crate a friend.
#[derive(Debug, PartialEq, Eq)]
pub struct FencedWrite {
    token: LeaseToken,
    connection_id: u64,
    step_id: std::num::NonZeroU64,
    purpose: FencedWritePurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FencedWritePurpose {
    Business,
    ResourceClose,
}

impl FencedWrite {
    pub fn token(&self) -> &LeaseToken {
        &self.token
    }

    pub const fn connection_id(&self) -> u64 {
        self.connection_id
    }

    pub const fn step_id(&self) -> std::num::NonZeroU64 {
        self.step_id
    }

    pub const fn purpose(&self) -> FencedWritePurpose {
        self.purpose
    }
}

/// Cross-crate issuance bridge. Only the scheduler's two admission methods may
/// call this on the production graph; this is enforced by the issuer guard.
pub fn issue_fenced_write(
    token: LeaseToken,
    connection_id: u64,
    step_id: std::num::NonZeroU64,
    purpose: FencedWritePurpose,
) -> FencedWrite {
    FencedWrite {
        token,
        connection_id,
        step_id,
        purpose,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeasePriority {
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseQueuePolicy {
    priority: LeasePriority,
    timeout_ms: u64,
}

impl LeaseQueuePolicy {
    pub fn new(priority: LeasePriority, timeout_ms: u64) -> RuntimeContractResult<Self> {
        let policy = Self {
            priority,
            timeout_ms,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.timeout_ms == 0 || self.timeout_ms > MAX_LEASE_QUEUE_TIMEOUT_MS {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLeaseQueueTimeout,
            ));
        }
        Ok(())
    }

    pub const fn priority(self) -> LeasePriority {
        self.priority
    }

    pub const fn timeout_ms(self) -> u64 {
        self.timeout_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseQueueStatus {
    request_id: RequestId,
    instance_id: InstanceId,
    priority: LeasePriority,
    position: u32,
    deadline_monotonic_ms: u64,
    preempt_requested: bool,
}

impl LeaseQueueStatus {
    pub fn new(
        request_id: RequestId,
        instance_id: InstanceId,
        priority: LeasePriority,
        position: u32,
        deadline_monotonic_ms: u64,
        preempt_requested: bool,
    ) -> RuntimeContractResult<Self> {
        let status = Self {
            request_id,
            instance_id,
            priority,
            position,
            deadline_monotonic_ms,
            preempt_requested,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.position == 0 || self.deadline_monotonic_ms == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidLeaseQueueStatus,
            ));
        }
        Ok(())
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn priority(&self) -> LeasePriority {
        self.priority
    }

    pub const fn position(&self) -> u32 {
        self.position
    }

    pub const fn deadline_monotonic_ms(&self) -> u64 {
        self.deadline_monotonic_ms
    }

    pub const fn preempt_requested(&self) -> bool {
        self.preempt_requested
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCaptureBackend {
    AdbScreencap,
    AdbScreencapEncode,
    AdbScreencapRawGzip,
    DroidcastRaw,
    NemuIpc,
    /// #402 RD-3: the scrcpy continuous stream (real devices only).
    Scrcpy,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadonlyObservation {
    width: u32,
    height: u32,
    verdict: RecognitionVerdict,
    capture_backend: RuntimeCaptureBackend,
    artifact: ProjectedArtifactReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadonlyFrame {
    width: u32,
    height: u32,
}

impl ReadonlyFrame {
    pub fn new(width: u32, height: u32) -> RuntimeContractResult<Self> {
        let frame = Self { width, height };
        frame.validate()?;
        Ok(frame)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.width == 0 || self.height == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidFrameDimensions,
            ));
        }
        Ok(())
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }
}

impl ReadonlyObservation {
    pub fn new(
        width: u32,
        height: u32,
        verdict: RecognitionVerdict,
        capture_backend: RuntimeCaptureBackend,
        artifact: ProjectedArtifactReference,
    ) -> RuntimeContractResult<Self> {
        let observation = Self {
            width,
            height,
            verdict,
            capture_backend,
            artifact,
        };
        observation.validate()?;
        Ok(observation)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.width == 0 || self.height == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationDimensions,
            ));
        }
        if self.artifact.validate().is_err()
            || self.artifact.object_key().is_none()
            || self.artifact.kind() != ArtifactKind::CaptureFrame
            || self.artifact.media_type() != ArtifactMediaType::ImagePng
            || self.artifact.frame_id().is_none()
            || self.artifact.redaction_state() == ArtifactRedactionState::Pending
            || self.artifact.byte_count() > MAX_READONLY_OBSERVATION_ARTIFACT_BYTES
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationArtifact,
            ));
        }
        Ok(())
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn verdict(&self) -> RecognitionVerdict {
        self.verdict
    }

    pub const fn capture_backend(&self) -> RuntimeCaptureBackend {
        self.capture_backend
    }

    pub const fn artifact(&self) -> &ProjectedArtifactReference {
        &self.artifact
    }
}

impl fmt::Debug for ReadonlyObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReadonlyObservation")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("verdict", &self.verdict)
            .field("capture_backend", &self.capture_backend)
            .field("artifact", &"<redacted-artifact-reference>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSequenceSpec {
    frame_count: u16,
    interval_ms: u64,
}

impl CaptureSequenceSpec {
    pub fn new(frame_count: u16, interval_ms: u64) -> RuntimeContractResult<Self> {
        let spec = Self {
            frame_count,
            interval_ms,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.frame_count == 0
            || self.frame_count > MAX_RUNTIME_CAPTURE_SEQUENCE_FRAMES
            || self.interval_ms > MAX_RUNTIME_CAPTURE_SEQUENCE_INTERVAL_MS
            || self.planned_wait_ms()? > MAX_RUNTIME_CAPTURE_SEQUENCE_WAIT_MS
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidCaptureSequenceSpec,
            ));
        }
        Ok(())
    }

    pub const fn frame_count(&self) -> u16 {
        self.frame_count
    }

    pub const fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    pub fn planned_wait_ms(&self) -> RuntimeContractResult<u64> {
        u64::from(self.frame_count.saturating_sub(1))
            .checked_mul(self.interval_ms)
            .ok_or_else(|| {
                RuntimeContractError::new(RuntimeContractCode::CaptureSequenceWaitOverflow)
            })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSequence {
    spec: CaptureSequenceSpec,
    observations: Vec<ReadonlyObservation>,
}

impl CaptureSequence {
    pub fn new(
        spec: CaptureSequenceSpec,
        observations: Vec<ReadonlyObservation>,
    ) -> RuntimeContractResult<Self> {
        let sequence = Self { spec, observations };
        sequence.validate()?;
        Ok(sequence)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.spec.validate()?;
        if self.observations.len() != usize::from(self.spec.frame_count()) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidCaptureSequenceObservationCount,
            ));
        }
        let mut artifact_ids = BTreeSet::new();
        let mut frame_ids = BTreeSet::new();
        for observation in &self.observations {
            observation.validate()?;
            let artifact = observation.artifact();
            let frame_id = artifact.frame_id().ok_or_else(|| {
                RuntimeContractError::new(
                    RuntimeContractCode::InvalidCaptureSequenceArtifactIdentity,
                )
            })?;
            if !artifact_ids.insert(artifact.artifact_id) || !frame_ids.insert(*frame_id) {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::DuplicateCaptureSequenceArtifactIdentity,
                ));
            }
        }
        Ok(())
    }

    pub const fn spec(&self) -> CaptureSequenceSpec {
        self.spec
    }

    pub fn observations(&self) -> &[ReadonlyObservation] {
        &self.observations
    }
}

impl fmt::Debug for CaptureSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptureSequence")
            .field("spec", &self.spec)
            .field("observation_count", &self.observations.len())
            .finish()
    }
}

/// Whether an instance's default resource package is a package file or a package directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceResourcePackageKind {
    File,
    Directory,
}

/// The default resource package configured for an instance (slice #324-r1): the local path the
/// daemon admitted at startup. No digest is carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceResourcePackage {
    pub path: String,
    pub kind: InstanceResourcePackageKind,
}

/// The stuck-recovery ladder settings of an instance (slice #316-B4): `enabled` is its
/// `stuck_recovery`, `cooldown_secs` its `stuck_recovery_cooldown_secs` (`1..=86400`): at most
/// one ladder per instance per cool-down window. Workflow #387 D1c (M387 §2.1 row 18b): there is
/// no code default; an instance that names neither takes the `recovery_ladder` section's
/// `enabled` and `cooldown_secs` in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceStuckRecovery {
    pub enabled: bool,
    pub cooldown_secs: u32,
}

impl InstanceStuckRecovery {
    pub const MAX_COOLDOWN_SECS: u32 = 86_400;

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if !(1..=Self::MAX_COOLDOWN_SECS).contains(&self.cooldown_secs) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidStuckRecoveryCooldown,
            ));
        }
        Ok(())
    }
}

/// Workflow #390 九处-1 (board #390 comment 6095942731, item 1; coordinator's corrected ruling
/// 4, 2026-10-16): whether an instance's emulator runs, as the Runtime last read it, with the
/// time of that reading. The Runtime reads it from `MuMuManager info` at instance discovery (the
/// daemon's start and an on-demand `DiscoverInstances`) and from an emulator control (a
/// confirmed start or stop, and the state read after a failed control); it never polls for it.
/// `unknown` while it holds no reading: an instance that is not discovery-bound, or one whose
/// failed control left the state unsettled. It never defaults to `off`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmulatorRunningState {
    On {
        observed_at_unix_ms: u64,
    },
    Off {
        observed_at_unix_ms: u64,
    },
    #[default]
    Unknown,
}

impl EmulatorRunningState {
    /// A reading taken at `observed_at_unix_ms`: `on` when the emulator runs, else `off`.
    pub const fn observed(running: bool, observed_at_unix_ms: u64) -> Self {
        if running {
            Self::On {
                observed_at_unix_ms,
            }
        } else {
            Self::Off {
                observed_at_unix_ms,
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInstanceStatus {
    instance_alias: String,
    instance_id: InstanceId,
    lease_active: bool,
    queued_request_count: u32,
    takeover_cooldown_active: bool,
    destructive_step_active: bool,
    preempt_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend_provenance: Option<crate::ExecutionBackendProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capabilities: Option<crate::EmulatorCapabilityProfile>,
    /// The configured ADB port that identifies the emulator instance in the ledger;
    /// absent for a serial-configured instance or one without an ADB target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adb_port: Option<u16>,
    /// The instance's configured default resource package; absent when none is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_package: Option<InstanceResourcePackage>,
    /// The game of the instance's configured policy identity (Workflow #308 slice 4a-2);
    /// absent when the host runs without policy inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    game_id: Option<String>,
    /// The operator's scheduling pause of this instance (Workflow #191 ps1); absent when
    /// the instance is not paused. Persisted since Workflow #361 B1 and restored at the next
    /// start at revision 1 in the new owner epoch, at stage `released`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pause: Option<InstancePauseState>,
    /// Workflow #403: the instance's resource fence; absent when it is not fenced. An
    /// unconfirmed close of the instance's own resources fences it until a preparation's
    /// self-check proves the close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_fence: Option<InstanceResourceFence>,
    /// Workflow #390 九处-1: the emulator's running state as the Runtime last read it; always
    /// answered, `{"state": "unknown"}` without a reading.
    #[serde(default)]
    emulator_state: EmulatorRunningState,
}

/// Workflow #403: why and since when an instance is fenced, and whether it waits for a person.
/// The fenced instance gets no new dispatch; the recovery ladder, or a person's resume,
/// emulator restart or self-check, lifts the fence only through a preparation whose self-check
/// passes. `awaiting_person` is set once the ladder ended without that proof, or when none
/// could start; `person_wait` then says since when and how its periodic self-check stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceResourceFence {
    /// The code of the unconfirmed failure that fenced the instance.
    pub cause: crate::outcome::CodeStr,
    pub since_unix_ms: u64,
    pub awaiting_person: bool,
    /// #411 A38: present exactly while the instance waits for a person: since when, and its
    /// periodic self-check (the recheck).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_wait: Option<InstanceFencePersonWait>,
}

/// #411 A38: a fenced instance's wait for a person. While it waits, the Runtime runs the
/// instance's self-check every `recovery_ladder.fence_recheck.interval_ms`, at most
/// `recovery_ladder.fence_recheck.max_attempts` times; a passing one lifts the fence as a
/// person's self-check does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceFencePersonWait {
    /// When the instance began to wait for a person.
    pub since_unix_ms: u64,
    /// The rechecks started in this wait, a running one included.
    pub recheck_attempts: u32,
    /// The most rechecks of one wait, `max_attempts` in effect (0: the recheck is off).
    pub recheck_max_attempts: u32,
    /// When the next recheck is due; absent while one runs, once the last one has run, and
    /// while the recheck is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_recheck_unix_ms: Option<u64>,
}

impl RuntimeInstanceStatus {
    pub fn new(
        instance_alias: impl Into<String>,
        instance_id: InstanceId,
        lease_active: bool,
        queued_request_count: u32,
        takeover_cooldown_active: bool,
        destructive_step_active: bool,
        preempt_requested: bool,
    ) -> RuntimeContractResult<Self> {
        let status = Self {
            instance_alias: instance_alias.into(),
            instance_id,
            lease_active,
            queued_request_count,
            takeover_cooldown_active,
            destructive_step_active,
            preempt_requested,
            backend_provenance: None,
            capabilities: None,
            adb_port: None,
            resource_package: None,
            game_id: None,
            pause: None,
            resource_fence: None,
            emulator_state: EmulatorRunningState::Unknown,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_instance_alias(&self.instance_alias)?;
        if let Some(game_id) = &self.game_id {
            validate_bounded_text(
                game_id,
                MAX_STATUS_GAME_ID_BYTES,
                RuntimeContractCode::InvalidRuntimeStatusGame,
            )?;
        }
        if let Some(pause) = &self.pause {
            pause.validate()?;
        }
        if self.capabilities.is_some() && self.backend_provenance.is_none() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::RuntimeCapabilitiesProvenanceMissing,
            ));
        }
        if (self.destructive_step_active || self.preempt_requested) && !self.lease_active {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeInstanceStatus,
            ));
        }
        if self.lease_active && self.takeover_cooldown_active {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeInstanceStatus,
            ));
        }
        Ok(())
    }

    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn lease_active(&self) -> bool {
        self.lease_active
    }

    pub const fn queued_request_count(&self) -> u32 {
        self.queued_request_count
    }

    pub const fn takeover_cooldown_active(&self) -> bool {
        self.takeover_cooldown_active
    }

    pub const fn destructive_step_active(&self) -> bool {
        self.destructive_step_active
    }

    pub const fn preempt_requested(&self) -> bool {
        self.preempt_requested
    }

    pub fn with_backend_metadata(
        mut self,
        provenance: crate::ExecutionBackendProvenance,
        capabilities: Option<crate::EmulatorCapabilityProfile>,
    ) -> Self {
        self.backend_provenance = Some(provenance);
        self.capabilities = capabilities;
        self
    }

    pub const fn backend_provenance(&self) -> Option<crate::ExecutionBackendProvenance> {
        self.backend_provenance
    }

    pub fn capabilities(&self) -> Option<&crate::EmulatorCapabilityProfile> {
        self.capabilities.as_ref()
    }

    pub const fn with_adb_port(mut self, adb_port: Option<u16>) -> Self {
        self.adb_port = adb_port;
        self
    }

    pub const fn adb_port(&self) -> Option<u16> {
        self.adb_port
    }

    pub fn with_resource_package(
        mut self,
        resource_package: Option<InstanceResourcePackage>,
    ) -> Self {
        self.resource_package = resource_package;
        self
    }

    pub const fn resource_package(&self) -> Option<&InstanceResourcePackage> {
        self.resource_package.as_ref()
    }

    pub fn with_game_id(mut self, game_id: Option<String>) -> RuntimeContractResult<Self> {
        self.game_id = game_id;
        self.validate()?;
        Ok(self)
    }

    pub fn game_id(&self) -> Option<&str> {
        self.game_id.as_deref()
    }

    pub fn with_pause(mut self, pause: Option<InstancePauseState>) -> RuntimeContractResult<Self> {
        self.pause = pause;
        self.validate()?;
        Ok(self)
    }

    pub const fn pause(&self) -> Option<&InstancePauseState> {
        self.pause.as_ref()
    }

    /// Workflow #403: this status with the instance's resource fence.
    pub fn with_resource_fence(mut self, resource_fence: Option<InstanceResourceFence>) -> Self {
        self.resource_fence = resource_fence;
        self
    }

    pub const fn resource_fence(&self) -> Option<&InstanceResourceFence> {
        self.resource_fence.as_ref()
    }

    /// Workflow #390 九处-1: this status with the emulator's running state as the Runtime last
    /// read it.
    pub const fn with_emulator_state(mut self, emulator_state: EmulatorRunningState) -> Self {
        self.emulator_state = emulator_state;
        self
    }

    pub const fn emulator_state(&self) -> EmulatorRunningState {
        self.emulator_state
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeControlPlaneStatus {
    owner_epoch: OwnerEpoch,
    instances: Vec<RuntimeInstanceStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<crate::RuntimeStateSource>,
    /// The operator's global scheduling pause (Workflow #191 ps1); absent when scheduling is
    /// not globally paused. Persisted since Workflow #361 B1 and restored at the next start at
    /// revision 1 in the new owner epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scheduling_pause: Option<SchedulingPauseState>,
}

impl RuntimeControlPlaneStatus {
    pub fn with_source(mut self, source: crate::RuntimeStateSource) -> RuntimeContractResult<Self> {
        source.validate().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeStateSource)
        })?;
        self.source = Some(source);
        Ok(self)
    }

    pub fn source(&self) -> Option<&crate::RuntimeStateSource> {
        self.source.as_ref()
    }
    pub fn new(
        owner_epoch: OwnerEpoch,
        mut instances: Vec<RuntimeInstanceStatus>,
    ) -> RuntimeContractResult<Self> {
        instances.sort_by(|left, right| left.instance_alias.cmp(&right.instance_alias));
        let status = Self {
            owner_epoch,
            instances,
            source: None,
            scheduling_pause: None,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn with_scheduling_pause(
        mut self,
        scheduling_pause: Option<SchedulingPauseState>,
    ) -> RuntimeContractResult<Self> {
        self.scheduling_pause = scheduling_pause;
        self.validate()?;
        Ok(self)
    }

    pub const fn scheduling_pause(&self) -> Option<&SchedulingPauseState> {
        self.scheduling_pause.as_ref()
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if let Some(source) = &self.source {
            source.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeStateSource)
            })?;
        }
        if let Some(pause) = &self.scheduling_pause {
            pause.validate()?;
        }
        let mut aliases = BTreeSet::new();
        let mut instance_ids = BTreeSet::new();
        let mut previous_alias = None;
        for instance in &self.instances {
            instance.validate()?;
            if !aliases.insert(instance.instance_alias.as_str())
                || !instance_ids.insert(instance.instance_id)
                || previous_alias.is_some_and(|previous| previous >= instance.instance_alias())
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeStatusRegistry,
                ));
            }
            previous_alias = Some(instance.instance_alias());
        }
        Ok(())
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub fn instances(&self) -> &[RuntimeInstanceStatus] {
        &self.instances
    }
}

/// What an operator scheduling pause covers (Workflow #191 ps1): every policy dispatch, or the
/// policy dispatches of one registered physical instance. The two scopes hold their own state
/// and revision and never imply each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchedulingPauseScope {
    Global,
    Instance { instance_alias: String },
}

impl SchedulingPauseScope {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Global => Ok(()),
            Self::Instance { instance_alias } => validate_instance_alias(instance_alias),
        }
    }

    pub fn instance_alias(&self) -> Option<&str> {
        match self {
            Self::Global => None,
            Self::Instance { instance_alias } => Some(instance_alias),
        }
    }
}

/// The pause a conditional `ResumeScheduling` may lift (Workflow #338 R4): the owner epoch the
/// caller saw it in and the revision of its scope then. The host compares both with its own
/// under the pause table's lock and refuses a resume of any other pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingPauseExpectation {
    pub owner_epoch: OwnerEpoch,
    pub revision: u64,
}

impl SchedulingPauseExpectation {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.revision == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidSchedulingPauseExpectation,
            ));
        }
        Ok(())
    }
}

/// The in-flight contained runs an instance pause drained: `finished` ended on their own,
/// `cancelled` were asked to stop after the drain timeout (`contained_task_paused`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingDrainSummary {
    pub finished: u32,
    pub cancelled: u32,
}

/// The connection self-check an instance resume reports (Workflow #191 ps2), projected from the
/// backend open reports of the resume's own reconnect. A side is `ok` only when that open
/// verified it: its open status and its check (`capture_check`, `input_check`) passed. A side
/// without a report of this reconnect (the preparation lease was refused, the close of a
/// session the instance still held failed, or an open failed before reaching that side)
/// reports `ok: false` with no values.
/// `failure_code` is the device code of a failed reconnect verbatim (for example
/// `capture_backend_open_failed`); a failed self-check never rolls the resume back. Since
/// Workflow #317 sc3 the reconnect is the host's preparation phase, which also answers
/// `SelfCheckInstance` with this shape; a preparation that could not take its lease reports
/// the scheduler code (for example `lease_transfer_not_safe`) with both sides `ok: false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingResumeSelfCheck {
    pub capture: SchedulingResumeCaptureCheck,
    pub touch: SchedulingResumeTouchCheck,
    pub failure_code: Option<String>,
}

/// The capture side of [`SchedulingResumeSelfCheck`]: the opened frame size and the capture
/// backend the open selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingResumeCaptureCheck {
    pub ok: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub capture_backend: Option<String>,
}

/// The touch side of [`SchedulingResumeSelfCheck`]: the touch backend the open selected and the
/// bounds its connection reported (the input geometry, else the handshake limits). The check is
/// read back from the connection; it sends no touch, so `invasive` is always `false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingResumeTouchCheck {
    pub ok: bool,
    pub backend: Option<String>,
    pub max_x: Option<i32>,
    pub max_y: Option<i32>,
    pub invasive: bool,
}

impl SchedulingResumeSelfCheck {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let backend_name = |value: &str| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        };
        let (capture, touch) = (&self.capture, &self.touch);
        if capture.width.is_some() != capture.height.is_some()
            || capture.width == Some(0)
            || capture.height == Some(0)
            || capture
                .capture_backend
                .as_deref()
                .is_some_and(|value| !backend_name(value))
            || touch.max_x.is_some() != touch.max_y.is_some()
            || touch.max_x.is_some_and(|value| value <= 0)
            || touch.max_y.is_some_and(|value| value <= 0)
            || touch
                .backend
                .as_deref()
                .is_some_and(|value| !backend_name(value))
            || touch.invasive
            || self
                .failure_code
                .as_deref()
                .is_some_and(|code| validate_scheduling_pause_reason(code).is_err())
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidSchedulingResumeSelfcheck,
            ));
        }
        Ok(())
    }
}

/// Where a per-instance pause stands: its in-flight runs are still draining (a), none is left
/// (b), or its device session is closed and the device handed back (c, Workflow #191 ps2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstancePauseStage {
    Draining,
    Paused,
    Released,
}

/// The global scheduling pause as `Status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingPauseState {
    pub revision: u64,
    pub pause_reason: SchedulingPauseReason,
    pub since_unix_ms: u64,
}

impl SchedulingPauseState {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_scheduling_pause_reason(self.pause_reason.as_str())?;
        if self.revision == 0 || self.since_unix_ms == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidSchedulingPauseState,
            ));
        }
        Ok(())
    }
}

/// One instance's scheduling pause as `Status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstancePauseState {
    pub revision: u64,
    pub pause_reason: SchedulingPauseReason,
    pub since_unix_ms: u64,
    pub stage: InstancePauseStage,
}

impl InstancePauseState {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_scheduling_pause_reason(self.pause_reason.as_str())?;
        if self.revision == 0 || self.since_unix_ms == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidSchedulingPauseState,
            ));
        }
        Ok(())
    }
}

/// A scheduling pause reason is a name the operator or the Runtime chose (the `pause_reason`
/// key, type `name`; Workflow #378 A-9): `1..=64` bytes of `[a-z0-9_.-]`.
pub fn validate_scheduling_pause_reason(value: &str) -> RuntimeContractResult<()> {
    if value.is_empty()
        || value.len() > MAX_SCHEDULING_PAUSE_REASON_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte)
        })
    {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidSchedulingPauseReason,
        ));
    }
    Ok(())
}

/// The reason of a scheduling pause (`pause_reason`, Workflow #378 A-9): a validated name, never
/// a free string. It is built only through [`SchedulingPauseReason::new`] or by decoding, and
/// both apply [`validate_scheduling_pause_reason`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SchedulingPauseReason(String);

impl SchedulingPauseReason {
    pub fn new(value: impl Into<String>) -> RuntimeContractResult<Self> {
        let value = value.into();
        validate_scheduling_pause_reason(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchedulingPauseReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SchedulingPauseReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|error| serde::de::Error::custom(error.code()))
    }
}

/// One instance an instance discovery query reported. `bound_alias` is the registered
/// instance bound to it: the discovery binding with this index, else the explicit HOST:PORT
/// instance with this ADB port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDiscoveredInstance {
    pub instance_index: u16,
    pub instance_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adb_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adb_port: Option<u16>,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_version: Option<String>,
}

/// The answer of `RuntimeOperation::DiscoverInstances`: the provider version and every
/// instance it reported, ordered by index, with the state source of the discovery as `source`
/// (Workflow #383 AO-4q: no ledger record). Tool paths, install roots and the resolution source
/// are not part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInstanceDiscovery {
    provider_version: String,
    source: crate::RuntimeStateSource,
    instances: Vec<RuntimeDiscoveredInstance>,
}

impl RuntimeInstanceDiscovery {
    pub fn new(
        provider_version: impl Into<String>,
        source: crate::RuntimeStateSource,
        instances: Vec<RuntimeDiscoveredInstance>,
    ) -> RuntimeContractResult<Self> {
        let discovery = Self {
            provider_version: provider_version.into(),
            source,
            instances,
        };
        discovery.validate()?;
        Ok(discovery)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.source.validate().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeStateSource)
        })?;
        Self::validate_instances(&self.instances)
    }

    /// Names are non-empty and at most `MAX_DISCOVERED_INSTANCE_NAME_BYTES` (the startup
    /// binding's bound), indexes strictly ascending, and a present `bound_alias` is a valid
    /// instance alias.
    pub fn validate_instances(
        instances: &[RuntimeDiscoveredInstance],
    ) -> RuntimeContractResult<()> {
        let mut previous = None;
        for instance in instances {
            if instance.instance_name.is_empty()
                || instance.instance_name.len() > crate::MAX_DISCOVERED_INSTANCE_NAME_BYTES
                || previous.is_some_and(|index| index >= instance.instance_index)
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidInstanceDiscovery,
                ));
            }
            if let Some(alias) = &instance.bound_alias {
                validate_instance_alias(alias)?;
            }
            previous = Some(instance.instance_index);
        }
        Ok(())
    }

    pub fn provider_version(&self) -> &str {
        &self.provider_version
    }

    pub const fn source(&self) -> &crate::RuntimeStateSource {
        &self.source
    }

    pub fn instances(&self) -> &[RuntimeDiscoveredInstance] {
        &self.instances
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadonlyObservationStage {
    Capture,
    Recognition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadonlyObservationOutcome {
    Completed {
        observation: ReadonlyObservation,
    },
    Failed {
        stage: ReadonlyObservationStage,
        #[serde(skip_serializing_if = "Option::is_none")]
        captured_frame: Option<ReadonlyFrame>,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthoringEvent {
    phase: ResourceAuthoringPhase,
    draft_id: String,
    target_label: String,
    target_fingerprint: String,
    changed_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDebugOperation {
    LabRun,
    Observe,
    Do,
    Ensure,
    Wait,
}

impl RuntimeDebugOperation {
    pub const fn event_action(self) -> crate::EventAction {
        match self {
            Self::LabRun => crate::EventAction::RuntimeDebugLabRun,
            Self::Observe => crate::EventAction::RuntimeDebugObserve,
            Self::Do => crate::EventAction::RuntimeDebugDo,
            Self::Ensure => crate::EventAction::RuntimeDebugEnsure,
            Self::Wait => crate::EventAction::RuntimeDebugWait,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeDebugPhase {
    Requested,
    Progress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDebugEvent {
    operation: RuntimeDebugOperation,
    phase: RuntimeDebugPhase,
    effect_disposition: EffectDisposition,
}

impl RuntimeDebugEvent {
    pub fn requested(operation: RuntimeDebugOperation) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Requested,
            effect_disposition: EffectDisposition::NotPerformed,
        }
    }

    pub fn progress(operation: RuntimeDebugOperation) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Progress,
            effect_disposition: EffectDisposition::NotPerformed,
        }
    }

    pub fn completed(
        operation: RuntimeDebugOperation,
        effect_disposition: EffectDisposition,
    ) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Completed,
            effect_disposition,
        }
    }

    pub fn failed(operation: RuntimeDebugOperation, effect_disposition: EffectDisposition) -> Self {
        Self {
            operation,
            phase: RuntimeDebugPhase::Failed,
            effect_disposition,
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if matches!(
            self.phase,
            RuntimeDebugPhase::Requested | RuntimeDebugPhase::Progress
        ) && self.effect_disposition != EffectDisposition::NotPerformed
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeDebugEvent,
            ));
        }
        if self.phase == RuntimeDebugPhase::Progress
            && self.operation != RuntimeDebugOperation::LabRun
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeDebugEvent,
            ));
        }
        Ok(())
    }

    pub const fn operation(&self) -> RuntimeDebugOperation {
        self.operation
    }

    pub const fn phase(&self) -> RuntimeDebugPhase {
        self.phase
    }

    pub const fn effect_disposition(&self) -> EffectDisposition {
        self.effect_disposition
    }
}

impl ResourceAuthoringEvent {
    pub fn new(
        phase: ResourceAuthoringPhase,
        draft_id: impl Into<String>,
        target_label: impl Into<String>,
        target_fingerprint: impl Into<String>,
        changed_paths: Vec<String>,
        failure_code: Option<String>,
    ) -> RuntimeContractResult<Self> {
        let event = Self {
            phase,
            draft_id: draft_id.into(),
            target_label: target_label.into(),
            target_fingerprint: target_fingerprint.into(),
            changed_paths,
            failure_code,
        };
        event.validate()?;
        Ok(event)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if let Some(code) = &self.failure_code {
            crate::event::validate_resource_authoring_failure_token(code).map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidResourceAuthoringEvent)
            })?;
        }
        let outcome = self.outcome();
        crate::validate_resource_authoring_fields(
            self.phase,
            &self.draft_id,
            &self.target_label,
            &self.target_fingerprint,
            &self.changed_paths,
            outcome.as_ref(),
        )
        .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidResourceAuthoringEvent))
    }

    /// The failure of a failed promotion as the ledger records it (ledger revision 3).
    pub fn outcome(&self) -> Option<crate::outcome::Outcome> {
        self.failure_code
            .as_deref()
            .map(crate::outcome::Outcome::relayed)
    }

    pub const fn phase(&self) -> ResourceAuthoringPhase {
        self.phase
    }

    pub fn draft_id(&self) -> &str {
        &self.draft_id
    }

    pub fn target_label(&self) -> &str {
        &self.target_label
    }

    pub fn target_fingerprint(&self) -> &str {
        &self.target_fingerprint
    }

    pub fn changed_paths(&self) -> &[String] {
        &self.changed_paths
    }

    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
    }
}

impl ReadonlyObservationOutcome {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Completed { observation } => observation.validate(),
            Self::Failed {
                stage: ReadonlyObservationStage::Capture,
                captured_frame: None,
            } => Ok(()),
            Self::Failed {
                stage: ReadonlyObservationStage::Recognition,
                captured_frame: Some(frame),
            } => frame.validate(),
            Self::Failed { .. } => Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationFailureContext,
            )),
        }
    }
}

/// Workflow #336 PKG-S4 (option 丙): a package is named by its path only; the request carries
/// no digest, and a request that still names `expected_sha256` does not decode.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDebugRequest {
    package_path: String,
}

impl PackageDebugRequest {
    pub fn new(package_path: impl Into<String>) -> RuntimeContractResult<Self> {
        let request = Self {
            package_path: package_path.into(),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.package_path.trim().is_empty()
            || self.package_path.len() > MAX_DEBUG_PACKAGE_PATH_BYTES
            || self.package_path.contains('\0')
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidDebugPackagePath,
            ));
        }
        Ok(())
    }

    pub fn package_path(&self) -> &str {
        &self.package_path
    }
}

impl fmt::Debug for PackageDebugRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PackageDebugRequest")
            .field("package_path", &"<redacted-path>")
            .finish()
    }
}

/// Workflow #336 PKG-S4: the recovery package by its path only, as `ContainedTaskRequest`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedTaskRecoveryBinding {
    package_path: String,
}

impl ContainedTaskRecoveryBinding {
    pub fn new(package_path: impl Into<String>) -> RuntimeContractResult<Self> {
        let binding = Self {
            package_path: package_path.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.package_path.trim().is_empty()
            || self.package_path.len() > MAX_CONTAINED_TASK_PATH_BYTES
            || self.package_path.contains('\0')
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidContainedTaskRecoveryPath,
            ));
        }
        Ok(())
    }

    pub fn package_path(&self) -> &str {
        &self.package_path
    }
}

impl fmt::Debug for ContainedTaskRecoveryBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContainedTaskRecoveryBinding")
            .field("package_path", &"<redacted-path>")
            .finish()
    }
}

/// Workflow #336 PKG-S4 (option 丙): the package of a contained task is found by its path; the
/// path chooses how it is read and its admission is structural only. The request carries no
/// digest: a request that still names `expected_sha256` does not decode
/// (`deny_unknown_fields`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainedTaskRequest {
    package_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery: Option<ContainedTaskRecoveryBinding>,
    #[serde(default = "default_contained_task_response_deadline_ms")]
    response_deadline_ms: u64,
}

const fn default_contained_task_response_deadline_ms() -> u64 {
    ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS
}

impl ContainedTaskRequest {
    pub const DEFAULT_RESPONSE_DEADLINE_MS: u64 = 60_000;
    pub const MAX_RESPONSE_DEADLINE_MS: u64 = crate::MAX_CONTAINED_TASK_TIMEOUT_MS;

    pub fn new(package_path: impl Into<String>) -> RuntimeContractResult<Self> {
        let request = Self {
            package_path: package_path.into(),
            recovery: None,
            response_deadline_ms: Self::DEFAULT_RESPONSE_DEADLINE_MS,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.package_path.trim().is_empty()
            || self.package_path.len() > MAX_CONTAINED_TASK_PATH_BYTES
            || self.package_path.contains('\0')
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidContainedTaskPath,
            ));
        }
        if let Some(recovery) = &self.recovery {
            recovery.validate()?;
        }
        if !(1..=Self::MAX_RESPONSE_DEADLINE_MS).contains(&self.response_deadline_ms) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidContainedTaskResponseDeadline,
            ));
        }
        Ok(())
    }

    pub fn package_path(&self) -> &str {
        &self.package_path
    }

    pub const fn recovery(&self) -> Option<&ContainedTaskRecoveryBinding> {
        self.recovery.as_ref()
    }

    pub const fn response_deadline_ms(&self) -> u64 {
        self.response_deadline_ms
    }

    pub fn with_response_deadline_ms(
        mut self,
        response_deadline_ms: u64,
    ) -> RuntimeContractResult<Self> {
        self.response_deadline_ms = response_deadline_ms;
        self.validate()?;
        Ok(self)
    }

    pub fn with_recovery(
        mut self,
        recovery: ContainedTaskRecoveryBinding,
    ) -> RuntimeContractResult<Self> {
        self.recovery = Some(recovery);
        self.validate()?;
        Ok(self)
    }
}

impl fmt::Debug for ContainedTaskRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContainedTaskRequest")
            .field("package_path", &"<redacted-path>")
            .field("recovery", &self.recovery)
            .field("response_deadline_ms", &self.response_deadline_ms)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceExportRequest {
    output_path: String,
    task_outcome: TaskOutcome,
}

impl RuntimeEvidenceExportRequest {
    pub fn new(
        output_path: impl Into<String>,
        task_outcome: TaskOutcome,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            output_path: output_path.into(),
            task_outcome,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.output_path.trim().is_empty()
            || self.output_path.len() > MAX_EVIDENCE_OUTPUT_PATH_BYTES
            || self.output_path.contains('\0')
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEvidenceOutputPath,
            ));
        }
        Ok(())
    }

    pub fn output_path(&self) -> &str {
        &self.output_path
    }

    pub const fn task_outcome(&self) -> TaskOutcome {
        self.task_outcome
    }
}

impl fmt::Debug for RuntimeEvidenceExportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEvidenceExportRequest")
            .field("output_path", &"<redacted-path>")
            .field("task_outcome", &self.task_outcome)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceScreenshotCounts {
    pub captured: u64,
    pub deduplicated: u64,
    pub dropped: u64,
    pub persisted: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceExportSummary {
    correlation_id: CorrelationId,
    run_id: RunId,
    task_outcome: TaskOutcome,
    evidence_completeness: EvidenceCompleteness,
    normalized_output_path: String,
    zip_byte_count: u64,
    zip_sha256: String,
    manifest_sha256: String,
    archive: ProjectedArtifactReference,
    screenshot_counts: RuntimeEvidenceScreenshotCounts,
    terminal_receipt: ProjectedEvent,
}

impl RuntimeEvidenceExportSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        correlation_id: CorrelationId,
        run_id: RunId,
        task_outcome: TaskOutcome,
        evidence_completeness: EvidenceCompleteness,
        normalized_output_path: impl Into<String>,
        zip_byte_count: u64,
        zip_sha256: impl Into<String>,
        manifest_sha256: impl Into<String>,
        archive: ProjectedArtifactReference,
        screenshot_counts: RuntimeEvidenceScreenshotCounts,
        terminal_receipt: ProjectedEvent,
    ) -> RuntimeContractResult<Self> {
        let summary = Self {
            correlation_id,
            run_id,
            task_outcome,
            evidence_completeness,
            normalized_output_path: normalized_output_path.into(),
            zip_byte_count,
            zip_sha256: zip_sha256.into(),
            manifest_sha256: manifest_sha256.into(),
            archive,
            screenshot_counts,
            terminal_receipt,
        };
        summary.validate()?;
        Ok(summary)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.normalized_output_path.trim().is_empty()
            || self.normalized_output_path.len() > MAX_EVIDENCE_OUTPUT_PATH_BYTES
            || self.normalized_output_path.contains('\0')
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEvidenceOutputPath,
            ));
        }
        if self.zip_byte_count == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEvidenceZipCount,
            ));
        }
        let accounted = self
            .screenshot_counts
            .deduplicated
            .checked_add(self.screenshot_counts.dropped)
            .and_then(|count| count.checked_add(self.screenshot_counts.persisted));
        if accounted.is_none_or(|count| count > self.screenshot_counts.captured) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEvidenceScreenshotCounts,
            ));
        }
        if self.archive.kind() != ArtifactKind::EvidenceArchive
            || self.archive.byte_count() != self.zip_byte_count
            || self.archive.sha256() != self.zip_sha256
            || self.archive.run_id != Some(self.run_id)
            || self.archive.correlation_id != Some(self.correlation_id)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEvidenceArchiveReference,
            ));
        }
        if self.terminal_receipt.sequence == 0
            || self.terminal_receipt.links.run_id() != Some(&self.run_id)
            || self.terminal_receipt.links.correlation_id() != Some(&self.correlation_id)
            || self.terminal_receipt.event_type != terminal_event_type(self.task_outcome)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEvidenceTerminalReceipt,
            ));
        }
        validate_canonical_sha256(&self.zip_sha256)
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidEvidenceZipHash))?;
        validate_canonical_sha256(&self.manifest_sha256).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidEvidenceManifestHash)
        })?;
        self.archive.validate().map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidEvidenceArchiveReference)
        })
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    pub const fn task_outcome(&self) -> TaskOutcome {
        self.task_outcome
    }

    pub const fn evidence_completeness(&self) -> EvidenceCompleteness {
        self.evidence_completeness
    }

    pub fn normalized_output_path(&self) -> &str {
        &self.normalized_output_path
    }

    pub const fn zip_byte_count(&self) -> u64 {
        self.zip_byte_count
    }

    pub fn zip_sha256(&self) -> &str {
        &self.zip_sha256
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub const fn archive(&self) -> &ProjectedArtifactReference {
        &self.archive
    }

    pub const fn screenshot_counts(&self) -> RuntimeEvidenceScreenshotCounts {
        self.screenshot_counts
    }

    pub const fn terminal_receipt(&self) -> &ProjectedEvent {
        &self.terminal_receipt
    }
}

impl fmt::Debug for RuntimeEvidenceExportSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEvidenceExportSummary")
            .field("correlation_id", &self.correlation_id)
            .field("run_id", &self.run_id)
            .field("task_outcome", &self.task_outcome)
            .field("evidence_completeness", &self.evidence_completeness)
            .field("normalized_output_path", &"<redacted-path>")
            .field("zip_byte_count", &self.zip_byte_count)
            .field("zip_sha256", &"<redacted-hash>")
            .field("manifest_sha256", &"<redacted-hash>")
            .field("archive", &self.archive)
            .field("screenshot_counts", &self.screenshot_counts)
            .field("terminal_receipt", &self.terminal_receipt)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageDebugLayout {
    Lab,
    Module,
}

/// Workflow #336 PKG-S4: the summary of an admitted package names no digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDebugSummary {
    task_id: String,
    layout: PackageDebugLayout,
    entry_count: u32,
    resident_bytes: u64,
    task_count: u32,
    has_recognition_pack: bool,
    has_pages: bool,
    has_navigation: bool,
}

impl PackageDebugSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_id: impl Into<String>,
        layout: PackageDebugLayout,
        entry_count: u32,
        resident_bytes: u64,
        task_count: u32,
        has_recognition_pack: bool,
        has_pages: bool,
        has_navigation: bool,
    ) -> RuntimeContractResult<Self> {
        let summary = Self {
            task_id: task_id.into(),
            layout,
            entry_count,
            resident_bytes,
            task_count,
            has_recognition_pack,
            has_pages,
            has_navigation,
        };
        summary.validate()?;
        Ok(summary)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.task_id.trim().is_empty()
            || self.task_id.len() > MAX_INSTANCE_ALIAS_BYTES
            || self.entry_count == 0
            || self.resident_bytes == 0
            || self.task_count == 0
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidDebugPackageSummary,
            ));
        }
        Ok(())
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub const fn layout(&self) -> PackageDebugLayout {
        self.layout
    }

    pub const fn entry_count(&self) -> u32 {
        self.entry_count
    }

    pub const fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    pub const fn task_count(&self) -> u32 {
        self.task_count
    }

    pub const fn has_recognition_pack(&self) -> bool {
        self.has_recognition_pack
    }

    pub const fn has_pages(&self) -> bool {
        self.has_pages
    }

    pub const fn has_navigation(&self) -> bool {
        self.has_navigation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSubscriptionRequest {
    query: EventQuery,
    profile: ProjectionProfile,
    cursor: SubscriptionCursor,
    wait_ms: u64,
    max_events: u16,
}

impl RuntimeSubscriptionRequest {
    pub fn new(
        query: EventQuery,
        profile: ProjectionProfile,
        cursor: SubscriptionCursor,
        wait_ms: u64,
        max_events: u16,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            query,
            profile,
            cursor,
            wait_ms,
            max_events,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.query
            .validate()
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidEventQueryBounds))?;
        if self.wait_ms > MAX_RUNTIME_SUBSCRIPTION_WAIT_MS
            || self.max_events == 0
            || self.max_events > MAX_RUNTIME_SUBSCRIPTION_EVENTS
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeSubscriptionRequest,
            ));
        }
        Ok(())
    }

    pub const fn query(&self) -> &EventQuery {
        &self.query
    }

    pub const fn profile(&self) -> ProjectionProfile {
        self.profile
    }

    pub const fn cursor(&self) -> SubscriptionCursor {
        self.cursor
    }

    pub const fn wait_ms(&self) -> u64 {
        self.wait_ms
    }

    pub const fn max_events(&self) -> u16 {
        self.max_events
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEventBatch {
    events: Vec<ProjectedEvent>,
    next_cursor: SubscriptionCursor,
    timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEventQueryCursor {
    snapshot_ledger_position: u64,
    after_sequence: u64,
    query_fingerprint: String,
}

impl RuntimeEventQueryCursor {
    pub fn new(
        snapshot_ledger_position: u64,
        after_sequence: u64,
        query: &EventQuery,
        profile: ProjectionProfile,
    ) -> RuntimeContractResult<Self> {
        let cursor = Self {
            snapshot_ledger_position,
            after_sequence,
            query_fingerprint: event_query_fingerprint(query, profile)?,
        };
        cursor.validate()?;
        Ok(cursor)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.snapshot_ledger_position == 0
            || self.after_sequence == 0
            || self.after_sequence > self.snapshot_ledger_position
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeEventQueryCursor,
            ));
        }
        validate_canonical_sha256(&self.query_fingerprint).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeEventQueryCursor)
        })
    }

    pub fn matches(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
    ) -> RuntimeContractResult<bool> {
        Ok(self.query_fingerprint == event_query_fingerprint(query, profile)?)
    }

    pub const fn snapshot_ledger_position(&self) -> u64 {
        self.snapshot_ledger_position
    }

    pub const fn after_sequence(&self) -> u64 {
        self.after_sequence
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEventQueryPageRequest {
    limit: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<RuntimeEventQueryCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot_position: Option<u64>,
}

impl RuntimeEventQueryPageRequest {
    pub fn new(limit: u16, cursor: Option<RuntimeEventQueryCursor>) -> RuntimeContractResult<Self> {
        let request = Self {
            limit,
            cursor,
            snapshot_position: None,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.limit == 0 || self.limit > MAX_RUNTIME_EVENT_QUERY_EVENTS {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeEventQueryPage,
            ));
        }
        if let Some(cursor) = &self.cursor {
            cursor.validate()?;
            if self
                .snapshot_position
                .is_some_and(|position| position != cursor.snapshot_ledger_position())
            {
                return Err(RuntimeContractError::new(
                    SharedCode::InvalidRuntimeEventQuerySnapshot,
                ));
            }
        }
        Ok(())
    }

    pub const fn limit(&self) -> u16 {
        self.limit
    }

    pub const fn cursor(&self) -> Option<&RuntimeEventQueryCursor> {
        self.cursor.as_ref()
    }

    pub fn at_snapshot(mut self, position: u64) -> RuntimeContractResult<Self> {
        self.snapshot_position = Some(position);
        self.validate()?;
        Ok(self)
    }

    pub const fn snapshot_position(&self) -> Option<u64> {
        self.snapshot_position
    }
}

impl Default for RuntimeEventQueryPageRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_RUNTIME_EVENT_QUERY_EVENTS,
            cursor: None,
            snapshot_position: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEventQueryPage {
    events: Vec<ProjectedEvent>,
    snapshot_ledger_position: u64,
    requested_limit: u16,
    returned_count: u16,
    has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_cursor: Option<RuntimeEventQueryCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    read_scope: Option<crate::LedgerReadScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    run_recovery: Vec<crate::LedgerRunRecovery>,
}

impl RuntimeEventQueryPage {
    pub fn new(
        events: Vec<ProjectedEvent>,
        snapshot_ledger_position: u64,
        requested_limit: u16,
        has_more: bool,
        next_cursor: Option<RuntimeEventQueryCursor>,
    ) -> RuntimeContractResult<Self> {
        let returned_count = u16::try_from(events.len()).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeEventQueryPage)
        })?;
        let page = Self {
            events,
            snapshot_ledger_position,
            requested_limit,
            returned_count,
            has_more,
            next_cursor,
            read_scope: None,
            run_recovery: Vec::new(),
        };
        page.validate()?;
        Ok(page)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.requested_limit == 0
            || self.requested_limit > MAX_RUNTIME_EVENT_QUERY_EVENTS
            || self.returned_count as usize != self.events.len()
            || self.returned_count > self.requested_limit
            || self.has_more != self.next_cursor.is_some()
            || (self.events.is_empty() && self.has_more)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeEventQueryPage,
            ));
        }
        let mut previous = 0;
        for event in &self.events {
            if event.sequence <= previous || event.sequence > self.snapshot_ledger_position {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeEventQueryPage,
                ));
            }
            previous = event.sequence;
            let mut artifacts = std::collections::BTreeSet::new();
            for observation in &event.artifact_evictions {
                observation.validate().map_err(|_| {
                    RuntimeContractError::new(
                        RuntimeContractCode::InvalidRuntimeArtifactEvictionObservation,
                    )
                })?;
                if observation.through_sequence > self.snapshot_ledger_position
                    || !artifacts.insert(observation.artifact_id)
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidRuntimeArtifactEvictionObservation,
                    ));
                }
            }
        }
        if let Some(cursor) = &self.next_cursor {
            cursor.validate()?;
            if cursor.snapshot_ledger_position != self.snapshot_ledger_position
                || self.events.last().map(|event| event.sequence) != Some(cursor.after_sequence)
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeEventQueryPage,
                ));
            }
        }
        if let Some(scope) = &self.read_scope {
            if scope.scanned_through_position > self.snapshot_ledger_position
                || self
                    .events
                    .last()
                    .is_some_and(|event| event.sequence > scope.scanned_through_position)
                || scope.read_complete
                    == scope
                        .limits
                        .contains(&crate::LedgerPageLimit::SourceIncomplete)
                || (self.has_more
                    != (scope.limits.contains(&crate::LedgerPageLimit::EventCount)
                        || scope
                            .limits
                            .contains(&crate::LedgerPageLimit::ResponseBytes)))
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeEventReadScope,
                ));
            }
        } else if !self.run_recovery.is_empty() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeEventReadScope,
            ));
        }
        let mut runs = std::collections::BTreeSet::new();
        for group in &self.run_recovery {
            if !runs.insert(group.run_id)
                || !self
                    .events
                    .iter()
                    .any(|event| event.links.run_id() == Some(&group.run_id))
                || (group.state == crate::LedgerRecoveryState::Unknown) == group.gaps.is_empty()
                || (group.state == crate::LedgerRecoveryState::Recovered
                    && (group.evidence.is_empty()
                        || group.evidence.iter().any(|item| item.success.is_none())))
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeEventRecovery,
                ));
            }
            let mut previous_failure = 0;
            for item in &group.evidence {
                if item.failure.sequence <= previous_failure
                    || item.failure.sequence > self.snapshot_ledger_position
                    || item.success.is_some_and(|success| {
                        success.sequence <= item.failure.sequence
                            || success.sequence > self.snapshot_ledger_position
                            || success.event_id == item.failure.event_id
                    })
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidRuntimeEventRecovery,
                    ));
                }
                previous_failure = item.failure.sequence;
            }
        }
        let encoded = serde_json::to_vec(self).map_err(|_| {
            RuntimeContractError::new(RuntimeContractCode::RuntimeEventQueryPageEncodeFailed)
        })?;
        if encoded.len() > MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES {
            return Err(RuntimeContractError::new(
                SharedCode::RuntimeEventQueryResponseTooLarge,
            ));
        }
        Ok(())
    }

    pub fn events(&self) -> &[ProjectedEvent] {
        &self.events
    }

    pub const fn snapshot_ledger_position(&self) -> u64 {
        self.snapshot_ledger_position
    }

    pub const fn requested_limit(&self) -> u16 {
        self.requested_limit
    }

    pub const fn returned_count(&self) -> u16 {
        self.returned_count
    }

    pub const fn has_more(&self) -> bool {
        self.has_more
    }

    pub const fn next_cursor(&self) -> Option<&RuntimeEventQueryCursor> {
        self.next_cursor.as_ref()
    }

    pub fn with_projection_context(
        mut self,
        scope: crate::LedgerReadScope,
        run_recovery: Vec<crate::LedgerRunRecovery>,
    ) -> RuntimeContractResult<Self> {
        self.read_scope = Some(scope);
        self.run_recovery = run_recovery;
        self.validate()?;
        Ok(self)
    }

    pub const fn read_scope(&self) -> Option<&crate::LedgerReadScope> {
        self.read_scope.as_ref()
    }

    pub fn run_recovery(&self) -> &[crate::LedgerRunRecovery] {
        &self.run_recovery
    }
}

fn event_query_fingerprint(
    query: &EventQuery,
    profile: ProjectionProfile,
) -> RuntimeContractResult<String> {
    query
        .validate()
        .map_err(|error| RuntimeContractError::new(error.code()))?;
    let bytes = serde_json::to_vec(&(query, profile)).map_err(|_| {
        RuntimeContractError::new(RuntimeContractCode::RuntimeEventQueryFingerprintFailed)
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

impl RuntimeEventBatch {
    pub fn new(
        events: Vec<ProjectedEvent>,
        next_cursor: SubscriptionCursor,
        timed_out: bool,
    ) -> RuntimeContractResult<Self> {
        let batch = Self {
            events,
            next_cursor,
            timed_out,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.events.len() > usize::from(MAX_RUNTIME_SUBSCRIPTION_EVENTS)
            || self.events.is_empty() != self.timed_out
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeEventBatch,
            ));
        }
        let mut previous = 0;
        for event in &self.events {
            if event.sequence <= previous || event.sequence > self.next_cursor.after_sequence {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeEventBatch,
                ));
            }
            previous = event.sequence;
        }
        Ok(())
    }

    pub fn events(&self) -> &[ProjectedEvent] {
        &self.events
    }

    pub const fn next_cursor(&self) -> SubscriptionCursor {
        self.next_cursor
    }

    pub const fn timed_out(&self) -> bool {
        self.timed_out
    }
}

/// Upper bound on the changes of one `ConfigureSettings` (Workflow #387 D1b).
pub const MAX_CONFIG_SETTING_CHANGES: usize = 64;
/// Upper bound on one changed value, serialized.
pub const MAX_CONFIG_SETTING_VALUE_BYTES: usize = 8 * 1024;

/// One change of a `ConfigureSettings` request (Workflow #387 D1b, model v1.2 §5.6): `key` as
/// `config.parameters` names it, and its new value; without a value the key returns to its
/// configuration-file value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSettingChange {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// One setting a `ConfigureSettings` changed, as it is in effect afterwards (the receipt,
/// Workflow #387 model v1.2 §5.6): its value and where the value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredSetting {
    pub key: String,
    pub value: crate::FactScalar,
    pub source: crate::ConfigParameterSource,
}

/// `ConfigureSettings`'s shape: 1 to [`MAX_CONFIG_SETTING_CHANGES`] changes, each key named once,
/// at most [`crate::MAX_CONFIG_MANIFEST_NAME_BYTES`] bytes of `a-z`, `0-9`, `_`, `.`, `:` and `-`
/// (every configuration key's spelling, and the outcome key `config_key`'s), each value at most
/// [`MAX_CONFIG_SETTING_VALUE_BYTES`] serialized. Whether a key exists, may change and takes the
/// value is the Runtime's answer.
fn validate_config_setting_changes(changes: &[ConfigSettingChange]) -> RuntimeContractResult<()> {
    let invalid = || RuntimeContractError::new(RuntimeContractCode::InvalidConfigureSettings);
    if changes.is_empty() || changes.len() > MAX_CONFIG_SETTING_CHANGES {
        return Err(invalid());
    }
    let mut keys = BTreeSet::new();
    for change in changes {
        if change.key.is_empty()
            || change.key.len() > crate::MAX_CONFIG_MANIFEST_NAME_BYTES
            || !change.key.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'.' | b':' | b'-')
            })
            || !keys.insert(change.key.as_str())
        {
            return Err(invalid());
        }
        if let Some(value) = &change.value
            && !serde_json::to_vec(value)
                .is_ok_and(|bytes| bytes.len() <= MAX_CONFIG_SETTING_VALUE_BYTES)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeOperation {
    Health,
    RequestShutdown {
        target: RuntimeShutdownTarget,
    },
    InstallTransition {
        target: RuntimeShutdownTarget,
        action: crate::InstallTransitionAction,
    },
    Status,
    ProjectInterface {
        request: ProjectInterfaceRequest,
    },
    ProjectPolicyInputIdentity {
        as_of_ledger_position: u64,
    },
    MonitorStatus,
    RuntimeFactSnapshot,
    /// Reads one instance's facts (Workflow #389 MCP-4, register W2): a general read from memory
    /// that records nothing, like `Status`. `instance` is the registered instance alias; the
    /// answer is `RuntimeResult::InstanceFactSnapshot`.
    InstanceFactSnapshot {
        instance: String,
    },
    /// Reads the shadow measurement's state (Workflow #385 P-m, PERF §4.6; register §1.61 W15):
    /// a general read from memory that reads and records nothing in the ledger. The answer is
    /// `RuntimeResult::PerformanceStatus`.
    PerformanceStatus,
    /// Reads what the scheduler's fact refresh waits for now (Workflow #384 D2-S5, D2 model
    /// §6 S5; register §1.68): a general read from memory that reads and records nothing in the
    /// ledger. The answer is `RuntimeResult::FactRefreshStatus`.
    FactRefreshStatus,
    /// Reads why each candidate of the latest policy evaluation did not run (Workflow #386 SR-6b,
    /// MCP-19a; register §1.143, §3 W5): a general read from the policy host's memory that reads
    /// and records nothing in the ledger. `instance` is a registered instance alias (every
    /// instance without it); `candidates: false` leaves out the candidate rows. The answer is
    /// `RuntimeResult::ScheduleExplanation`.
    ScheduleExplanation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance: Option<String>,
        candidates: bool,
    },
    ConfigureMonitor {
        instance_alias: String,
        policy: RuntimeMonitorPolicy,
    },
    ClearMonitor {
        instance_alias: String,
    },
    AcquireLease {
        instance_alias: String,
        holder_id: HolderId,
    },
    QueueLease {
        instance_alias: String,
        holder_id: HolderId,
        policy: LeaseQueuePolicy,
    },
    PollQueuedLease {
        queued_request_id: RequestId,
    },
    CancelQueuedLease {
        queued_request_id: RequestId,
    },
    CancelContainedTask {
        task_request_id: RequestId,
    },
    RenewLease {
        token: LeaseToken,
    },
    ReleaseLease {
        token: LeaseToken,
    },
    ObserveReadonly {
        instance_alias: String,
    },
    RecognizeArtifact {
        request: Box<SavedArtifactOcrRequest>,
    },
    ObserveContainedPage {
        instance_alias: String,
        request: ContainedObservationRequest,
    },
    RunContainedLabOperation {
        instance_alias: String,
        holder_id: HolderId,
        request: ContainedLabOperationRequest,
    },
    CaptureSequence {
        instance_alias: String,
        spec: CaptureSequenceSpec,
    },
    SafeReset {
        instance_alias: String,
        holder_id: HolderId,
    },
    ApplicationLifecycle {
        instance_alias: String,
        holder_id: HolderId,
        action: ApplicationLifecycleAction,
    },
    /// Start, stop or restart the emulator instance through its discovered provider. Only an
    /// explicit User+Ui or Cli+Cli request may issue it; no lease is held, the per-instance
    /// fence is checked and the device session is closed before the provider is driven.
    ControlEmulatorInstance {
        instance_alias: String,
        action: EmulatorInstanceAction,
    },
    /// Re-runs the provider's instance discovery on demand and reports every instance with
    /// its bound alias. Spawns the vendor tool, so the origin gate is the emulator control
    /// one; binds, leases and opens nothing and records one `command.validated` observation.
    DiscoverInstances,
    /// Pauses policy dispatch (Workflow #191 ps1). Only an explicit User+Ui or Cli+Cli request
    /// may issue it. `Global` closes the dispatch gate for every instance; `Instance` closes it
    /// for one physical instance and then drains that instance's in-flight contained runs:
    /// they finish on their own within `drain_timeout_ms` (`1_000..=600_000`, validated for
    /// both scopes, used only by `Instance`) or are asked to stop at their next checkpoint.
    PauseScheduling {
        scope: SchedulingPauseScope,
        pause_reason: SchedulingPauseReason,
        drain_timeout_ms: u64,
    },
    /// Lifts the matching scheduling pause; same origin gate as `PauseScheduling`. With
    /// `expected` (Workflow #338 R4) only the pause of that owner epoch and scope revision is
    /// lifted; without it the resume is unconditional, as before.
    ResumeScheduling {
        scope: SchedulingPauseScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected: Option<SchedulingPauseExpectation>,
    },
    /// Reconnects one physical instance and self-checks it now (Workflow #317 sc3): the
    /// host's controlled preparation phase, under a dedicated preparation lease, closes a
    /// session the instance still holds (Workflow #191 h3), opens its input and capture
    /// backends and records the opens. The new session then stays open for the instance's
    /// next leases until an external command disconnects it (Workflow #191 H); a failed open,
    /// or an instance outside the multi-Nemu gate, closes it again. Only an explicit User+Ui or
    /// Cli+Cli request may issue it (the emulator control origin gate).
    SelfCheckInstance {
        instance_alias: String,
    },
    /// Workflow #390 DRILL-1 (forced compromise T-08): a person's drill of one physical
    /// instance's stuck-recovery ladder, an option-two operation. The drill is admitted, climbs
    /// and is recorded like a ladder a trigger started, its own records tagged `drill`; with
    /// `mode` `plan` it never acts on the device, with `device` it climbs its rungs on the
    /// instance under the same key, admission and fences as a real ladder. A direct task run
    /// still starts no ladder. Only an explicit User+Ui or Cli+Cli request may issue it (the
    /// emulator control origin gate: a drill may restart the emulator).
    DrillRecoveryLadder {
        instance_alias: String,
        mode: crate::RecoveryDrillMode,
    },
    RunContainedTask {
        instance_alias: String,
        holder_id: HolderId,
        request: ContainedTaskRequest,
    },
    Input {
        token: LeaseToken,
        action: InputAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<InputFrameReference>,
    },
    PublishFact {
        record: FactRecord,
    },
    PublishFacts {
        observation: crate::FactObservation,
    },
    QueryEvents {
        query: EventQuery,
        profile: ProjectionProfile,
        page: RuntimeEventQueryPageRequest,
    },
    ReadMaterial {
        request: Box<RuntimeMaterialReadRequest>,
    },
    SubscribeEvents {
        request: RuntimeSubscriptionRequest,
    },
    RegisterDiagnosticSignature {
        definition: Box<crate::DiagnosticSignatureDefinition>,
    },
    MatchDiagnosticSignatures {
        request: Box<crate::RuntimeSignatureMatchRequest>,
    },
    RetireDiagnosticSignature {
        registration: crate::SignatureRegistrationRef,
    },
    DebugPackage {
        request: PackageDebugRequest,
    },
    ExportEvidence {
        request: RuntimeEvidenceExportRequest,
    },
    RecordAuthoringEvent {
        event: ResourceAuthoringEvent,
    },
    RecordDebugEvent {
        event: RuntimeDebugEvent,
    },
    ReleaseLabPin {
        target: crate::LabPinReleaseTarget,
    },
    RecordClientAction {
        action: ClientActionRecord,
    },
    /// Declares who this connection is for governance writes (Workflow #318 cfg4). The
    /// actor and source stay on the request envelope; the card never repeats them.
    DeclareGovernanceIdentity {
        card: GovernanceIdentityCard,
    },
    RecordApprovalDecision {
        decision: ApprovalDecisionRecord,
    },
    StartAgentSession {
        wake_id: AgentWakeId,
    },
    ResumeAgentSession {
        session_id: AgentSessionId,
    },
    AgentSessionStatus {
        session_id: AgentSessionId,
    },
    RecordAgentResponse {
        response: AgentSessionResponse,
    },
    PrepareStrategicReport {
        request: Box<RuntimeStrategicReportRequest>,
    },
    ProjectPolicyForward {
        request: Box<RuntimeForwardProjectionRequest>,
    },
    AssessPredictiveMaintenance {
        query: Box<RuntimeMaintenanceQuery>,
    },
    CompileProposal {
        proposal: Box<CatalogProposal>,
    },
    PromoteProposal {
        proposal: Box<CatalogProposal>,
    },
    /// Applies one instance resource target policy (Workflow #308 RT-S1a): the raw
    /// `actingcommand.resource-targets.v1` document (`1..=64 KiB`), parsed, checked and
    /// stored as the instance fact `session.resource_targets` by the Runtime, the only formal
    /// entry of that fact. Agent/Adapter only.
    ApplyResourceTargets {
        document_json: String,
    },
    /// Reads what one instance can target and its stored resource target policy (Workflow #338
    /// R5). Read-only: records nothing and has no origin gate, like `Status`.
    ResourceTargetView {
        instance_alias: String,
    },
    /// Changes settings the Runtime may change while it runs (Workflow #387 D1b, model v1.2
    /// §5.6), all of them or none: a general instruction the three counters send, recorded as
    /// a command with the new `host.config_overrides` and `config.parameters.<section>` facts.
    /// A change without a value returns its key to the configuration-file value.
    ConfigureSettings {
        changes: Vec<ConfigSettingChange>,
    },
    // Workflow #389 U-1: the shapes of the unified interface's new request kinds. Each slice
    // named below adds the behaviour; until then the host refuses the kind `not_implemented`.
    /// Binds this connection to its channel, once, right after `Health` (section 3.2; U-2).
    /// `via` names the counter that handed a Lab request over, on the Lab channel only.
    BindChannel {
        channel: RuntimeChannel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<ChannelVia>,
    },
    /// The upgrade state (section 3.6, install-control revision 2; U-4): the installer's
    /// instruction family for an upgrade. `target` is the Runtime it is meant for, as in
    /// `InstallTransition`.
    UpgradeState {
        target: RuntimeShutdownTarget,
        action: crate::UpgradeStateAction,
    },
    /// Reads the phase of one upgrade state from memory and records nothing (section 3.6; U-4).
    UpgradeStateStatus {
        transition_id: String,
    },
    /// Puts one package into the loaded-package table, in memory only (section 6; U-3).
    LoadPack {
        entry: PackLoadEntry,
    },
    /// Takes one entry out of the loaded-package table, stopping first the runs that use it,
    /// and answers once they ended and reset or after `wait_ms` (section 6.4; U-3).
    UnloadPack {
        key: PackRegistryKey,
        wait_ms: u64,
    },
}

impl RuntimeOperation {
    pub fn acquire_lease(instance_alias: impl Into<String>, holder_id: IssuedHolderId) -> Self {
        Self::AcquireLease {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
        }
    }

    pub fn queue_lease(
        instance_alias: impl Into<String>,
        holder_id: IssuedHolderId,
        policy: LeaseQueuePolicy,
    ) -> Self {
        Self::QueueLease {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
            policy,
        }
    }

    pub fn safe_reset(instance_alias: impl Into<String>, holder_id: IssuedHolderId) -> Self {
        Self::SafeReset {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
        }
    }

    pub fn application_lifecycle(
        instance_alias: impl Into<String>,
        holder_id: IssuedHolderId,
        action: ApplicationLifecycleAction,
    ) -> Self {
        Self::ApplicationLifecycle {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
            action,
        }
    }

    pub fn run_contained_task(
        instance_alias: impl Into<String>,
        holder_id: IssuedHolderId,
        request: ContainedTaskRequest,
    ) -> Self {
        Self::RunContainedTask {
            instance_alias: instance_alias.into(),
            holder_id: *holder_id.transport(),
            request,
        }
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::RequestShutdown { target } => target.validate(),
            Self::InstallTransition { target, action } => {
                target.validate()?;
                action.validate()
            }
            Self::Health
            | Self::Status
            | Self::MonitorStatus
            | Self::RuntimeFactSnapshot
            | Self::PerformanceStatus
            | Self::FactRefreshStatus
            | Self::DiscoverInstances
            | Self::PollQueuedLease { .. }
            | Self::CancelQueuedLease { .. }
            | Self::CancelContainedTask { .. } => Ok(()),
            Self::QueryEvents { query, page, .. } => {
                query.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidEventQueryBounds)
                })?;
                page.validate()
            }
            Self::ReadMaterial { request } => request.validate(),
            Self::ProjectInterface { request } => request.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidProjectInterfaceRequest)
            }),
            Self::ProjectPolicyInputIdentity {
                as_of_ledger_position,
            } => {
                if *as_of_ledger_position == 0 {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidPolicyInputPosition,
                    ));
                }
                Ok(())
            }
            Self::SubscribeEvents { request } => request.validate(),
            Self::RegisterDiagnosticSignature { definition } => {
                definition.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidSignatureDefinition)
                })
            }
            Self::MatchDiagnosticSignatures { request } => request.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidSignatureMatchRequest)
            }),
            Self::RetireDiagnosticSignature { registration } => {
                registration.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidSignatureRegistration)
                })
            }
            Self::DebugPackage { request } => request.validate(),
            Self::ExportEvidence { request } => request.validate(),
            Self::RecordAuthoringEvent { event } => event.validate(),
            Self::RecordDebugEvent { event } => event.validate(),
            Self::ReleaseLabPin { target } => target
                .validate()
                .map_err(|_| RuntimeContractError::new(SharedCode::InvalidLabPinReleaseTarget)),
            Self::RecordClientAction { action } => action
                .validate()
                .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidClientAction)),
            Self::PublishFact { record } => record
                .validate()
                .map_err(|error| RuntimeContractError::new(error.code())),
            Self::PublishFacts { observation } => observation
                .validate()
                .map_err(|error| RuntimeContractError::new(error.code())),
            Self::DeclareGovernanceIdentity { card } => card.validate(),
            Self::RecordApprovalDecision { decision } => decision.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidApprovalDecision)
            }),
            Self::StartAgentSession { .. }
            | Self::ResumeAgentSession { .. }
            | Self::AgentSessionStatus { .. } => Ok(()),
            Self::RecordAgentResponse { response } => response
                .validate()
                .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidAgentResponse)),
            Self::PrepareStrategicReport { request } => request.validate(),
            Self::ProjectPolicyForward { request } => request.validate(),
            Self::AssessPredictiveMaintenance { query } => query.validate(),
            Self::CompileProposal { proposal } | Self::PromoteProposal { proposal } => {
                proposal.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidCatalogProposal)
                })
            }
            Self::ApplyResourceTargets { document_json } => {
                if document_json.is_empty()
                    || document_json.len() > crate::MAX_RESOURCE_TARGETS_DOCUMENT_BYTES
                {
                    return Err(RuntimeContractError::new(
                        SharedCode::ResourceTargetsDocumentInvalid,
                    ));
                }
                Ok(())
            }
            Self::ConfigureSettings { changes } => validate_config_setting_changes(changes),
            Self::AcquireLease { instance_alias, .. }
            | Self::ObserveReadonly { instance_alias }
            | Self::SafeReset { instance_alias, .. }
            | Self::ApplicationLifecycle { instance_alias, .. }
            | Self::ControlEmulatorInstance { instance_alias, .. }
            | Self::ClearMonitor { instance_alias }
            | Self::ResourceTargetView { instance_alias }
            | Self::InstanceFactSnapshot {
                instance: instance_alias,
            } => validate_instance_alias(instance_alias),
            Self::ScheduleExplanation { instance, .. } => {
                instance.as_deref().map_or(Ok(()), validate_instance_alias)
            }
            Self::ConfigureMonitor {
                instance_alias,
                policy,
            } => {
                validate_instance_alias(instance_alias)?;
                policy.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeMonitorPolicy)
                })
            }
            Self::QueueLease {
                instance_alias,
                policy,
                ..
            } => {
                validate_instance_alias(instance_alias)?;
                policy.validate()
            }
            Self::PauseScheduling {
                scope,
                pause_reason,
                drain_timeout_ms,
            } => {
                scope.validate()?;
                validate_scheduling_pause_reason(pause_reason.as_str())?;
                if !(MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS..=MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS)
                    .contains(drain_timeout_ms)
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSchedulingPauseDrainTimeout,
                    ));
                }
                Ok(())
            }
            Self::ResumeScheduling { scope, expected } => {
                scope.validate()?;
                if let Some(expected) = expected {
                    expected.validate()?;
                }
                Ok(())
            }
            Self::SelfCheckInstance { instance_alias }
            | Self::DrillRecoveryLadder { instance_alias, .. } => {
                validate_instance_alias(instance_alias)
            }
            Self::RecognizeArtifact { request } => request.validate(),
            Self::RenewLease { token } | Self::ReleaseLease { token } => token.validate(),
            Self::CaptureSequence {
                instance_alias,
                spec,
            } => {
                validate_instance_alias(instance_alias)?;
                spec.validate()
            }
            Self::Input {
                token,
                action,
                frame,
            } => {
                if let Some(frame) = frame {
                    frame.validate()?;
                }
                token.validate()?;
                action.validate()
            }
            Self::RunContainedTask {
                instance_alias,
                request,
                ..
            } => {
                validate_instance_alias(instance_alias)?;
                request.validate()
            }
            Self::ObserveContainedPage {
                instance_alias,
                request,
            } => {
                validate_instance_alias(instance_alias)?;
                request.validate()
            }
            Self::RunContainedLabOperation {
                instance_alias,
                request,
                ..
            } => {
                validate_instance_alias(instance_alias)?;
                request.validate()
            }
            // Workflow #389 U-2: `via` exists only on the Lab channel (section 3.2 rule 7), a
            // shape check. No channel judgement here (section 3.1): one binding per connection
            // is the host's (`channel_binding_exists`).
            Self::BindChannel { channel, via } => {
                if (ChannelBinding {
                    channel: *channel,
                    via: *via,
                })
                .is_valid()
                {
                    Ok(())
                } else {
                    Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidBindChannel,
                    ))
                }
            }
            // Workflow #389 U-1: the package entries' checks come with U-3's `pack_registry_`
            // codes.
            Self::LoadPack { .. } | Self::UnloadPack { .. } => Ok(()),
            Self::UpgradeState { target, action } => {
                target.validate()?;
                action.validate()
            }
            Self::UpgradeStateStatus { transition_id } => {
                crate::installation::validate_transition_id(transition_id)
            }
        }
    }

    pub fn instance_alias(&self) -> Option<&str> {
        match self {
            Self::AcquireLease { instance_alias, .. }
            | Self::QueueLease { instance_alias, .. }
            | Self::ObserveReadonly { instance_alias }
            | Self::CaptureSequence { instance_alias, .. }
            | Self::ObserveContainedPage { instance_alias, .. }
            | Self::RunContainedLabOperation { instance_alias, .. }
            | Self::SafeReset { instance_alias, .. }
            | Self::ApplicationLifecycle { instance_alias, .. }
            | Self::ControlEmulatorInstance { instance_alias, .. }
            | Self::RunContainedTask { instance_alias, .. }
            | Self::ConfigureMonitor { instance_alias, .. }
            | Self::ClearMonitor { instance_alias } => Some(instance_alias),
            Self::RecordClientAction { action } => action.instance_alias(),
            _ => None,
        }
    }

    pub const fn lease_token(&self) -> Option<&LeaseToken> {
        match self {
            Self::RenewLease { token }
            | Self::ReleaseLease { token }
            | Self::Input { token, .. } => Some(token),
            _ => None,
        }
    }
}

impl fmt::Debug for RuntimeOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Health => "RuntimeOperation::Health",
            Self::RequestShutdown { .. } => "RuntimeOperation::RequestShutdown",
            Self::InstallTransition { .. } => "RuntimeOperation::InstallTransition",
            Self::Status => "RuntimeOperation::Status",
            Self::ProjectInterface { .. } => {
                "RuntimeOperation::ProjectInterface(<version-negotiation>)"
            }
            Self::ProjectPolicyInputIdentity { .. } => {
                "RuntimeOperation::ProjectPolicyInputIdentity(<ledger-position>)"
            }
            Self::MonitorStatus => "RuntimeOperation::MonitorStatus",
            Self::RuntimeFactSnapshot => "RuntimeOperation::RuntimeFactSnapshot",
            Self::InstanceFactSnapshot { .. } => {
                "RuntimeOperation::InstanceFactSnapshot(<redacted>)"
            }
            Self::PerformanceStatus => "RuntimeOperation::PerformanceStatus",
            Self::FactRefreshStatus => "RuntimeOperation::FactRefreshStatus",
            Self::ScheduleExplanation { .. } => "RuntimeOperation::ScheduleExplanation(<redacted>)",
            Self::ConfigureMonitor { .. } => "RuntimeOperation::ConfigureMonitor(<redacted>)",
            Self::ClearMonitor { .. } => "RuntimeOperation::ClearMonitor(<redacted>)",
            Self::AcquireLease { .. } => "RuntimeOperation::AcquireLease(<redacted>)",
            Self::QueueLease { .. } => "RuntimeOperation::QueueLease(<redacted>)",
            Self::PollQueuedLease { .. } => "RuntimeOperation::PollQueuedLease(<opaque-request>)",
            Self::CancelQueuedLease { .. } => {
                "RuntimeOperation::CancelQueuedLease(<opaque-request>)"
            }
            Self::CancelContainedTask { .. } => {
                "RuntimeOperation::CancelContainedTask(<opaque-request>)"
            }
            Self::RenewLease { .. } => "RuntimeOperation::RenewLease(<opaque-token>)",
            Self::ReleaseLease { .. } => "RuntimeOperation::ReleaseLease(<opaque-token>)",
            Self::ObserveReadonly { .. } => "RuntimeOperation::ObserveReadonly(<redacted>)",
            Self::RecognizeArtifact { .. } => "RuntimeOperation::RecognizeArtifact(<redacted>)",
            Self::ObserveContainedPage { .. } => {
                "RuntimeOperation::ObserveContainedPage(<contained-resource>)"
            }
            Self::RunContainedLabOperation { .. } => {
                "RuntimeOperation::RunContainedLabOperation(<contained-resource>)"
            }
            Self::CaptureSequence { .. } => "RuntimeOperation::CaptureSequence(<redacted>)",
            Self::SafeReset { .. } => "RuntimeOperation::SafeReset(<redacted>)",
            Self::ApplicationLifecycle { .. } => {
                "RuntimeOperation::ApplicationLifecycle(<redacted>)"
            }
            Self::ControlEmulatorInstance { .. } => {
                "RuntimeOperation::ControlEmulatorInstance(<redacted>)"
            }
            Self::DiscoverInstances => "RuntimeOperation::DiscoverInstances",
            Self::PauseScheduling { .. } => "RuntimeOperation::PauseScheduling(<redacted>)",
            Self::ResumeScheduling { .. } => "RuntimeOperation::ResumeScheduling(<redacted>)",
            Self::SelfCheckInstance { .. } => "RuntimeOperation::SelfCheckInstance(<redacted>)",
            Self::DrillRecoveryLadder { .. } => "RuntimeOperation::DrillRecoveryLadder(<redacted>)",
            Self::RunContainedTask { .. } => "RuntimeOperation::RunContainedTask(<redacted>)",
            Self::Input { .. } => "RuntimeOperation::Input(<redacted>)",
            Self::PublishFact { .. } => "RuntimeOperation::PublishFact(<typed-fact>)",
            Self::PublishFacts { .. } => "RuntimeOperation::PublishFacts(<typed-observation>)",
            Self::QueryEvents { .. } => "RuntimeOperation::QueryEvents(<typed-query>)",
            Self::ReadMaterial { .. } => "RuntimeOperation::ReadMaterial(<committed-reference>)",
            Self::SubscribeEvents { .. } => "RuntimeOperation::SubscribeEvents(<typed-query>)",
            Self::RegisterDiagnosticSignature { .. } => {
                "RuntimeOperation::RegisterDiagnosticSignature(<typed-definition>)"
            }
            Self::MatchDiagnosticSignatures { .. } => {
                "RuntimeOperation::MatchDiagnosticSignatures(<frozen-input>)"
            }
            Self::RetireDiagnosticSignature { .. } => {
                "RuntimeOperation::RetireDiagnosticSignature(<registration>)"
            }
            Self::DebugPackage { .. } => "RuntimeOperation::DebugPackage(<redacted>)",
            Self::ExportEvidence { .. } => "RuntimeOperation::ExportEvidence(<redacted>)",
            Self::RecordAuthoringEvent { .. } => {
                "RuntimeOperation::RecordAuthoringEvent(<redacted>)"
            }
            Self::RecordDebugEvent { .. } => {
                "RuntimeOperation::RecordDebugEvent(<typed-debug-event>)"
            }
            Self::ReleaseLabPin { .. } => "RuntimeOperation::ReleaseLabPin(<typed-target>)",
            Self::RecordClientAction { .. } => {
                "RuntimeOperation::RecordClientAction(<typed-redacted-action>)"
            }
            Self::DeclareGovernanceIdentity { .. } => {
                "RuntimeOperation::DeclareGovernanceIdentity(<identity-card>)"
            }
            Self::RecordApprovalDecision { .. } => {
                "RuntimeOperation::RecordApprovalDecision(<typed-approval>)"
            }
            Self::StartAgentSession { .. } => "RuntimeOperation::StartAgentSession(<opaque-wake>)",
            Self::ResumeAgentSession { .. } => {
                "RuntimeOperation::ResumeAgentSession(<opaque-session>)"
            }
            Self::AgentSessionStatus { .. } => {
                "RuntimeOperation::AgentSessionStatus(<opaque-session>)"
            }
            Self::RecordAgentResponse { .. } => {
                "RuntimeOperation::RecordAgentResponse(<typed-response>)"
            }
            Self::PrepareStrategicReport { .. } => {
                "RuntimeOperation::PrepareStrategicReport(<typed-planning-document>)"
            }
            Self::ProjectPolicyForward { .. } => {
                "RuntimeOperation::ProjectPolicyForward(<typed-planning-documents>)"
            }
            Self::AssessPredictiveMaintenance { .. } => {
                "RuntimeOperation::AssessPredictiveMaintenance(<typed-ledger-query>)"
            }
            Self::CompileProposal { .. } => "RuntimeOperation::CompileProposal(<typed-proposal>)",
            Self::PromoteProposal { .. } => "RuntimeOperation::PromoteProposal(<typed-proposal>)",
            Self::ApplyResourceTargets { .. } => {
                "RuntimeOperation::ApplyResourceTargets(<document>)"
            }
            Self::ResourceTargetView { .. } => "RuntimeOperation::ResourceTargetView(<redacted>)",
            Self::ConfigureSettings { .. } => "RuntimeOperation::ConfigureSettings(<redacted>)",
            Self::BindChannel { .. } => "RuntimeOperation::BindChannel(<channel>)",
            Self::UpgradeState { .. } => "RuntimeOperation::UpgradeState(<transition>)",
            Self::UpgradeStateStatus { .. } => "RuntimeOperation::UpgradeStateStatus(<transition>)",
            Self::LoadPack { .. } => "RuntimeOperation::LoadPack(<redacted>)",
            Self::UnloadPack { .. } => "RuntimeOperation::UnloadPack(<redacted>)",
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRequest {
    schema_version: String,
    request_id: RequestId,
    correlation_id: CorrelationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    causation_id: Option<CausationId>,
    actor: EventActor,
    source: EventSource,
    submitted_at_unix_ms: u64,
    operation: RuntimeOperation,
    /// Workflow #378 A4 (request v4, model section 7.1): how much of the receipt's outcome the
    /// client wants, `codes` or `full`; absent, the class default of its source applies
    /// ([`Self::receipt_detail`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<Detail>,
}

impl RuntimeRequest {
    pub fn new(
        request_id: IssuedRequestId,
        correlation_id: IssuedCorrelationId,
        causation_id: Option<IssuedCausationId>,
        actor: EventActor,
        source: EventSource,
        submitted_at_unix_ms: u64,
        operation: RuntimeOperation,
    ) -> RuntimeContractResult<Self> {
        let request = Self {
            schema_version: RUNTIME_REQUEST_SCHEMA_VERSION.to_string(),
            request_id: *request_id.transport(),
            correlation_id: *correlation_id.transport(),
            causation_id: causation_id.map(|value| *value.transport()),
            actor,
            source,
            submitted_at_unix_ms,
            operation,
            detail: None,
        };
        request.validate()?;
        Ok(request)
    }

    /// This request asking for the given receipt detail (Workflow #378 A4): it overrides the
    /// class default of the request's source.
    #[must_use]
    pub const fn with_detail(mut self, detail: Detail) -> Self {
        self.detail = Some(detail);
        self
    }

    /// The receipt detail this request asks for, if it names one.
    pub const fn detail(&self) -> Option<Detail> {
        self.detail
    }

    /// The detail its receipt is written with (model section 7.1): the request's own, else the
    /// class default of its source, codes only for the console (`source: ui`) and the full
    /// outcome for every other client.
    pub fn receipt_detail(&self) -> Detail {
        let class_default = if self.source == EventSource::Ui {
            Detail::Codes
        } else {
            Detail::Full
        };
        self.detail.unwrap_or(class_default)
    }

    pub fn validate(&self) -> RuntimeContractResult<ValidatedRuntimeRequest<'_>> {
        if self.schema_version != RUNTIME_REQUEST_SCHEMA_VERSION {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::UnsupportedRequestSchema,
            ));
        }
        if self.submitted_at_unix_ms == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRequestTimestamp,
            ));
        }
        if !valid_client_origin(self.actor, self.source) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidClientOrigin,
            ));
        }
        // Shutdown is only ever requested, never forced: the person at the console (Ui) or the
        // operator (Cli) may ask; the host still decides and stops at its own pace.
        if matches!(self.operation, RuntimeOperation::RequestShutdown { .. })
            && !matches!(
                (self.actor, self.source),
                (EventActor::User, EventSource::Ui) | (EventActor::Cli, EventSource::Cli)
            )
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidShutdownOrigin,
            ));
        }
        if matches!(self.operation, RuntimeOperation::InstallTransition { .. })
            && !matches!(
                (self.actor, self.source),
                (EventActor::User, EventSource::Ui) | (EventActor::Cli, EventSource::Cli)
            )
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidInstallTransitionOrigin,
            ));
        }
        if matches!(
            self.operation,
            RuntimeOperation::RecordAuthoringEvent { .. }
        ) && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidResourceAuthoringOrigin,
            ));
        }
        if matches!(
            self.operation,
            RuntimeOperation::RegisterDiagnosticSignature { .. }
                | RuntimeOperation::MatchDiagnosticSignatures { .. }
                | RuntimeOperation::RetireDiagnosticSignature { .. }
        ) && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidSignatureOrigin,
            ));
        }
        if matches!(
            self.operation,
            RuntimeOperation::RecordDebugEvent { .. }
                | RuntimeOperation::ReleaseLabPin { .. }
                | RuntimeOperation::RunContainedLabOperation { .. }
                | RuntimeOperation::RecognizeArtifact { .. }
        ) && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeDebugOrigin,
            ));
        }
        if matches!(
            self.operation,
            RuntimeOperation::DebugPackage { .. } | RuntimeOperation::ExportEvidence { .. }
        ) && (self.actor != EventActor::Lab || self.source != EventSource::Lab)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeDebugOrigin,
            ));
        }
        // A governance identity card may be declared by the person (Ui) or the operator (Cli);
        // approval decisions stay person-only.
        if matches!(
            self.operation,
            RuntimeOperation::DeclareGovernanceIdentity { .. }
        ) && !valid_governance_declaration_origin(self.actor, self.source)
        {
            return Err(RuntimeContractError::new(
                SharedCode::InvalidGovernanceOrigin,
            ));
        }
        if matches!(
            self.operation,
            RuntimeOperation::RecordApprovalDecision { .. }
        ) && (self.actor != EventActor::User || self.source != EventSource::Ui)
        {
            return Err(RuntimeContractError::new(
                SharedCode::InvalidGovernanceOrigin,
            ));
        }
        // Only an explicit person (Ui) or operator (Cli) request may drive the emulator, spawn
        // its discovery tool or reconnect it for a self-check; Adapter/Agent origins are
        // excluded so no scheduler or agent path can restart or reconnect it. Workflow #390
        // DRILL-1: a ladder drill may restart the emulator, so the same gate holds it.
        if matches!(
            self.operation,
            RuntimeOperation::ControlEmulatorInstance { .. }
                | RuntimeOperation::DiscoverInstances
                | RuntimeOperation::SelfCheckInstance { .. }
                | RuntimeOperation::DrillRecoveryLadder { .. }
        ) && !matches!(
            (self.actor, self.source),
            (EventActor::User, EventSource::Ui) | (EventActor::Cli, EventSource::Cli)
        ) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidEmulatorControlOrigin,
            ));
        }
        // Only the person (Ui) or the operator (Cli) may pause or resume scheduling; no
        // scheduler, agent or Lab path can stop or restart dispatch by itself.
        if matches!(
            self.operation,
            RuntimeOperation::PauseScheduling { .. } | RuntimeOperation::ResumeScheduling { .. }
        ) && !matches!(
            (self.actor, self.source),
            (EventActor::User, EventSource::Ui) | (EventActor::Cli, EventSource::Cli)
        ) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidSchedulingPauseOrigin,
            ));
        }
        // Fact publication (Workflow #308 slice 4a-2; Workflow #411 B15 R6): an observation
        // whose every key belongs to a family with the origin exception (manual priority
        // offsets, remedy choices) may also come from the person (Ui) or the operator (Cli);
        // every other key stays Agent/Adapter only, and no observation mixes two families.
        let fact_keys = match &self.operation {
            RuntimeOperation::PublishFact { record } => Some(vec![record.key.as_str()]),
            RuntimeOperation::PublishFacts { observation } => Some(
                observation
                    .records
                    .iter()
                    .map(|record| record.key.as_str())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        };
        if let Some(keys) = fact_keys {
            let families = keys
                .iter()
                .map(|key| crate::fact_key_family(key))
                .collect::<std::collections::BTreeSet<_>>();
            if families.len() > 1 {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::FactOriginMixed,
                ));
            }
            let agent = (self.actor, self.source) == (EventActor::Agent, EventSource::Adapter);
            let person = matches!(
                (self.actor, self.source),
                (EventActor::User, EventSource::Ui) | (EventActor::Cli, EventSource::Cli)
            );
            let excepted = families
                .first()
                .is_some_and(|family| family.takes_origin_exception());
            if !(agent || (excepted && person)) {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidAgentDispatcherOrigin,
                ));
            }
        }
        if matches!(
            self.operation,
            RuntimeOperation::StartAgentSession { .. }
                | RuntimeOperation::ResumeAgentSession { .. }
                | RuntimeOperation::AgentSessionStatus { .. }
                | RuntimeOperation::RecordAgentResponse { .. }
                | RuntimeOperation::ProjectPolicyInputIdentity { .. }
                | RuntimeOperation::PrepareStrategicReport { .. }
                | RuntimeOperation::ProjectPolicyForward { .. }
                | RuntimeOperation::AssessPredictiveMaintenance { .. }
                | RuntimeOperation::CompileProposal { .. }
                | RuntimeOperation::PromoteProposal { .. }
                | RuntimeOperation::ApplyResourceTargets { .. }
        ) && (self.actor != EventActor::Agent || self.source != EventSource::Adapter)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidAgentDispatcherOrigin,
            ));
        }
        self.operation.validate()?;
        Ok(ValidatedRuntimeRequest {
            request: self,
            channel: None,
        })
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn actor(&self) -> EventActor {
        self.actor
    }

    pub const fn source(&self) -> EventSource {
        self.source
    }

    pub const fn submitted_at_unix_ms(&self) -> u64 {
        self.submitted_at_unix_ms
    }

    pub const fn operation(&self) -> &RuntimeOperation {
        &self.operation
    }
}

impl fmt::Debug for RuntimeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeRequest")
            .field("schema_version", &self.schema_version)
            .field("request_id", &self.request_id)
            .field("correlation_id", &self.correlation_id)
            .field("actor", &self.actor)
            .field("source", &self.source)
            .field("submitted_at_unix_ms", &self.submitted_at_unix_ms)
            .field("operation", &self.operation)
            .field("detail", &self.detail)
            .finish()
    }
}

#[derive(Debug)]
pub struct ValidatedRuntimeRequest<'a> {
    request: &'a RuntimeRequest,
    /// Workflow #389 U-2: the channel the door tagged the request with (section 3.1 duty 3);
    /// `None` for a request that never passed the door (the host's own requests) and for the
    /// handshake of a connection not bound yet.
    channel: Option<ChannelBinding>,
}

impl ValidatedRuntimeRequest<'_> {
    pub const fn request_id(&self) -> RequestId {
        self.request.request_id
    }

    /// The request tagged with the channel of the connection it came on (Workflow #389 U-2).
    #[must_use]
    pub fn with_channel(mut self, channel: Option<ChannelBinding>) -> Self {
        self.channel = channel;
        self
    }

    /// The channel the door tagged the request with (Workflow #389 U-2).
    pub const fn channel(&self) -> Option<ChannelBinding> {
        self.channel
    }

    pub fn event_links(
        &self,
        instance_id: Option<InstanceId>,
        lease_id: Option<LeaseId>,
        action_id: Option<ActionId>,
    ) -> EventLinksDraft {
        EventLinksDraft::from_verified_runtime(
            instance_id,
            self.request.request_id,
            self.request.correlation_id,
            self.request.causation_id,
            lease_id,
            action_id,
        )
    }

    pub fn task_event_links(&self, task_id: IssuedTaskId, run_id: IssuedRunId) -> EventLinksDraft {
        self.event_links(None, None, None)
            .with_task_id(task_id)
            .with_run_id(run_id)
    }

    /// Rebinds task/run identities already validated from this request's durable contained-task
    /// chain. This is only for recovery of an interrupted request replay with the same request and
    /// correlation identities.
    pub fn contained_task_recovery_event_links(
        &self,
        instance_id: InstanceId,
        lease_id: Option<LeaseId>,
        task_id: crate::TaskId,
        run_id: RunId,
        action_id: Option<ActionId>,
    ) -> EventLinksDraft {
        EventLinksDraft::from_verified_runtime(
            Some(instance_id),
            self.request.request_id,
            self.request.correlation_id,
            self.request.causation_id,
            lease_id,
            action_id,
        )
        .with_task_id(IssuedTaskId::from_verified_transport(task_id))
        .with_run_id(IssuedRunId::from_verified_transport(run_id))
    }

    pub fn task_artifact_links(&self, run_id: IssuedRunId) -> ArtifactLinksDraft {
        self.artifact_links().with_run_id(run_id)
    }

    pub fn artifact_links(&self) -> ArtifactLinksDraft {
        ArtifactLinksDraft::default().with_correlation_id(
            IssuedCorrelationId::from_verified_transport(self.request.correlation_id),
        )
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.request.correlation_id
    }

    pub const fn actor(&self) -> EventActor {
        self.request.actor
    }

    pub const fn source(&self) -> EventSource {
        self.request.source
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeReceiptState {
    Admitted,
    Observed,
    Queued,
    Denied,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalEvent {
    pub sequence: u64,
    pub event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainedTaskCancellationReason {
    DeadlineExceeded,
    ClientRequested,
    RecoveredAfterRestart,
    /// An instance scheduling pause asked the run to stop after its drain timeout
    /// (`contained_task_paused`, Workflow #191 ps1).
    PausedByOperator,
    /// Workflow #386 (register S2, runtime-client revision 2): a run the Runtime started itself
    /// was still active when a blackout started (`blackout_run_stopped`). SR-4b writes it.
    BlackoutStop,
    /// Workflow #385 (register P7, runtime-client revision 2): performance pacing ended a policy
    /// run to be rerun (`perf_run_yielded`, `perf_hold_expired`). P-k and P-h write it.
    PerformanceYield,
}

impl ContainedTaskCancellationReason {
    /// The reason of a cancelled terminal from the code of its committed outcome (Workflow #338
    /// R1, ledger revision 3): the three cancellation codes map to their reason, the runtime
    /// host's blackout stop to `BlackoutStop` and its two pacing endings to `PerformanceYield`
    /// (Workflow #383 SH-1, register S2, P7), any other code or none to `RecoveredAfterRestart`.
    pub fn from_failure_code(failure_code: Option<&crate::outcome::CodeStr>) -> Self {
        match failure_code {
            Some(code) if *code == RuntimeErrorCode::ContainedTaskDeadlineExceeded.code() => {
                Self::DeadlineExceeded
            }
            Some(code) if *code == RuntimeErrorCode::ContainedTaskCancelled.code() => {
                Self::ClientRequested
            }
            Some(code) if *code == RuntimeErrorCode::ContainedTaskPaused.code() => {
                Self::PausedByOperator
            }
            // The runtime host's codes (its registry `SchedulingRuleHostCode`,
            // `ShapeHostCode`): the contract names them by spelling.
            Some(code) if code.as_str() == "blackout_run_stopped" => Self::BlackoutStop,
            Some(code) if matches!(code.as_str(), "perf_run_yielded" | "perf_hold_expired") => {
                Self::PerformanceYield
            }
            _ => Self::RecoveredAfterRestart,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainedTaskLeaseTerminal {
    Released,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContainedTaskCancellationStatus {
    Pending {
        deadline_monotonic_ms: u64,
    },
    RecoveryRequired {
        #[serde(skip_serializing_if = "Option::is_none")]
        deadline_monotonic_ms: Option<u64>,
    },
    Terminal {
        #[serde(skip_serializing_if = "Option::is_none")]
        deadline_monotonic_ms: Option<u64>,
        /// How the cancelled run ended (Workflow #378 A4: receipt v2 reserves `outcome` for the
        /// envelope, model section 8.1).
        task_result: TaskOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<ContainedTaskCancellationReason>,
        task_terminal: TerminalEvent,
        lease_terminal: TerminalEvent,
        lease_disposition: ContainedTaskLeaseTerminal,
    },
}

impl ContainedTaskCancellationStatus {
    fn validate(&self) -> RuntimeContractResult<()> {
        match self {
            Self::Pending {
                deadline_monotonic_ms,
            } if *deadline_monotonic_ms == 0 => Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidContainedTaskCancellationStatus,
            )),
            Self::RecoveryRequired {
                deadline_monotonic_ms: Some(0),
            } => Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidContainedTaskCancellationStatus,
            )),
            Self::Terminal {
                deadline_monotonic_ms,
                task_result,
                reason,
                task_terminal,
                lease_terminal,
                ..
            } if deadline_monotonic_ms.is_some_and(|deadline| deadline == 0)
                || task_terminal.sequence == 0
                || lease_terminal.sequence == 0
                || task_terminal == lease_terminal
                || (*task_result == TaskOutcome::Cancelled) != reason.is_some() =>
            {
                Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidContainedTaskCancellationStatus,
                ))
            }
            _ => Ok(()),
        }
    }
}

/// The Runtime's wire error codes are a registry of their own (Workflow #378 A2a,
/// `crate::codes`); this path keeps naming them.
pub use crate::codes::RuntimeErrorCode;

/// The Runtime's typed view of a refusal or failure: its closed error code, whether the host is
/// halted (`fatal`), the current holder of a busy instance and when to retry. Receipt v2
/// (Workflow #378 A4) carries it inside its `outcome` ([`Self::receipt_outcome`],
/// [`Self::from_receipt_outcome`]); the lab operation and material read failure records keep it
/// as their own member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeErrorProjection {
    pub code: RuntimeErrorCode,
    pub fatal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_holder_id: Option<HolderId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_lease_id: Option<LeaseId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// The outcome keys of a receipt's projection values (Workflow #378 model section 7.2).
const RUNTIME_CODE_KEY: &str = "runtime_code";
const HOST_HALTED_KEY: &str = "host_halted";
const HOLDER_ID_KEY: &str = "holder_id";
const LEASE_ID_KEY: &str = "lease_id";
const RETRY_AFTER_MS_KEY: &str = "retry_after_ms";

impl RuntimeErrorProjection {
    pub const fn new(code: RuntimeErrorCode, fatal: bool) -> Self {
        Self {
            code,
            fatal,
            current_holder_id: None,
            current_lease_id: None,
            retry_after_ms: None,
        }
    }

    pub const fn with_holder(mut self, holder_id: HolderId, lease_id: LeaseId) -> Self {
        self.current_holder_id = Some(holder_id);
        self.current_lease_id = Some(lease_id);
        self
    }

    pub const fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    /// This projection as the outcome of a denied or failed receipt (Workflow #378 A4, model
    /// section 7.2). The Runtime's specific code `top` is the top code, and the projection's
    /// Runtime error code its `runtime_code` value; without a specific code (or when it is the
    /// Runtime error code itself) the Runtime error code is the top. `operation` says where the
    /// request failed, `fatal` travels as `host_halted`, the current holder as `holder_id` and
    /// `lease_id`, and `retry_after_ms` stays.
    pub fn receipt_outcome(&self, top: Option<Code>, operation: Option<Location>) -> Outcome {
        let runtime_code = self.code.code();
        let mut outcome = match top {
            Some(top) if top != runtime_code => Outcome::new(top).runtime_code(runtime_code),
            _ => Outcome::new(runtime_code),
        };
        if let Some(operation) = operation {
            outcome = outcome.operation(operation);
        }
        self.receipt_state_values(outcome)
    }

    /// The receipt values of this projection on `outcome`, which its producer built with the
    /// values its code declares (Workflow #378 A4): a code whose catalog entry requires values
    /// tops a denied or failed receipt only with them. The projection's Runtime error code is
    /// the `runtime_code` value unless it is the outcome's own code; `host_halted`, the holder
    /// and `retry_after_ms` follow as in [`Self::receipt_outcome`].
    pub fn with_receipt_values(&self, outcome: Outcome) -> Outcome {
        let runtime_code = self.code.code();
        let outcome = if outcome.code().as_str() == runtime_code.as_str() {
            outcome
        } else {
            outcome.runtime_code(runtime_code)
        };
        self.receipt_state_values(outcome)
    }

    /// `fatal` as `host_halted`, the current holder as `holder_id` and `lease_id`, and
    /// `retry_after_ms`.
    fn receipt_state_values(&self, outcome: Outcome) -> Outcome {
        let mut outcome = outcome.host_halted(self.fatal);
        if let Some(holder_id) = self.current_holder_id.as_ref().and_then(id_text) {
            outcome = outcome.holder_id(holder_id);
        }
        if let Some(lease_id) = self.current_lease_id.as_ref().and_then(id_text) {
            outcome = outcome.lease_id(lease_id);
        }
        if let Some(retry_after_ms) = self.retry_after_ms {
            outcome = outcome.retry_after_ms(retry_after_ms);
        }
        outcome
    }

    /// The projection a denied or failed receipt's outcome carries (Workflow #378 A4): its
    /// Runtime error code is the `runtime_code` value, or the top code when that is a Runtime
    /// error code itself; `None` when neither names one. `fatal` is `host_halted` as sent, which
    /// every denied or failed receipt carries in every detail mode (coordinator ruling A-M1). A
    /// receipt written in codes mode withholds the other data values, so the holder and
    /// `retry_after_ms` read as absent.
    pub fn from_receipt_outcome(outcome: &Outcome) -> Option<Self> {
        let code = match outcome.value(RUNTIME_CODE_KEY) {
            Some(value) => runtime_error_code(value.as_str()?)?,
            None => runtime_error_code(outcome.code().as_str())?,
        };
        let fatal = outcome
            .value(HOST_HALTED_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut projection = Self::new(code, fatal);
        let holder_id = outcome
            .value(HOLDER_ID_KEY)
            .and_then(|value| HolderId::deserialize(value).ok());
        let lease_id = outcome
            .value(LEASE_ID_KEY)
            .and_then(|value| LeaseId::deserialize(value).ok());
        if let (Some(holder_id), Some(lease_id)) = (holder_id, lease_id) {
            projection = projection.with_holder(holder_id, lease_id);
        }
        if let Some(retry_after_ms) = outcome
            .value(RETRY_AFTER_MS_KEY)
            .and_then(serde_json::Value::as_u64)
        {
            projection = projection.with_retry_after(retry_after_ms);
        }
        Some(projection)
    }
}

/// A receipt's outcome envelope for `detail`. Codes mode withholds the data values (model section
/// 4.3) except a denied or failed receipt's `host_halted`, which travels in every detail mode
/// (Workflow #378 A4, coordinator ruling A-M1): the client's own fatal handling runs on it.
fn receipt_envelope(outcome: &Outcome, detail: Detail) -> OutcomeEnvelope {
    OutcomeEnvelope::keeping_top_values(outcome, detail, &[HOST_HALTED_KEY])
}

/// The registered Runtime error code a received spelling names, if any.
fn runtime_error_code(spelling: &str) -> Option<RuntimeErrorCode> {
    RuntimeErrorCode::ALL
        .iter()
        .copied()
        .find(|code| code.as_str() == spelling)
}

/// The canonical text of a typed identifier, as it travels on the wire.
fn id_text<T: Serialize>(id: &T) -> Option<String> {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => Some(text),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyCaptureCapability {
    owner_epoch: OwnerEpoch,
    instance_id: InstanceId,
    frame_id: FrameId,
    recognition_id: RecognitionId,
}

impl ReadOnlyCaptureCapability {
    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub const fn recognition_id(&self) -> RecognitionId {
        self.recognition_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssuedReadOnlyCaptureCapability {
    transport: ReadOnlyCaptureCapability,
    frame_id: IssuedFrameId,
    recognition_id: IssuedRecognitionId,
}

impl IssuedReadOnlyCaptureCapability {
    pub const fn transport(&self) -> &ReadOnlyCaptureCapability {
        &self.transport
    }

    pub fn event_links(&self, request: &ValidatedRuntimeRequest<'_>) -> EventLinksDraft {
        request
            .event_links(Some(self.transport.instance_id), None, None)
            .with_frame_id(self.frame_id)
            .with_recognition_id(self.recognition_id)
    }

    pub fn artifact_links(
        &self,
        request: &ValidatedRuntimeRequest<'_>,
    ) -> crate::ArtifactLinksDraft {
        crate::ArtifactLinksDraft::default()
            .with_frame_id(self.frame_id)
            .with_correlation_id(IssuedCorrelationId::from_verified_transport(
                request.request.correlation_id,
            ))
    }
}

impl IdentifierIssuer {
    pub fn issue_readonly_capture_capability(
        &self,
        owner_epoch: OwnerEpoch,
        instance_id: InstanceId,
    ) -> Result<IssuedReadOnlyCaptureCapability, IdentifierIssuanceError> {
        let frame_id = self.mint_frame_id()?;
        let recognition_id = self.mint_recognition_id()?;
        Ok(IssuedReadOnlyCaptureCapability {
            transport: ReadOnlyCaptureCapability {
                owner_epoch,
                instance_id,
                frame_id: *frame_id.transport(),
                recognition_id: *recognition_id.transport(),
            },
            frame_id,
            recognition_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeResult {
    InstallTransition {
        status: crate::InstallTransitionStatus,
    },
    ShutdownAccepted {
        target: RuntimeShutdownTarget,
    },
    Health {
        owner_epoch: OwnerEpoch,
    },
    Status {
        status: RuntimeControlPlaneStatus,
    },
    ProjectInterface {
        response: Box<ProjectInterfaceResponse>,
    },
    PolicyInputIdentityProjected {
        identity: RuntimePolicyInputIdentity,
    },
    MonitorStatus {
        status: RuntimeMonitorRegistryStatus,
    },
    RuntimeFactSnapshot {
        snapshot: crate::RuntimeFactSnapshot,
    },
    /// `InstanceFactSnapshot` (Workflow #389 MCP-4, register W2): the instance's facts and the
    /// read's state source (register W1), never a ledger position of the snapshot.
    InstanceFactSnapshot {
        snapshot: Box<crate::InstanceFactView>,
        source: crate::RuntimeStateSource,
    },
    /// `PerformanceStatus` (Workflow #385 P-m, register §1.61 W15): the shadow measurement's
    /// host row and one row per registered instance, from memory.
    PerformanceStatus {
        status: Box<RuntimePerformanceStatus>,
    },
    /// `FactRefreshStatus` (Workflow #384 D2-S5, register §1.68): the keys each instance's
    /// candidates wait for, from the transition recorder's memory.
    FactRefreshStatus {
        status: Box<RuntimeFactRefreshStatus>,
    },
    /// `ScheduleExplanation` (Workflow #386 SR-6b, MCP-19a; register §1.143, §3 W5): the latest
    /// evaluation's not-running rows and the candidates that did not run, from the policy host's
    /// memory.
    ScheduleExplanation {
        explanation: Box<RuntimeScheduleExplanation>,
    },
    MonitorConfigured {
        status: RuntimeMonitorInstanceStatus,
    },
    MonitorCleared {
        status: RuntimeMonitorInstanceStatus,
    },
    LeaseGranted {
        token: LeaseToken,
    },
    LeaseRenewed {
        token: LeaseToken,
    },
    LeaseReleased {
        instance_id: InstanceId,
        lease_id: LeaseId,
    },
    LeaseQueued {
        status: LeaseQueueStatus,
    },
    LeasePending {
        status: LeaseQueueStatus,
    },
    LeaseQueueCancelled {
        request_id: RequestId,
        instance_id: InstanceId,
    },
    ReadonlyObservationCompleted {
        observation: ReadonlyObservation,
    },
    ArtifactRecognized {
        result: Box<SavedArtifactOcrResult>,
    },
    ContainedPageObserved {
        observation: Box<ContainedPageObservation>,
    },
    ContainedLabOperation {
        operation: Box<ContainedLabOperationResult>,
    },
    CaptureSequenceCompleted {
        sequence: CaptureSequence,
    },
    SafeResetCompleted {
        action_id: ActionId,
    },
    ApplicationLifecycleCompleted {
        action_id: ActionId,
        action: ApplicationLifecycleAction,
    },
    /// The provider `control` dispatch completed and the readiness wait met its criterion.
    EmulatorInstanceControlled {
        instance_alias: String,
        action: EmulatorInstanceAction,
        instance_index: u16,
        running: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        adb_port: Option<u16>,
        elapsed_ms: u64,
        /// Whether the instance's configured startup package was handed to the host's own
        /// scheduling point after this action (slice #316-B3). The package runs later as a
        /// contained task with its own `task.*` events; nothing runs inside this request.
        #[serde(default)]
        startup_package: StartupPackageDisposition,
    },
    /// The provider's on-demand instance discovery answer (`DiscoverInstances`).
    InstancesDiscovered {
        discovery: RuntimeInstanceDiscovery,
    },
    /// The scheduling pause is in effect (`PauseScheduling`); an instance pause answers once
    /// its in-flight runs drained and its device session was closed, and carries what the runs
    /// did.
    SchedulingPaused {
        scope: SchedulingPauseScope,
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drained: Option<SchedulingDrainSummary>,
    },
    /// The scheduling pause is lifted (`ResumeScheduling`); an instance resume reconnects the
    /// device at once and carries its self-check.
    SchedulingResumed {
        scope: SchedulingPauseScope,
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selfcheck: Option<SchedulingResumeSelfCheck>,
    },
    /// The preparation phase of `SelfCheckInstance` ran (Workflow #317 sc3); `selfcheck` has
    /// the shape of an instance resume's and is projected from the opens it made.
    InstanceSelfChecked {
        instance_alias: String,
        selfcheck: SchedulingResumeSelfCheck,
    },
    /// `DrillRecoveryLadder` (Workflow #390 DRILL-1): the drill was admitted and its claim
    /// waits for the instance's key; its ladder records follow under `drill.ladder_request_id`.
    RecoveryLadderDrillQueued {
        drill: crate::RecoveryLadderDrill,
    },
    ContainedTaskCompleted {
        run_id: RunId,
        task_id: crate::TaskId,
        task_request_id: RequestId,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_deadline_monotonic_ms: Option<u64>,
        /// How the run ended (Workflow #378 A4: receipt v2 reserves `outcome` for the envelope,
        /// model section 8.1).
        task_result: TaskOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        final_page: Option<String>,
        executed_steps: u32,
    },
    ContainedTaskCancelled {
        run_id: RunId,
        task_id: crate::TaskId,
        task_request_id: RequestId,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_deadline_monotonic_ms: Option<u64>,
        reason: ContainedTaskCancellationReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        lease_terminal: Option<ContainedTaskLeaseTerminal>,
    },
    ContainedTaskCancellation {
        task_request_id: RequestId,
        status: ContainedTaskCancellationStatus,
    },
    InputCommitted {
        action_id: ActionId,
    },
    FactPublished {
        event_id: EventId,
    },
    EventPage {
        page: RuntimeEventQueryPage,
    },
    MaterialRead {
        result: Box<RuntimeMaterialReadResult>,
    },
    EventBatch {
        batch: RuntimeEventBatch,
    },
    SignatureRegistered {
        registration: crate::SignatureRegistrationRef,
    },
    SignaturesMatched {
        page: Box<crate::SignatureReplayPage>,
    },
    SignatureRetired {
        registration: crate::SignatureRegistrationRef,
    },
    PackageDebugCompleted {
        summary: PackageDebugSummary,
    },
    EvidenceExportCompleted {
        summary: Box<RuntimeEvidenceExportSummary>,
    },
    AuthoringEventRecorded {
        phase: ResourceAuthoringPhase,
    },
    DebugEventRecorded {
        phase: RuntimeDebugPhase,
    },
    LabPinReleased {
        release: Box<crate::LabPinReleaseResult>,
    },
    ClientActionRecorded,
    GovernanceIdentityAccepted,
    ApprovalDecisionRecorded {
        approval_id: String,
        disposition: ApprovalDisposition,
    },
    AgentSessionOpened {
        context: Box<AgentSessionContext>,
    },
    AgentSessionObserved {
        context: Box<AgentSessionContext>,
    },
    AgentResponseRecorded {
        status: AgentSessionStatus,
    },
    StrategicPlanPrepared {
        plan: Box<RuntimeStrategicPlanResult>,
    },
    PolicyForwardProjected {
        projection: Box<RuntimePlanningDocument>,
    },
    PredictiveMaintenanceAssessed {
        assessment: Box<RuntimePlanningDocument>,
    },
    ProposalEvaluated {
        preview: ProposalPreview,
    },
    ProposalPromoted {
        promotion: ProposalPromotion,
    },
    /// `ApplyResourceTargets` stored (or replayed) the instance's policy; the receipt's
    /// terminal is the `fact.published` event that holds it.
    ResourceTargetsApplied {
        applied: Box<crate::ResourceTargetsApplied>,
    },
    /// The read-only view of one instance's resource targets (`ResourceTargetView`).
    ResourceTargetView {
        view: Box<crate::ResourceTargetView>,
    },
    /// `ConfigureSettings` changed every key it named (Workflow #387 D1b): each key as it is in
    /// effect afterwards, in request order.
    SettingsConfigured {
        settings: Vec<ConfiguredSetting>,
    },
    // Workflow #389 U-1: the results of the new request kinds; the slices that add their
    // behaviour answer with them.
    /// `BindChannel`: the connection's channel, as bound (U-2).
    ChannelBound {
        channel: RuntimeChannel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<ChannelVia>,
    },
    /// `UpgradeState`: the transition after the step, its ticket included (U-4). An accepted
    /// `enter` answers drained, having accepted its own shutdown; `cancel` answers aborted;
    /// `leave` answers released.
    UpgradeState {
        status: crate::InstallTransitionStatus,
    },
    /// `UpgradeStateStatus`: the transition's current phase (U-4).
    UpgradeStateStatus {
        status: crate::InstallTransitionStatus,
    },
    /// `LoadPack`: the entry now in the table (U-3). For a procedure binding,
    /// `named_by_catalog` says whether a task of the catalog names it: a binding no task names
    /// loads, but nothing uses it.
    PackLoaded {
        key: PackRegistryKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        named_by_catalog: Option<bool>,
    },
    /// `UnloadPack`: the entry left the table (U-3); the runs it stopped, and how many
    /// admitted dispatches of it were withdrawn.
    PackUnloaded {
        key: PackRegistryKey,
        stopped_runs: Vec<PackUnloadStoppedRun>,
        withdrawn_dispatches: u32,
    },
}

/// A Runtime's answer to one request (receipt v2, Workflow #378 A4). Every receipt carries
/// `outcome`: the info code of its state when admitted, observed, queued or cancelled; the
/// success code of its request kind when completed ([`RuntimeOperationKind::success_code`]); its
/// refusal or failure when denied or failed, with the Runtime error code as the `runtime_code`
/// value ([`RuntimeErrorProjection::receipt_outcome`]). The Runtime writes it with the detail the
/// request asks for ([`RuntimeRequest::receipt_detail`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReceipt {
    schema_version: String,
    request_id: RequestId,
    correlation_id: CorrelationId,
    state: RuntimeReceiptState,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<TerminalEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<RuntimeResult>,
    outcome: OutcomeEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_declaration: Option<Box<crate::ResourceDeclarationRejection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_declaration_event: Option<TerminalEvent>,
    /// The field-positioned rejection of an `ApplyResourceTargets` document (Denied, no
    /// terminal, nothing recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_targets_rejection: Option<Box<crate::ResourceTargetsRejection>>,
    /// Workflow #389 U-2 (model section 5.1): the channel of the connection the request came
    /// on, as the door tagged it; absent on the handshake of a connection not bound yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel: Option<RuntimeChannel>,
    /// Workflow #389 U-2: the counter that handed the request over to the Lab channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<ChannelVia>,
}

/// The outcome of a receipt whose request did not fail (Workflow #378 A4): the info code of its
/// state, or the success code of the request's kind when completed. A denied or failed state, or
/// a completed kind without a success code, has none.
fn receipt_state_outcome(
    request: &RuntimeRequest,
    state: RuntimeReceiptState,
) -> RuntimeContractResult<Outcome> {
    let code = match state {
        RuntimeReceiptState::Admitted => ReceiptCode::RequestAdmitted.code(),
        RuntimeReceiptState::Observed => ReceiptCode::RequestObserved.code(),
        RuntimeReceiptState::Queued => ReceiptCode::RequestQueued.code(),
        RuntimeReceiptState::Cancelled => ReceiptCode::RequestCancelled.code(),
        RuntimeReceiptState::Completed => {
            request.operation().kind().success_code().ok_or_else(|| {
                RuntimeContractError::new(RuntimeContractCode::InvalidReceiptOutcome)
            })?
        }
        RuntimeReceiptState::Denied | RuntimeReceiptState::Failed => {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidReceiptOutcome,
            ));
        }
    };
    Ok(Outcome::new(code))
}

impl RuntimeReceipt {
    /// Turns a material read's receipt into a failure after its reply was measured: `outcome` is
    /// the host's failure as the receipt outcome (Workflow #378 A4, coordinator ruling A-M2), and
    /// tops the receipt as [`Self::failure`]'s does.
    pub fn fail_material_read(
        &mut self,
        state: RuntimeMaterialReadState,
        limit: Option<RuntimeMaterialReadLimit>,
        failure: RuntimeMaterialReadFailure,
        outcome: Outcome,
    ) -> RuntimeContractResult<()> {
        let Some(RuntimeResult::MaterialRead { result }) = &mut self.result else {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::MaterialReadResultMissing,
            ));
        };
        if result.failure.is_some() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::MaterialReadFailureAlreadyRecorded,
            ));
        }
        result.chunk = None;
        result.state = state;
        result.limit = limit;
        result.failure = Some(failure);
        self.state = result.receipt_state();
        self.outcome = receipt_envelope(&outcome, self.outcome.detail());
        self.validate()
    }

    /// A material read's receipt. A read that failed carries `failure_outcome`, the host's
    /// failure as the receipt outcome, on top (Workflow #378 A4, coordinator ruling A-M2); one
    /// that did not fail carries none.
    pub fn material_read(
        request: &RuntimeRequest,
        terminal: Option<TerminalEvent>,
        result: Box<RuntimeMaterialReadResult>,
        failure_outcome: Option<Outcome>,
    ) -> RuntimeContractResult<Self> {
        if !matches!(request.operation(), RuntimeOperation::ReadMaterial { request } if request.as_ref() == &result.request)
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::MaterialReadRequestMismatch,
            ));
        }
        let state = result.receipt_state();
        let outcome = match (&result.failure, failure_outcome) {
            (Some(_), Some(outcome)) => outcome,
            (None, None) => receipt_state_outcome(request, state)?,
            _ => {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidReceiptOutcome,
                ));
            }
        };
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: Some(RuntimeResult::MaterialRead { result }),
            outcome: OutcomeEnvelope::new(&outcome, Detail::Full),
            resource_declaration: None,
            resource_declaration_event: None,
            resource_targets_rejection: None,
            channel: None,
            via: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// A contained Lab operation's receipt. An operation that failed carries `failure_outcome`,
    /// the host's failure as the receipt outcome, on top (Workflow #378 A4, coordinator ruling
    /// A-M2); one that did not fail carries none.
    pub fn contained_lab_operation(
        request: &RuntimeRequest,
        terminal: TerminalEvent,
        operation: Box<ContainedLabOperationResult>,
        failure_outcome: Option<Outcome>,
    ) -> RuntimeContractResult<Self> {
        let (state, outcome) = match (&operation.record.failure, failure_outcome) {
            (Some(_), Some(outcome)) => (RuntimeReceiptState::Failed, outcome),
            (None, None) => (
                RuntimeReceiptState::Completed,
                receipt_state_outcome(request, RuntimeReceiptState::Completed)?,
            ),
            _ => {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidReceiptOutcome,
                ));
            }
        };
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal: Some(terminal),
            result: Some(RuntimeResult::ContainedLabOperation { operation }),
            outcome: OutcomeEnvelope::new(&outcome, Detail::Full),
            resource_declaration: None,
            resource_declaration_event: None,
            resource_targets_rejection: None,
            channel: None,
            via: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// A receipt whose request did not fail: admitted, observed, queued, completed or cancelled,
    /// with its result and the outcome of its state.
    pub fn success(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        result: RuntimeResult,
    ) -> RuntimeContractResult<Self> {
        let outcome = receipt_state_outcome(request, state)?;
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: Some(result),
            outcome: OutcomeEnvelope::new(&outcome, Detail::Full),
            resource_declaration: None,
            resource_declaration_event: None,
            resource_targets_rejection: None,
            channel: None,
            via: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// A denied or failed receipt whose outcome is the Runtime error code itself with the
    /// projection's values: a refusal that names no more specific code.
    pub fn error(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        error: RuntimeErrorProjection,
    ) -> RuntimeContractResult<Self> {
        Self::failure(request, state, terminal, error.receipt_outcome(None, None))
    }

    /// A denied or failed receipt carrying its refusal or failure as `outcome` (Workflow #378
    /// A4): the Runtime's specific code on top with the Runtime error code as its `runtime_code`
    /// value ([`RuntimeErrorProjection::receipt_outcome`]).
    pub fn failure(
        request: &RuntimeRequest,
        state: RuntimeReceiptState,
        terminal: Option<TerminalEvent>,
        outcome: Outcome,
    ) -> RuntimeContractResult<Self> {
        let receipt = Self {
            schema_version: RUNTIME_RECEIPT_SCHEMA_VERSION.to_string(),
            request_id: request.request_id,
            correlation_id: request.correlation_id,
            state,
            terminal,
            result: None,
            outcome: OutcomeEnvelope::new(&outcome, Detail::Full),
            resource_declaration: None,
            resource_declaration_event: None,
            resource_targets_rejection: None,
            channel: None,
            via: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// This receipt as the Runtime writes it for its request (model section 7.1): the outcome is
    /// filtered once, here, to the detail the request asks for.
    #[must_use]
    pub fn for_request_detail(mut self, request: &RuntimeRequest) -> Self {
        let detail = request.receipt_detail();
        if detail != self.outcome.detail() {
            self.outcome = receipt_envelope(self.outcome.outcome(), detail);
        }
        self
    }

    /// This receipt tagged with the channel of its connection (Workflow #389 U-2): the door
    /// marks, it never refuses.
    #[must_use]
    pub fn with_channel(mut self, channel: Option<ChannelBinding>) -> Self {
        self.channel = channel.map(|binding| binding.channel);
        self.via = channel.and_then(|binding| binding.via);
        self
    }

    /// The channel of the connection the request came on (Workflow #389 U-2).
    pub const fn channel(&self) -> Option<RuntimeChannel> {
        self.channel
    }

    /// The counter that handed the request over to the Lab channel (Workflow #389 U-2).
    pub const fn via(&self) -> Option<ChannelVia> {
        self.via
    }

    pub fn with_resource_declaration(
        mut self,
        rejection: crate::ResourceDeclarationRejection,
        event: TerminalEvent,
    ) -> RuntimeContractResult<Self> {
        self.resource_declaration = Some(Box::new(rejection));
        self.resource_declaration_event = Some(event);
        self.validate()?;
        Ok(self)
    }

    pub fn resource_declaration(&self) -> Option<&crate::ResourceDeclarationRejection> {
        self.resource_declaration.as_deref()
    }

    pub const fn resource_declaration_event(&self) -> Option<TerminalEvent> {
        self.resource_declaration_event
    }

    /// Attaches the field-positioned rejection of an `ApplyResourceTargets` document to a
    /// Denied `InvalidRequest` receipt without a terminal.
    pub fn with_resource_targets_rejection(
        mut self,
        rejection: crate::ResourceTargetsRejection,
    ) -> RuntimeContractResult<Self> {
        self.resource_targets_rejection = Some(Box::new(rejection));
        self.validate()?;
        Ok(self)
    }

    pub fn resource_targets_rejection(&self) -> Option<&crate::ResourceTargetsRejection> {
        self.resource_targets_rejection.as_deref()
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_RECEIPT_SCHEMA_VERSION {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::UnsupportedReceiptSchema,
            ));
        }
        // Workflow #378 A4: the outcome matches the state. A denied or failed receipt names its
        // Runtime error code (the `runtime_code` value, or the top code itself), so it reads the
        // same in codes mode; every other state carries its own info code, or a success code when
        // completed.
        let runtime_code = self.runtime_code();
        let outcome = self.outcome.outcome();
        let failure_state = matches!(
            self.state,
            RuntimeReceiptState::Denied | RuntimeReceiptState::Failed
        );
        let outcome_matches_state = match self.state {
            RuntimeReceiptState::Admitted => outcome.is(ReceiptCode::RequestAdmitted),
            RuntimeReceiptState::Observed => outcome.is(ReceiptCode::RequestObserved),
            RuntimeReceiptState::Queued => outcome.is(ReceiptCode::RequestQueued),
            RuntimeReceiptState::Cancelled => outcome.is(ReceiptCode::RequestCancelled),
            RuntimeReceiptState::Completed => outcome.category() == Category::Success,
            RuntimeReceiptState::Denied | RuntimeReceiptState::Failed => runtime_code.is_some(),
        };
        if !outcome_matches_state {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidReceiptOutcome,
            ));
        }
        let recorded_lab_failure = matches!(&self.result,
            Some(RuntimeResult::ContainedLabOperation { operation })
                if self.state == RuntimeReceiptState::Failed
                    && operation.record.failure.as_ref().is_some_and(|failure| Some(failure.error.code) == runtime_code));
        let recorded_material_failure = matches!(&self.result,
            Some(RuntimeResult::MaterialRead { result })
                if failure_state
                    && self.state == result.receipt_state()
                    && result.failure.as_ref().is_some_and(|failure| Some(failure.error.code) == runtime_code));
        // A request that did not fail answers with its result; a refusal or failure with none,
        // except the lab operation and the material read, whose results record their failure.
        if !recorded_lab_failure
            && !recorded_material_failure
            && failure_state == self.result.is_some()
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidReceiptOutcome,
            ));
        }
        if self.state == RuntimeReceiptState::Observed
            && !matches!(
                self.result,
                Some(RuntimeResult::ContainedPageObserved { .. })
            )
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidObservationReceipt,
            ));
        }
        if self.terminal.is_some_and(|terminal| terminal.sequence == 0) {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidTerminalEvent,
            ));
        }
        if let Some(rejection) = &self.resource_declaration {
            if !matches!(
                self.state,
                RuntimeReceiptState::Denied | RuntimeReceiptState::Failed
            ) || self.terminal.is_none()
                || self.resource_declaration_event.is_none_or(|event| {
                    event.sequence == 0
                        || self
                            .terminal
                            .is_none_or(|terminal| terminal.sequence < event.sequence)
                })
                || runtime_code != Some(RuntimeErrorCode::PackageInvalid)
                || self.result.is_some()
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidResourceDeclarationReceipt,
                ));
            }
            rejection.validate().map_err(RuntimeContractError::new)?;
        }
        if self.resource_declaration.is_none() && self.resource_declaration_event.is_some() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidResourceDeclarationReceipt,
            ));
        }
        if let Some(rejection) = &self.resource_targets_rejection {
            if self.state != RuntimeReceiptState::Denied
                || self.terminal.is_some()
                || self.result.is_some()
                || runtime_code != Some(RuntimeErrorCode::InvalidRequest)
                || self.resource_declaration.is_some()
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidResourceTargetsReceipt,
                ));
            }
            rejection.validate()?;
        }
        if let Some(RuntimeResult::LeaseGranted { token } | RuntimeResult::LeaseRenewed { token }) =
            &self.result
        {
            token.validate()?;
        }
        match &self.result {
            Some(RuntimeResult::MaterialRead { result }) => {
                result.validate()?;
                if self.state != result.receipt_state()
                    || result.failure.as_ref().map(|failure| failure.error.code) != runtime_code
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidMaterialReadReceipt,
                    ));
                }
            }
            Some(
                RuntimeResult::SignatureRegistered { registration }
                | RuntimeResult::SignatureRetired { registration },
            ) => {
                registration.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidSignatureReceipt)
                })?;
                let terminal = self
                    .terminal
                    .filter(|_| self.state == RuntimeReceiptState::Completed)
                    .ok_or_else(|| {
                        RuntimeContractError::new(RuntimeContractCode::InvalidSignatureReceipt)
                    })?;
                if matches!(
                    &self.result,
                    Some(RuntimeResult::SignatureRegistered { .. })
                ) {
                    if terminal.event_id != registration.event_id
                        || terminal.sequence != registration.sequence
                    {
                        return Err(RuntimeContractError::new(
                            RuntimeContractCode::InvalidSignatureReceipt,
                        ));
                    }
                } else if terminal.sequence <= registration.sequence {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSignatureReceipt,
                    ));
                }
            }
            Some(RuntimeResult::SignaturesMatched { page }) => {
                page.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidSignatureReceipt)
                })?;
                if self.state != RuntimeReceiptState::Completed
                    || self.terminal.is_none_or(|terminal| {
                        terminal.sequence <= page.catalog.observed_through_sequence
                    })
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSignatureReceipt,
                    ));
                }
            }
            Some(RuntimeResult::ShutdownAccepted { target }) => {
                target.validate()?;
                if self.state != RuntimeReceiptState::Admitted || self.terminal.is_none() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidShutdownReceipt,
                    ));
                }
            }
            Some(RuntimeResult::InstallTransition { status }) => {
                status.validate()?;
                if self.state != RuntimeReceiptState::Completed {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidInstallTransitionReceipt,
                    ));
                }
            }
            Some(
                RuntimeResult::UpgradeState { status }
                | RuntimeResult::UpgradeStateStatus { status },
            ) => status.validate()?,
            Some(RuntimeResult::Status { status }) => status.validate()?,
            Some(RuntimeResult::InstancesDiscovered { discovery }) => discovery.validate()?,
            Some(RuntimeResult::SchedulingPaused {
                scope,
                revision,
                drained,
            }) => {
                scope.validate()?;
                if self.state != RuntimeReceiptState::Completed
                    || *revision == 0
                    || drained.is_some() != matches!(scope, SchedulingPauseScope::Instance { .. })
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSchedulingPauseReceipt,
                    ));
                }
            }
            Some(RuntimeResult::SchedulingResumed {
                scope,
                revision,
                selfcheck,
            }) => {
                scope.validate()?;
                if self.state != RuntimeReceiptState::Completed
                    || *revision == 0
                    || selfcheck.is_some() != matches!(scope, SchedulingPauseScope::Instance { .. })
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidSchedulingPauseReceipt,
                    ));
                }
                if let Some(selfcheck) = selfcheck {
                    selfcheck.validate()?;
                }
            }
            Some(RuntimeResult::InstanceSelfChecked {
                instance_alias,
                selfcheck,
            }) => {
                validate_instance_alias(instance_alias)?;
                if self.state != RuntimeReceiptState::Completed {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidInstanceSelfcheckReceipt,
                    ));
                }
                selfcheck.validate()?;
            }
            // Workflow #390 DRILL-1: an admitted drill names a registered instance alias.
            Some(RuntimeResult::RecoveryLadderDrillQueued { drill }) => {
                validate_instance_alias(&drill.instance_alias)?;
            }
            Some(RuntimeResult::ProjectInterface { response }) => {
                response.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidProjectInterfaceResponse)
                })?
            }
            Some(RuntimeResult::PolicyInputIdentityProjected { identity }) => {
                identity.validate()?
            }
            Some(RuntimeResult::MonitorStatus { status }) => status.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeMonitorStatus)
            })?,
            Some(RuntimeResult::InstanceFactSnapshot { snapshot, source }) => {
                snapshot
                    .validate()
                    .map_err(|error| RuntimeContractError::new(error.code()))?;
                source.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeStateSource)
                })?;
            }
            Some(RuntimeResult::PerformanceStatus { status }) => {
                if self.state != RuntimeReceiptState::Completed || self.terminal.is_some() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidPerformanceStatus,
                    ));
                }
                status.validate()?;
            }
            Some(RuntimeResult::FactRefreshStatus { status }) => {
                if self.state != RuntimeReceiptState::Completed || self.terminal.is_some() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidFactRefreshStatus,
                    ));
                }
                status.validate()?;
            }
            Some(RuntimeResult::ScheduleExplanation { explanation }) => {
                if self.state != RuntimeReceiptState::Completed || self.terminal.is_some() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidScheduleExplanation,
                    ));
                }
                explanation.validate()?;
            }
            Some(
                RuntimeResult::MonitorConfigured { status }
                | RuntimeResult::MonitorCleared { status },
            ) => status.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeMonitorStatus)
            })?,
            Some(
                RuntimeResult::LeaseQueued { status } | RuntimeResult::LeasePending { status },
            ) => status.validate()?,
            Some(RuntimeResult::ArtifactRecognized { result }) => {
                result.validate()?;
                if self.state != RuntimeReceiptState::Completed
                    || self.terminal.is_none()
                    || result.artifact.correlation_id != Some(self.correlation_id)
                    || self
                        .terminal
                        .is_some_and(|end| end.sequence <= result.verified.sequence)
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidArtifactOcrReceipt,
                    ));
                }
            }
            Some(RuntimeResult::ReadonlyObservationCompleted { observation }) => {
                observation.validate()?
            }
            Some(RuntimeResult::ContainedPageObserved { observation }) => {
                if self.state != RuntimeReceiptState::Observed || self.terminal.is_none() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidPageObservationReceipt,
                    ));
                }
                observation.validate()?;
            }
            Some(RuntimeResult::ContainedLabOperation { operation }) => {
                operation.validate()?;
                if self.request_id != operation.record.prepared.request_id
                    || self.correlation_id != operation.record.prepared.correlation_id
                    || self.terminal.is_none_or(|terminal| {
                        terminal.sequence <= operation.terminal_artifact.verified.sequence
                    })
                    || (operation.record.failure.is_none()
                        && self.state != RuntimeReceiptState::Completed)
                    || (operation.record.failure.is_some() && !recorded_lab_failure)
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidLabOperationReceipt,
                    ));
                }
            }
            Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => sequence.validate()?,
            Some(RuntimeResult::EventPage { page }) => page.validate()?,
            Some(RuntimeResult::EventBatch { batch }) => batch.validate()?,
            Some(RuntimeResult::PackageDebugCompleted { summary }) => summary.validate()?,
            Some(RuntimeResult::EvidenceExportCompleted { summary }) => summary.validate()?,
            Some(RuntimeResult::ContainedTaskCompleted {
                response_deadline_monotonic_ms,
                task_result,
                final_page,
                executed_steps,
                ..
            }) if response_deadline_monotonic_ms.is_some_and(|deadline| deadline == 0)
                || *task_result != TaskOutcome::Success
                || *executed_steps > 1_000
                || final_page
                    .as_deref()
                    .is_some_and(|value| value.is_empty() || value.len() > 256) =>
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidContainedTaskResult,
                ));
            }
            Some(RuntimeResult::ContainedTaskCancelled {
                response_deadline_monotonic_ms,
                ..
            }) if self.state != RuntimeReceiptState::Cancelled
                || response_deadline_monotonic_ms.is_some_and(|deadline| deadline == 0) =>
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidContainedTaskResult,
                ));
            }
            Some(RuntimeResult::ContainedTaskCancellation { status, .. }) => status.validate()?,
            Some(RuntimeResult::LabPinReleased { release }) => {
                release.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidLabPinReleaseReceipt)
                })?;
                if self.state != RuntimeReceiptState::Completed
                    || self.terminal != Some(release.released)
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidLabPinReleaseReceipt,
                    ));
                }
            }
            Some(
                RuntimeResult::AgentSessionOpened { context }
                | RuntimeResult::AgentSessionObserved { context },
            ) => context.validate().map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidAgentSessionContext)
            })?,
            Some(RuntimeResult::AgentResponseRecorded { status }) => {
                status.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidAgentSessionStatus)
                })?
            }
            Some(RuntimeResult::StrategicPlanPrepared { plan }) => plan.validate()?,
            Some(RuntimeResult::PolicyForwardProjected { projection }) => {
                projection.validate_kind(RuntimePlanningDocumentKind::ForwardProjection)?
            }
            Some(RuntimeResult::PredictiveMaintenanceAssessed { assessment }) => {
                assessment.validate_kind(RuntimePlanningDocumentKind::MaintenanceAssessmentV2)?
            }
            Some(RuntimeResult::ProposalEvaluated { preview }) => {
                preview.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidProposalPreview)
                })?
            }
            Some(RuntimeResult::ProposalPromoted { promotion }) => {
                promotion.validate().map_err(|_| {
                    RuntimeContractError::new(RuntimeContractCode::InvalidProposalPromotion)
                })?
            }
            Some(RuntimeResult::ResourceTargetsApplied { applied }) => {
                if self.state != RuntimeReceiptState::Completed
                    || self.terminal
                        != Some(TerminalEvent {
                            sequence: applied.version,
                            event_id: applied.event_id,
                        })
                {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidResourceTargetsReceipt,
                    ));
                }
                applied.validate()?;
            }
            Some(RuntimeResult::ResourceTargetView { view }) => {
                if self.state != RuntimeReceiptState::Completed || self.terminal.is_some() {
                    return Err(RuntimeContractError::new(
                        RuntimeContractCode::InvalidResourceTargetViewReceipt,
                    ));
                }
                view.validate()?;
            }
            Some(RuntimeResult::SettingsConfigured { settings })
                if self.state != RuntimeReceiptState::Completed
                    || self.terminal.is_none()
                    || settings.is_empty()
                    || settings.len() > MAX_CONFIG_SETTING_CHANGES =>
            {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidConfigureSettings,
                ));
            }
            _ => {}
        }
        Ok(())
    }

    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    pub const fn state(&self) -> RuntimeReceiptState {
        self.state
    }

    pub const fn terminal(&self) -> Option<TerminalEvent> {
        self.terminal
    }

    pub const fn result(&self) -> Option<&RuntimeResult> {
        self.result.as_ref()
    }

    /// The receipt's outcome (Workflow #378 A4): the state's info code, the request kind's
    /// success code, or the refusal or failure.
    pub fn outcome(&self) -> &Outcome {
        self.outcome.outcome()
    }

    /// The receipt's outcome in its interface form, with the detail it was written with.
    pub const fn outcome_envelope(&self) -> &OutcomeEnvelope {
        &self.outcome
    }

    /// The projection a denied or failed receipt's outcome carries
    /// ([`RuntimeErrorProjection::from_receipt_outcome`]); `None` for every other state.
    pub fn error_projection(&self) -> Option<RuntimeErrorProjection> {
        if matches!(
            self.state,
            RuntimeReceiptState::Denied | RuntimeReceiptState::Failed
        ) {
            RuntimeErrorProjection::from_receipt_outcome(self.outcome.outcome())
        } else {
            None
        }
    }

    /// The Runtime error code of a denied or failed receipt.
    fn runtime_code(&self) -> Option<RuntimeErrorCode> {
        self.error_projection().map(|projection| projection.code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeInfo {
    schema_version: String,
    pid: u32,
    host: String,
    port: u16,
    owner_epoch: OwnerEpoch,
    started_at_unix_ms: u64,
}

/// Exact process selected by local Runtime discovery; this is not an authentication credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeShutdownTarget {
    pub owner_epoch: OwnerEpoch,
    pub pid: u32,
    pub started_at_unix_ms: u64,
}

impl RuntimeShutdownTarget {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.pid == 0 || self.started_at_unix_ms == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidShutdownTarget,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeShutdownDecision {
    Accepted,
    Busy,
    OwnerMismatch,
    AlreadyStopping,
}

impl RuntimeInfo {
    pub const fn shutdown_target(&self) -> RuntimeShutdownTarget {
        RuntimeShutdownTarget {
            owner_epoch: self.owner_epoch,
            pid: self.pid,
            started_at_unix_ms: self.started_at_unix_ms,
        }
    }
    pub fn new(
        pid: u32,
        host: impl Into<String>,
        port: u16,
        owner_epoch: OwnerEpoch,
        started_at_unix_ms: u64,
    ) -> RuntimeContractResult<Self> {
        let info = Self {
            schema_version: RUNTIME_INFO_SCHEMA_VERSION.to_string(),
            pid,
            host: host.into(),
            port,
            owner_epoch,
            started_at_unix_ms,
        };
        info.validate()?;
        Ok(info)
    }

    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.schema_version != RUNTIME_INFO_SCHEMA_VERSION {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::UnsupportedRuntimeInfoSchema,
            ));
        }
        let host = self
            .host
            .parse::<IpAddr>()
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeHost))?;
        if self.pid == 0 || self.port == 0 || self.started_at_unix_ms == 0 || !host.is_loopback() {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidRuntimeInfo,
            ));
        }
        Ok(())
    }

    pub fn socket_addr(&self) -> RuntimeContractResult<SocketAddr> {
        self.validate()?;
        let host = self
            .host
            .parse::<IpAddr>()
            .map_err(|_| RuntimeContractError::new(RuntimeContractCode::InvalidRuntimeHost))?;
        Ok(SocketAddr::new(host, self.port))
    }

    pub const fn pid(&self) -> u32 {
        self.pid
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub const fn started_at_unix_ms(&self) -> u64 {
        self.started_at_unix_ms
    }
}

fn validate_point(x: i32, y: i32) -> RuntimeContractResult<()> {
    if x < 0 || y < 0 {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidInputCoordinate,
        ));
    }
    Ok(())
}

fn validate_duration(duration_ms: u64) -> RuntimeContractResult<()> {
    if !(1..=MAX_INPUT_DURATION_MS).contains(&duration_ms) {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidInputDuration,
        ));
    }
    Ok(())
}

/// Workflow #336 PKG-S2 (option 丙): a package named by its `package_id` in Lab and observation
/// evidence (`package_label`), bounded as a task label: 1 to 256 bytes, no control character.
fn is_package_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn validate_sha256_hex(value: &str) -> RuntimeContractResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidSha256,
        ));
    }
    Ok(())
}

fn validate_canonical_sha256(value: &str) -> RuntimeContractResult<()> {
    value
        .strip_prefix("sha256:")
        .ok_or_else(|| RuntimeContractError::new(RuntimeContractCode::InvalidSha256))
        .and_then(validate_sha256_hex)
}

const fn terminal_event_type(outcome: TaskOutcome) -> EventType {
    match outcome {
        TaskOutcome::Success => EventType::TaskCompleted,
        TaskOutcome::Failure => EventType::TaskFailed,
        TaskOutcome::Cancelled => EventType::TaskCancelled,
    }
}

/// Validates a registered instance alias without changing its UTF-8 bytes.
pub fn validate_instance_alias(value: &str) -> RuntimeContractResult<()> {
    if value.is_empty()
        || value.len() > MAX_INSTANCE_ALIAS_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(RuntimeContractError::new(SharedCode::InvalidInstanceAlias));
    }
    Ok(())
}

/// A governance connection's declarative identity card (Workflow #318 cfg4): the connecting
/// client says who it is instead of presenting a shared secret. The Runtime verifies the card
/// against its governance policy and records every declaration, accepted or refused, as a
/// `governance.identity_declared` event. Actor and source are the request envelope's and are
/// never repeated here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceIdentityCard {
    /// The client name: `1..=64` bytes of `[A-Za-z0-9._-]` (`validate_governance_client`).
    pub client: String,
    /// The client's own version text: `1..=32` bytes without control characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    /// The instance alias the client acts for; the Runtime requires it to be registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl GovernanceIdentityCard {
    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_governance_client(&self.client)?;
        if let Some(version) = &self.client_version
            && (version.is_empty()
                || version.len() > MAX_GOVERNANCE_CLIENT_VERSION_BYTES
                || version.chars().any(char::is_control))
        {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidGovernanceClientVersion,
            ));
        }
        if let Some(instance) = &self.instance {
            validate_instance_alias(instance).map_err(|_| {
                RuntimeContractError::new(RuntimeContractCode::InvalidGovernanceInstance)
            })?;
        }
        Ok(())
    }
}

/// The card's client-name rule, shared with governance policy allow-lists.
pub fn validate_governance_client(client: &str) -> RuntimeContractResult<()> {
    if client.is_empty()
        || client.len() > MAX_GOVERNANCE_CLIENT_BYTES
        || !client
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(RuntimeContractError::new(
            RuntimeContractCode::InvalidGovernanceClient,
        ));
    }
    Ok(())
}

/// The origins that may declare a governance identity card: the person at the console
/// (User, Ui) and the operator (Cli, Cli).
pub const fn valid_governance_declaration_origin(actor: EventActor, source: EventSource) -> bool {
    matches!(
        (actor, source),
        (EventActor::User, EventSource::Ui) | (EventActor::Cli, EventSource::Cli)
    )
}

/// Bound of the configured game identifier a status instance carries (the scheduling
/// identifier bound).
const MAX_STATUS_GAME_ID_BYTES: usize = 128;

fn validate_bounded_text(
    value: &str,
    max_bytes: usize,
    code: impl Into<Code>,
) -> RuntimeContractResult<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.chars().any(|character| character == '\0')
    {
        return Err(RuntimeContractError::new(code));
    }
    Ok(())
}

fn valid_client_origin(actor: EventActor, source: EventSource) -> bool {
    matches!(
        source,
        EventSource::Cli | EventSource::Ui | EventSource::Lab | EventSource::Adapter
    ) && matches!(
        actor,
        EventActor::User | EventActor::Cli | EventActor::Ui | EventActor::Lab | EventActor::Agent
    )
}

#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
