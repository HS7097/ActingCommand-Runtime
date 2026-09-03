// SPDX-License-Identifier: AGPL-3.0-only

use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, ReceiptReadDeadline, exchange};
use crate::{RuntimeClientError, RuntimeClientResult};
use actingcommand_contract::{
    ActionId, AgentSessionContext, AgentSessionId, AgentSessionResponse, AgentSessionStatus,
    AgentWakeId, ApplicationLifecycleAction, ApprovalDecisionRecord, ArtifactKind,
    ArtifactProducer, ArtifactRedactionState, CaptureSequenceSpec, CatalogProposal,
    ClientActionRecord, ContainedTaskCancellationReason, ContainedTaskCancellationStatus,
    ContainedTaskRequest, CorrelationId, EffectDisposition, EventActor, EventId, EventPayload,
    EventQuery, EventSource, EventType, FactRecord, FactScope, FrameId, IdentifierIssuer,
    InputAction, InputPayload, IssuedCorrelationId, LeaseQueuePolicy, LeaseQueueStatus, LeaseToken,
    MAX_RUNTIME_EVENT_QUERY_EVENTS, OriginModule, OwnerEpoch, PackageDebugRequest,
    PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition, PolicyPayload,
    ProjectDecisionPageCursor, ProjectDecisionPageRequest, ProjectInterfaceRequest,
    ProjectLedgerSnapshot, ProjectedArtifactReference, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, ProposalPreview, ProposalPromotion, RUNTIME_INFO_FILE, RequestId,
    ResourceAuthoringEvent, RetentionClass, RunId, RuntimeControlPlaneStatus, RuntimeDebugEvent,
    RuntimeErrorCode, RuntimeEventBatch, RuntimeEventQueryPage, RuntimeEventQueryPageRequest,
    RuntimeEvidenceExportRequest, RuntimeForwardProjectionRequest, RuntimeInfo,
    RuntimeMaintenanceQuery, RuntimeMonitorInstanceStatus, RuntimeMonitorPolicy,
    RuntimeMonitorRegistryStatus, RuntimeOperation, RuntimePlanningDocument,
    RuntimePlanningDocumentKind, RuntimePolicyInputIdentity, RuntimeReceipt, RuntimeRequest,
    RuntimeResult, RuntimeStrategicReportRequest, RuntimeSubscriptionRequest, TaskId, TaskOutcome,
    TaskPayload, TaskSemanticFact, TerminalEvent,
};
use actingcommand_policy::{
    EvaluationFacts, EvaluationResources, EvaluationTime, ForwardProjection,
    ForwardProjectionConfig, MaintenanceAssessment, MaintenanceTrendPolicy, StrategicProjection,
    StrategicReport,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "test-observation")]
use crate::test_observation::{
    ObservationOperation, ObservationOutcome, ObservationStage, ObservationThreadRole,
    record_active,
};

const DEFAULT_RUNTIME_IO_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_BACKEND_OPEN_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RUNTIME_IO_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BACKEND_OPEN_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RUNTIME_INFO_BYTES: u64 = 64 * 1024;
const MAX_RUN_SUMMARY_EVENTS: usize = 16_384;
const MAX_RUN_SUMMARY_PAGES: usize = 64;
const MAX_RUN_SUMMARY_RESIDENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPLETE_EVENT_QUERY_EVENTS: usize = 16_384;
const MAX_COMPLETE_EVENT_QUERY_PAGES: usize = 64;
const MAX_COMPLETE_EVENT_QUERY_DURATION: Duration = Duration::from_secs(60);
const MAX_OFFICIAL_OCR_ARTIFACTS: usize = 257;
pub(crate) const MAX_OFFICIAL_OCR_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OFFICIAL_OCR_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OFFICIAL_OCR_PROJECTION_DURATION: Duration = Duration::from_secs(60);
const MAX_OFFICIAL_OCR_FRAMES: u32 = 256;
const MAX_OFFICIAL_OCR_ITEMS: usize = 4_096;
const MAX_OFFICIAL_OCR_MAPPING_FACTS: usize = 16_384;
const OFFICIAL_OCR_PROJECTION_SCHEMA: &str = "actingcommand.runtime.official-ocr-projection.v2";
pub(crate) const OCR_OBSERVATION_SCHEMA: &str =
    "actingcommand.runtime.post-admission-ocr-observation.v1";
pub(crate) const OCR_COMPARISON_ENVELOPE_SCHEMA: &str =
    "actingcommand.runtime.post-admission-ocr-comparison-envelope.v1";
const OCR_COMPARISON_SCHEMA_V1: &str = "actingcommand.runtime.post-admission-ocr-comparison.v1";
pub(crate) const OCR_COMPARISON_SCHEMA_V2: &str =
    "actingcommand.runtime.post-admission-ocr-comparison.v2";

/// Discovery, identity, framing, and timeout configuration for one local Runtime session.
#[derive(Clone)]
pub struct RuntimeClientConfig {
    state_root: PathBuf,
    actor: EventActor,
    source: EventSource,
    io_timeout: Duration,
    backend_open_timeout: Duration,
    maximum_frame_bytes: usize,
}

impl RuntimeClientConfig {
    pub fn new(state_root: impl Into<PathBuf>, actor: EventActor, source: EventSource) -> Self {
        Self {
            state_root: state_root.into(),
            actor,
            source,
            io_timeout: DEFAULT_RUNTIME_IO_TIMEOUT,
            backend_open_timeout: DEFAULT_BACKEND_OPEN_TIMEOUT,
            maximum_frame_bytes: DEFAULT_RUNTIME_MAX_FRAME_BYTES,
        }
    }

    pub fn with_io_timeout(mut self, io_timeout: Duration) -> Self {
        self.io_timeout = io_timeout;
        self
    }

    pub fn with_maximum_frame_bytes(mut self, maximum_frame_bytes: usize) -> Self {
        self.maximum_frame_bytes = maximum_frame_bytes;
        self
    }

    pub fn with_backend_open_timeout(mut self, backend_open_timeout: Duration) -> Self {
        self.backend_open_timeout = backend_open_timeout;
        self
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    fn validate(&self) -> RuntimeClientResult<()> {
        if self.state_root.as_os_str().is_empty()
            || self.io_timeout.is_zero()
            || self.io_timeout > MAX_RUNTIME_IO_TIMEOUT
            || self.backend_open_timeout.is_zero()
            || self.backend_open_timeout > MAX_BACKEND_OPEN_TIMEOUT
            || self.maximum_frame_bytes == 0
            || self.maximum_frame_bytes > DEFAULT_RUNTIME_MAX_FRAME_BYTES
        {
            return Err(RuntimeClientError::fatal(
                "runtime_client_config_invalid",
                "connect_runtime",
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for RuntimeClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeClientConfig")
            .field("state_root", &"<redacted>")
            .field("actor", &self.actor)
            .field("source", &self.source)
            .field("io_timeout", &self.io_timeout)
            .field("backend_open_timeout", &self.backend_open_timeout)
            .field("maximum_frame_bytes", &self.maximum_frame_bytes)
            .finish()
    }
}

struct RuntimeConnection {
    stream: TcpStream,
    ids: IdentifierIssuer,
    actor: EventActor,
    source: EventSource,
    io_timeout: Duration,
    backend_open_timeout: Duration,
    maximum_frame_bytes: usize,
    terminal_error: Option<RuntimeClientError>,
}

struct RuntimeClientShared {
    info: RuntimeInfo,
    state_root: PathBuf,
    connection: Mutex<RuntimeConnection>,
}

/// Cloneable handle to one connection-bound Runtime IPC session.
#[derive(Clone)]
pub struct RuntimeClient {
    shared: Arc<RuntimeClientShared>,
}

/// Read-only project projection client. It exposes neither device operations nor ledger writes.
#[derive(Clone)]
pub struct RuntimeProjectClient {
    client: RuntimeClient,
}

/// Correlation-scoped authoring ingress. Runtime remains the only global-ledger writer.
#[derive(Clone)]
pub struct RuntimeAuthoringSession {
    client: RuntimeClient,
    correlation: IssuedCorrelationId,
}

/// Correlation-scoped Lab adapter for Runtime-owned capture, scheduling, input, and ledger facts.
#[derive(Clone)]
pub struct RuntimeDebugSession {
    client: RuntimeClient,
    correlation: IssuedCorrelationId,
}

/// Host receipt plus its correlation-scoped durable ledger projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFlowOutput {
    receipt: RuntimeReceipt,
    events: Vec<ProjectedEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    official_ocr_projection: Option<RuntimeOfficialOcrProjection>,
}

/// Authoritative OCR comparison and provider facts resolved from this run's durable artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrProjection {
    schema_version: &'static str,
    run_id: RunId,
    task_id: TaskId,
    comparison_artifact: ProjectedArtifactReference,
    comparison_artifact_created_event_id: EventId,
    comparison_artifact_created_sequence: u64,
    comparison_artifact_verified_event_id: EventId,
    comparison_artifact_verified_sequence: u64,
    observations: Vec<RuntimeOfficialOcrObservation>,
    summary: RuntimeOfficialOcrSummary,
    provider_execution: RuntimeOfficialOcrProviderExecution,
    comparison: RuntimeOfficialOcrComparison,
}

/// One durable OCR observation and its complete target coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrObservation {
    frame_id: FrameId,
    frame_index: u32,
    artifact: ProjectedArtifactReference,
    artifact_created_event_id: EventId,
    artifact_created_sequence: u64,
    artifact_verified_event_id: EventId,
    artifact_verified_sequence: u64,
    target_ids: Vec<String>,
}

/// Stable human- and machine-readable index over the complete comparison payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrSummary {
    unique_canonical_count: usize,
    canonical_names: Vec<String>,
    screen_coverage: Vec<RuntimeOfficialOcrScreenCoverage>,
    duplicates: Vec<RuntimeOfficialOcrDuplicate>,
    unmatched_raw_readings: Vec<RuntimeOfficialOcrUnmatchedReading>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrScreenCoverage {
    frame_index: u32,
    target_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrDuplicate {
    name: String,
    occurrences: u32,
}

/// Complete authoritative comparison emitted by the execution owner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrComparison {
    schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_ids: Option<Vec<String>>,
    truth_set_path: String,
    truth_set_sha256: String,
    normalization: RuntimeOfficialOcrNormalization,
    comparison: RuntimeOfficialOcrComparisonMode,
    outcome_key: String,
    frames_collected: u32,
    items_collected: u32,
    discarded_empty_items: u32,
    total_observed_utf8_bytes: u64,
    exact_match: bool,
    truth: Vec<String>,
    observed: Vec<RuntimeOfficialOcrObservedValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    classification_contract: Option<RuntimeOfficialOcrClassificationContract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mapping_evidence: Option<Vec<RuntimeOfficialOcrMappingEvidence>>,
    missed: Vec<String>,
    unexpected: Vec<String>,
    duplicates: Vec<RuntimeOfficialOcrDuplicateEvidence>,
}

impl Eq for RuntimeOfficialOcrComparison {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOfficialOcrNormalization {
    TrimLowercaseV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOfficialOcrComparisonMode {
    ExactSetV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrObservedValue {
    value: String,
    occurrences: u32,
    confidences: Vec<Option<f32>>,
}

impl Eq for RuntimeOfficialOcrObservedValue {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrClassificationContract {
    predicate: String,
    max_substitutions: u8,
    max_retry_index: u32,
    max_candidate_checks: u64,
    candidate_checks: u64,
    max_scalar_comparisons: u64,
    scalar_comparisons: u64,
    max_candidate_witnesses: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrMappingEvidence {
    frame_index: u32,
    retry_index: u32,
    target_id: String,
    raw_text: String,
    normalized_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical: Option<String>,
    confidence: Option<f32>,
    candidate_count: u32,
    candidates: Vec<RuntimeOfficialOcrCandidateEvidence>,
    disposition: RuntimeOfficialOcrMappingDisposition,
}

impl Eq for RuntimeOfficialOcrMappingEvidence {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrCandidateEvidence {
    canonical: String,
    differing_scalars: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOfficialOcrMappingDisposition {
    CanonicalExact,
    AliasExact,
    TolerantUnique,
    RetryRequiredAbsent,
    RetryRequiredAmbiguous,
    UnmatchedAfterRetry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrDuplicateEvidence {
    value: String,
    occurrences: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrUnmatchedReading {
    frame_index: u32,
    retry_index: u32,
    target_id: String,
    raw_text: String,
    normalized_text: String,
    disposition: RuntimeOfficialOcrMappingDisposition,
}

/// Actual provider stream binding plus the persisted invocation witnesses for this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrProviderExecution {
    requested_provider: RuntimeOfficialOcrProviderKind,
    actual_provider: RuntimeOfficialOcrProviderKind,
    requested_cuda_ordinal: Option<u32>,
    requested_cuda_identity: Option<String>,
    actual_cuda_ordinal: Option<u32>,
    actual_cuda_identity: Option<String>,
    provider_implementation: String,
    provider_binary_sha256: String,
    runtime_version: String,
    model_ref: String,
    model_sha256: String,
    cpu_ep_registered: bool,
    cpu_fallback_disabled: bool,
    fallback_forbidden: bool,
    fallback_observed: Option<bool>,
    strict_no_fallback: bool,
    complete: bool,
    session_id: String,
    session_generation: u64,
    evidence: Vec<RuntimeOfficialOcrProviderEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOfficialOcrProviderEvidence {
    frame_index: u32,
    target_id: String,
    invocation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOfficialOcrProviderKind {
    Cpu,
    Cuda,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseAdmission {
    Granted(LeaseToken),
    Queued(LeaseQueueStatus),
}

/// Typed client projection of one strategic report preparation completed by resident Runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStrategicPlan {
    report: ProjectedArtifactReference,
    projection: StrategicProjection,
    proposal: Option<CatalogProposal>,
    preview: Option<ProposalPreview>,
}

impl RuntimeStrategicPlan {
    pub const fn report(&self) -> &ProjectedArtifactReference {
        &self.report
    }

    pub const fn projection(&self) -> &StrategicProjection {
        &self.projection
    }

    pub const fn proposal(&self) -> Option<&CatalogProposal> {
        self.proposal.as_ref()
    }

    pub const fn preview(&self) -> Option<&ProposalPreview> {
        self.preview.as_ref()
    }
}

/// Typed maintenance query converted to the policy-independent wire envelope at send time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictiveMaintenanceRequest {
    transport: RuntimeMaintenanceQuery,
}

impl PredictiveMaintenanceRequest {
    pub fn new(
        instance_id: impl Into<String>,
        task_id: impl Into<String>,
        fact_scope: FactScope,
        fact_key: impl Into<String>,
        as_of_ledger_position: u64,
        as_of_unix_ms: u64,
        trend_policy: MaintenanceTrendPolicy,
    ) -> RuntimeClientResult<Self> {
        trend_policy.validate().map_err(|_| {
            RuntimeClientError::fatal(
                "maintenance_trend_policy_invalid",
                "build_predictive_maintenance_request",
            )
        })?;
        let policy = encode_policy_document(
            RuntimePlanningDocumentKind::MaintenanceTrendPolicy,
            &trend_policy,
            "build_predictive_maintenance_request",
        )?;
        let transport = RuntimeMaintenanceQuery::new(
            instance_id,
            task_id,
            fact_scope,
            fact_key,
            as_of_ledger_position,
            as_of_unix_ms,
            policy,
        )
        .map_err(|_| {
            RuntimeClientError::fatal(
                "predictive_maintenance_request_invalid",
                "build_predictive_maintenance_request",
            )
        })?;
        Ok(Self { transport })
    }
}

impl RuntimeFlowOutput {
    pub const fn receipt(&self) -> &RuntimeReceipt {
        &self.receipt
    }

    pub fn events(&self) -> &[ProjectedEvent] {
        &self.events
    }

    pub const fn official_ocr_projection(&self) -> Option<&RuntimeOfficialOcrProjection> {
        self.official_ocr_projection.as_ref()
    }
}

impl RuntimeOfficialOcrProjection {
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    pub const fn task_id(&self) -> TaskId {
        self.task_id
    }

    pub const fn comparison_artifact(&self) -> &ProjectedArtifactReference {
        &self.comparison_artifact
    }

    pub const fn comparison_artifact_created_sequence(&self) -> u64 {
        self.comparison_artifact_created_sequence
    }

    pub const fn comparison_artifact_created_event_id(&self) -> EventId {
        self.comparison_artifact_created_event_id
    }

    pub const fn comparison_artifact_verified_sequence(&self) -> u64 {
        self.comparison_artifact_verified_sequence
    }

    pub const fn comparison_artifact_verified_event_id(&self) -> EventId {
        self.comparison_artifact_verified_event_id
    }

    pub fn observations(&self) -> &[RuntimeOfficialOcrObservation] {
        &self.observations
    }

    pub const fn summary(&self) -> &RuntimeOfficialOcrSummary {
        &self.summary
    }

    pub const fn provider_execution(&self) -> &RuntimeOfficialOcrProviderExecution {
        &self.provider_execution
    }

    pub const fn comparison(&self) -> &RuntimeOfficialOcrComparison {
        &self.comparison
    }
}

impl RuntimeOfficialOcrObservation {
    pub const fn frame_id(&self) -> FrameId {
        self.frame_id
    }

    pub const fn frame_index(&self) -> u32 {
        self.frame_index
    }

    pub const fn artifact(&self) -> &ProjectedArtifactReference {
        &self.artifact
    }

    pub const fn artifact_created_sequence(&self) -> u64 {
        self.artifact_created_sequence
    }

    pub const fn artifact_created_event_id(&self) -> EventId {
        self.artifact_created_event_id
    }

    pub const fn artifact_verified_sequence(&self) -> u64 {
        self.artifact_verified_sequence
    }

    pub const fn artifact_verified_event_id(&self) -> EventId {
        self.artifact_verified_event_id
    }

    pub fn target_ids(&self) -> &[String] {
        &self.target_ids
    }
}

impl RuntimeClient {
    pub fn connect(config: RuntimeClientConfig) -> RuntimeClientResult<Self> {
        config.validate()?;
        let info = read_runtime_info(config.state_root())?;
        let address = info
            .socket_addr()
            .map_err(|_| RuntimeClientError::fatal("runtime_info_invalid", "connect_runtime"))?;
        let stream = connect_runtime_stream(address, config.io_timeout, "connect_runtime")?;
        let client = Self {
            shared: Arc::new(RuntimeClientShared {
                info,
                state_root: config.state_root,
                connection: Mutex::new(RuntimeConnection {
                    stream,
                    ids: IdentifierIssuer::new().map_err(|_| {
                        RuntimeClientError::fatal(
                            "runtime_identifier_issuer_failed",
                            "connect_runtime",
                        )
                    })?,
                    actor: config.actor,
                    source: config.source,
                    io_timeout: config.io_timeout,
                    backend_open_timeout: config.backend_open_timeout,
                    maximum_frame_bytes: config.maximum_frame_bytes,
                    terminal_error: None,
                }),
            }),
        };
        let observed_epoch = client.health()?;
        if observed_epoch != client.shared.info.owner_epoch() {
            return Err(RuntimeClientError::fatal(
                "runtime_owner_epoch_changed",
                "connect_runtime",
            ));
        }
        Ok(client)
    }

    pub fn runtime_info(&self) -> &RuntimeInfo {
        &self.shared.info
    }

    pub fn health(&self) -> RuntimeClientResult<OwnerEpoch> {
        match self.execute("runtime_health", RuntimeOperation::Health)? {
            RuntimeResult::Health { owner_epoch } => Ok(owner_epoch),
            _ => Err(self.unexpected_result("runtime_health")),
        }
    }

    pub fn status(&self) -> RuntimeClientResult<RuntimeControlPlaneStatus> {
        match self.execute("runtime_status", RuntimeOperation::Status)? {
            RuntimeResult::Status { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_status")),
        }
    }

    pub fn publish_fact(&self, record: FactRecord) -> RuntimeClientResult<EventId> {
        match self.execute("publish_fact", RuntimeOperation::PublishFact { record })? {
            RuntimeResult::FactPublished { event_id } => Ok(event_id),
            _ => Err(self.unexpected_result("publish_fact")),
        }
    }

    pub fn project_snapshot(
        &self,
        request: ProjectInterfaceRequest,
    ) -> RuntimeClientResult<ProjectLedgerSnapshot> {
        match self.execute(
            "runtime_project_interface",
            RuntimeOperation::ProjectInterface { request },
        )? {
            RuntimeResult::ProjectInterface { response } => {
                response.into_snapshot().map_err(|_| {
                    RuntimeClientError::fatal(
                        "runtime_project_interface_invalid",
                        "runtime_project_interface",
                    )
                })
            }
            _ => Err(self.unexpected_result("runtime_project_interface")),
        }
    }

    pub fn monitor_status(&self) -> RuntimeClientResult<RuntimeMonitorRegistryStatus> {
        match self.execute("runtime_monitor_status", RuntimeOperation::MonitorStatus)? {
            RuntimeResult::MonitorStatus { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_monitor_status")),
        }
    }

    pub fn configure_monitor(
        &self,
        instance_alias: &str,
        policy: RuntimeMonitorPolicy,
    ) -> RuntimeClientResult<RuntimeMonitorInstanceStatus> {
        match self.execute(
            "runtime_monitor_configure",
            RuntimeOperation::ConfigureMonitor {
                instance_alias: instance_alias.to_string(),
                policy,
            },
        )? {
            RuntimeResult::MonitorConfigured { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_monitor_configure")),
        }
    }

    pub fn clear_monitor(
        &self,
        instance_alias: &str,
    ) -> RuntimeClientResult<RuntimeMonitorInstanceStatus> {
        match self.execute(
            "runtime_monitor_clear",
            RuntimeOperation::ClearMonitor {
                instance_alias: instance_alias.to_string(),
            },
        )? {
            RuntimeResult::MonitorCleared { status } => Ok(status),
            _ => Err(self.unexpected_result("runtime_monitor_clear")),
        }
    }

    pub fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientAcquireStart,
            ObservationOperation::AcquireLease,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            None,
        );
        let result = (|| {
            let holder = self
                .connection("issue_lease_holder")?
                .ids
                .mint_holder_id()
                .map_err(|_| {
                    RuntimeClientError::fatal("runtime_identifier_issue_failed", "acquire_lease")
                })?;
            match self.execute(
                "acquire_lease",
                RuntimeOperation::acquire_lease(instance_alias, holder),
            )? {
                RuntimeResult::LeaseGranted { token } => Ok(token),
                _ => Err(self.unexpected_result("acquire_lease")),
            }
        })();
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientAcquireResult,
            ObservationOperation::AcquireLease,
            ObservationThreadRole::Client,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            None,
            None,
            result.as_ref().ok(),
        );
        result
    }

    pub fn queue_lease(
        &self,
        instance_alias: &str,
        policy: LeaseQueuePolicy,
    ) -> RuntimeClientResult<LeaseAdmission> {
        let holder = self
            .connection("issue_queued_lease_holder")?
            .ids
            .mint_holder_id()
            .map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", "queue_lease")
            })?;
        match self.execute(
            "queue_lease",
            RuntimeOperation::queue_lease(instance_alias, holder, policy),
        )? {
            RuntimeResult::LeaseGranted { token } => Ok(LeaseAdmission::Granted(token)),
            RuntimeResult::LeaseQueued { status } => Ok(LeaseAdmission::Queued(status)),
            _ => Err(self.unexpected_result("queue_lease")),
        }
    }

    pub fn poll_queued_lease(
        &self,
        queued_request_id: RequestId,
    ) -> RuntimeClientResult<LeaseAdmission> {
        match self.execute(
            "poll_queued_lease",
            RuntimeOperation::PollQueuedLease { queued_request_id },
        )? {
            RuntimeResult::LeaseGranted { token } => Ok(LeaseAdmission::Granted(token)),
            RuntimeResult::LeasePending { status } => Ok(LeaseAdmission::Queued(status)),
            _ => Err(self.unexpected_result("poll_queued_lease")),
        }
    }

    pub fn cancel_queued_lease(&self, queued_request_id: RequestId) -> RuntimeClientResult<()> {
        match self.execute(
            "cancel_queued_lease",
            RuntimeOperation::CancelQueuedLease { queued_request_id },
        )? {
            RuntimeResult::LeaseQueueCancelled { request_id, .. }
                if request_id == queued_request_id =>
            {
                Ok(())
            }
            _ => Err(self.unexpected_result("cancel_queued_lease")),
        }
    }

    pub fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientRenewStart,
            ObservationOperation::RenewLease,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            Some(token),
        );
        let result = match self.execute(
            "renew_lease",
            RuntimeOperation::RenewLease {
                token: token.clone(),
            },
        ) {
            Ok(RuntimeResult::LeaseRenewed { token }) => Ok(token),
            Ok(_) => Err(self.unexpected_result("renew_lease")),
            Err(error) => Err(error),
        };
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientRenewResult,
            ObservationOperation::RenewLease,
            ObservationThreadRole::Client,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            None,
            None,
            result.as_ref().ok().or(Some(token)),
        );
        result
    }

    pub fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientReleaseStart,
            ObservationOperation::ReleaseLease,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            Some(token),
        );
        let result = match self.execute(
            "release_lease",
            RuntimeOperation::ReleaseLease {
                token: token.clone(),
            },
        ) {
            Ok(RuntimeResult::LeaseReleased {
                instance_id,
                lease_id,
            }) if instance_id == token.instance_id() && lease_id == token.lease_id() => Ok(()),
            Ok(_) => Err(self.unexpected_result("release_lease")),
            Err(error) => Err(error),
        };
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientReleaseResult,
            ObservationOperation::ReleaseLease,
            ObservationThreadRole::Client,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            None,
            None,
            Some(token),
        );
        result
    }

    pub fn observe_readonly(&self, instance_alias: &str) -> RuntimeClientResult<RuntimeFlowOutput> {
        let correlation = self.issue_correlation("observe_readonly")?;
        let correlation_id = *correlation.transport();
        let receipt = self.execute_receipt_with_correlation(
            "observe_readonly",
            RuntimeOperation::ObserveReadonly {
                instance_alias: instance_alias.to_string(),
            },
            correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::ReadonlyObservationCompleted { .. })
        ) {
            return Err(self.unexpected_result("observe_readonly"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn capture_sequence(
        &self,
        instance_alias: &str,
        spec: CaptureSequenceSpec,
    ) -> RuntimeClientResult<RuntimeFlowOutput> {
        spec.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_capture_sequence_invalid", "capture_sequence")
        })?;
        let response_timeout = {
            let connection = self.connection("capture_sequence")?;
            capture_sequence_response_timeout(connection.backend_open_timeout, spec)?
        };
        let correlation = self.issue_correlation("capture_sequence")?;
        let correlation_id = *correlation.transport();
        let receipt = self.execute_receipt_with_correlation(
            "capture_sequence",
            RuntimeOperation::CaptureSequence {
                instance_alias: instance_alias.to_string(),
                spec,
            },
            correlation,
            Some(response_timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::CaptureSequenceCompleted { .. })
        ) {
            return Err(self.unexpected_result("capture_sequence"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn safe_reset(&self, instance_alias: &str) -> RuntimeClientResult<RuntimeFlowOutput> {
        let connection = self.connection("safe_reset")?;
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "safe_reset")
        })?;
        let holder = connection.ids.mint_holder_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "safe_reset")
        })?;
        let correlation_id = *correlation.transport();
        drop(connection);
        let receipt = self.execute_receipt_with_correlation(
            "safe_reset",
            RuntimeOperation::safe_reset(instance_alias, holder),
            correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::SafeResetCompleted { .. })
        ) {
            return Err(self.unexpected_result("safe_reset"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> RuntimeClientResult<RuntimeFlowOutput> {
        let connection = self.connection("application_lifecycle")?;
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "application_lifecycle")
        })?;
        let holder = connection.ids.mint_holder_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "application_lifecycle")
        })?;
        let correlation_id = *correlation.transport();
        drop(connection);
        let receipt = self.execute_receipt_with_correlation(
            "application_lifecycle",
            RuntimeOperation::application_lifecycle(instance_alias, holder, action),
            correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::ApplicationLifecycleCompleted {
                action: completed,
                ..
            }) if *completed == action
        ) {
            return Err(self.unexpected_result("application_lifecycle"));
        }
        self.flow_output(receipt, correlation_id)
    }

    pub fn run_contained_task(
        &self,
        instance_alias: &str,
        request: ContainedTaskRequest,
    ) -> RuntimeClientResult<RuntimeFlowOutput> {
        let mut connection = self.connection("run_contained_task")?;
        let response_timeout = contained_task_response_timeout(
            connection.io_timeout,
            Duration::from_millis(request.response_deadline_ms()),
        )?;
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "run_contained_task")
        })?;
        let holder = connection.ids.mint_holder_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "run_contained_task")
        })?;
        let correlation_id = *correlation.transport();
        let operation = RuntimeOperation::run_contained_task(instance_alias, holder, request);
        let runtime_request = connection.request_with_correlation(
            "run_contained_task",
            operation.clone(),
            correlation,
        )?;
        let receipt = match self.exchange_receipt(
            &mut connection,
            "run_contained_task",
            operation,
            runtime_request.clone(),
            Some(response_timeout),
        ) {
            Ok(receipt) => receipt,
            Err(error) if error.code() == "runtime_contained_task_response_timeout" => {
                drop(connection);
                let recovery =
                    self.recover_contained_task_timeout(instance_alias, &runtime_request);
                return Err(contained_task_recovery_outcome(error, recovery));
            }
            Err(error) => return Err(error),
        };
        match receipt.result() {
            Some(RuntimeResult::ContainedTaskCompleted { .. }) => {}
            Some(RuntimeResult::ContainedTaskCancelled { reason, .. }) => {
                drop(connection);
                let error = RuntimeClientError::fatal(
                    match reason {
                        ContainedTaskCancellationReason::DeadlineExceeded => {
                            "runtime_contained_task_response_timeout"
                        }
                        ContainedTaskCancellationReason::ClientRequested
                        | ContainedTaskCancellationReason::RecoveredAfterRestart => {
                            "runtime_contained_task_cancelled"
                        }
                    },
                    "run_contained_task",
                );
                let recovery = self.safe_reset(instance_alias).map(|_| ());
                return Err(contained_task_recovery_outcome(error, recovery));
            }
            _ => return Err(self.unexpected_result("run_contained_task")),
        }
        drop(connection);
        self.flow_output(receipt, correlation_id)
    }

    fn recover_contained_task_timeout(
        &self,
        instance_alias: &str,
        original: &RuntimeRequest,
    ) -> RuntimeClientResult<()> {
        {
            let mut connection = self.connection("recover_contained_task_timeout")?;
            connection.stream.shutdown(Shutdown::Both).map_err(|_| {
                connection.latch(RuntimeClientError::fatal(
                    "runtime_contained_task_cancel_failed",
                    "recover_contained_task_timeout",
                ))
            })?;
            let address = self.shared.info.socket_addr().map_err(|_| {
                connection.latch(RuntimeClientError::fatal(
                    "runtime_info_invalid",
                    "recover_contained_task_timeout",
                ))
            })?;
            let stream = connect_runtime_stream(
                address,
                connection.io_timeout,
                "recover_contained_task_timeout",
            )
            .map_err(|error| connection.latch(error))?;
            connection.stream = stream;
            connection.terminal_error = None;
        }

        let observed_epoch = self.health().map_err(|error| {
            RuntimeClientError::fatal(
                "runtime_contained_task_recovery_failed",
                "recover_contained_task_timeout",
            )
            .with_related(error)
        })?;
        if observed_epoch != self.shared.info.owner_epoch() {
            let error = RuntimeClientError::fatal(
                "runtime_owner_epoch_changed",
                "recover_contained_task_timeout",
            );
            return Err(self
                .connection("recover_contained_task_timeout")?
                .latch(error));
        }
        let recovery_timeout = {
            let connection = self.connection("recover_contained_task_timeout")?;
            connection.io_timeout
        };
        let recovery_deadline = std::time::Instant::now()
            .checked_add(recovery_timeout)
            .ok_or_else(|| {
                RuntimeClientError::fatal(
                    "runtime_contained_task_recovery_timeout_overflow",
                    "recover_contained_task_timeout",
                )
            })?;
        loop {
            let status = self.execute(
                "cancel_contained_task",
                RuntimeOperation::CancelContainedTask {
                    task_request_id: original.request_id(),
                },
            )?;
            let RuntimeResult::ContainedTaskCancellation {
                task_request_id,
                status,
            } = status
            else {
                return Err(self.unexpected_result("cancel_contained_task"));
            };
            if task_request_id != original.request_id() {
                return Err(RuntimeClientError::fatal(
                    "runtime_contained_task_cancellation_identity_mismatch",
                    "recover_contained_task_timeout",
                ));
            }
            match status {
                ContainedTaskCancellationStatus::Terminal { .. } => break,
                ContainedTaskCancellationStatus::RecoveryRequired { .. } => {
                    let mut connection = self.connection("recover_contained_task_timeout")?;
                    let response_timeout = connection.io_timeout;
                    let operation = original.operation().clone();
                    let receipt = self.exchange_receipt(
                        &mut connection,
                        "recover_contained_task_timeout",
                        operation,
                        original.clone(),
                        Some(response_timeout),
                    )?;
                    if !matches!(
                        receipt.result(),
                        Some(RuntimeResult::ContainedTaskCancelled { .. })
                            | Some(RuntimeResult::ContainedTaskCompleted { .. })
                    ) {
                        return Err(self.unexpected_result("recover_contained_task_timeout"));
                    }
                    break;
                }
                ContainedTaskCancellationStatus::Pending { .. } => {
                    if std::time::Instant::now() >= recovery_deadline {
                        return Err(RuntimeClientError::fatal(
                            "runtime_contained_task_busy",
                            "recover_contained_task_timeout",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        match self.safe_reset(instance_alias) {
            Ok(_) => Ok(()),
            Err(error) => {
                let error = RuntimeClientError::fatal(
                    "runtime_contained_task_recovery_failed",
                    "recover_contained_task_timeout",
                )
                .with_related(error);
                Err(self
                    .connection("recover_contained_task_timeout")?
                    .latch(error))
            }
        }
    }

    pub fn input(&self, token: &LeaseToken, action: InputAction) -> RuntimeClientResult<()> {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientInputStart,
            ObservationOperation::Input,
            ObservationThreadRole::Client,
            ObservationOutcome::Started,
            None,
            None,
            Some(token),
        );
        let result = (|| {
            let response_timeout = {
                let connection = self.connection("runtime_input")?;
                input_response_timeout(connection.io_timeout, &action)?
            };
            match self.execute_with_timeout(
                "runtime_input",
                RuntimeOperation::Input {
                    token: token.clone(),
                    action,
                },
                Some(response_timeout),
            ) {
                Ok(RuntimeResult::InputCommitted { .. }) => Ok(()),
                Ok(_) => Err(self.unexpected_result("runtime_input")),
                Err(error) => Err(error),
            }
        })();
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientInputResult,
            ObservationOperation::Input,
            ObservationThreadRole::Client,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            None,
            None,
            Some(token),
        );
        result
    }

    pub fn record_client_action(
        &self,
        action: ClientActionRecord,
    ) -> RuntimeClientResult<TerminalEvent> {
        action.validate().map_err(|_| {
            RuntimeClientError::fatal("client_action_invalid", "record_client_action")
        })?;
        let receipt = self.execute_receipt(
            "record_client_action",
            RuntimeOperation::RecordClientAction { action },
            None,
        )?;
        if !matches!(receipt.result(), Some(RuntimeResult::ClientActionRecorded)) {
            return Err(self.unexpected_result("record_client_action"));
        }
        receipt
            .terminal()
            .ok_or_else(|| self.unexpected_result("record_client_action"))
    }

    pub fn record_approval_decision(
        &self,
        decision: ApprovalDecisionRecord,
    ) -> RuntimeClientResult<TerminalEvent> {
        decision.validate().map_err(|_| {
            RuntimeClientError::fatal("approval_decision_invalid", "record_approval_decision")
        })?;
        let approval_id = decision.approval_id().to_owned();
        let disposition = decision.disposition();
        let receipt = self.execute_receipt(
            "record_approval_decision",
            RuntimeOperation::RecordApprovalDecision { decision },
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::ApprovalDecisionRecorded {
                approval_id: recorded_id,
                disposition: recorded_disposition,
            }) if recorded_id == &approval_id && *recorded_disposition == disposition
        ) {
            return Err(self.unexpected_result("record_approval_decision"));
        }
        receipt
            .terminal()
            .ok_or_else(|| self.unexpected_result("record_approval_decision"))
    }

    /// Authenticates this connection for governance writes without extending authority to peers.
    pub fn authenticate_governance(
        &self,
        capability: impl Into<String>,
    ) -> RuntimeClientResult<()> {
        match self.execute(
            "authenticate_governance",
            RuntimeOperation::AuthenticateGovernance {
                capability: capability.into(),
            },
        )? {
            RuntimeResult::GovernanceAuthenticated => Ok(()),
            _ => Err(self.unexpected_result("authenticate_governance")),
        }
    }

    pub fn start_agent_session(
        &self,
        wake_id: AgentWakeId,
    ) -> RuntimeClientResult<AgentSessionContext> {
        match self.execute(
            "start_agent_session",
            RuntimeOperation::StartAgentSession { wake_id },
        )? {
            RuntimeResult::AgentSessionOpened { context } => Ok(*context),
            _ => Err(self.unexpected_result("start_agent_session")),
        }
    }

    pub fn resume_agent_session(
        &self,
        session_id: AgentSessionId,
    ) -> RuntimeClientResult<AgentSessionContext> {
        match self.execute(
            "resume_agent_session",
            RuntimeOperation::ResumeAgentSession { session_id },
        )? {
            RuntimeResult::AgentSessionObserved { context } => Ok(*context),
            _ => Err(self.unexpected_result("resume_agent_session")),
        }
    }

    pub fn agent_session_status(
        &self,
        session_id: AgentSessionId,
    ) -> RuntimeClientResult<AgentSessionContext> {
        match self.execute(
            "agent_session_status",
            RuntimeOperation::AgentSessionStatus { session_id },
        )? {
            RuntimeResult::AgentSessionObserved { context } => Ok(*context),
            _ => Err(self.unexpected_result("agent_session_status")),
        }
    }

    pub fn record_agent_response(
        &self,
        response: AgentSessionResponse,
    ) -> RuntimeClientResult<AgentSessionStatus> {
        response.validate().map_err(|_| {
            RuntimeClientError::fatal("agent_response_invalid", "record_agent_response")
        })?;
        match self.execute(
            "record_agent_response",
            RuntimeOperation::RecordAgentResponse { response },
        )? {
            RuntimeResult::AgentResponseRecorded { status } => Ok(status),
            _ => Err(self.unexpected_result("record_agent_response")),
        }
    }

    pub fn prepare_strategic_report(
        &self,
        report: &StrategicReport,
        evidence: Vec<ProjectedArtifactReference>,
    ) -> RuntimeClientResult<RuntimeStrategicPlan> {
        report.validate().map_err(|_| {
            RuntimeClientError::fatal("strategic_report_invalid", "prepare_strategic_report")
        })?;
        let report = encode_policy_document(
            RuntimePlanningDocumentKind::StrategicReport,
            report,
            "prepare_strategic_report",
        )?;
        let request = RuntimeStrategicReportRequest::new(report, evidence).map_err(|_| {
            RuntimeClientError::fatal(
                "strategic_report_request_invalid",
                "prepare_strategic_report",
            )
        })?;
        match self.execute(
            "prepare_strategic_report",
            RuntimeOperation::PrepareStrategicReport {
                request: Box::new(request),
            },
        )? {
            RuntimeResult::StrategicPlanPrepared { plan } => {
                let (report, projection, proposal, preview) = plan.into_parts();
                let projection = self.decode_policy_document(
                    &projection,
                    RuntimePlanningDocumentKind::StrategicProjection,
                    "prepare_strategic_report",
                )?;
                Ok(RuntimeStrategicPlan {
                    report,
                    projection,
                    proposal,
                    preview,
                })
            }
            _ => Err(self.unexpected_result("prepare_strategic_report")),
        }
    }

    pub fn project_policy_input_identity(
        &self,
        as_of_ledger_position: u64,
    ) -> RuntimeClientResult<RuntimePolicyInputIdentity> {
        match self.execute(
            "project_policy_input_identity",
            RuntimeOperation::ProjectPolicyInputIdentity {
                as_of_ledger_position,
            },
        )? {
            RuntimeResult::PolicyInputIdentityProjected { identity } => Ok(identity),
            _ => Err(self.unexpected_result("project_policy_input_identity")),
        }
    }

    pub fn project_policy_forward(
        &self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        time: EvaluationTime,
        seed: u64,
        config: ForwardProjectionConfig,
    ) -> RuntimeClientResult<ForwardProjection> {
        config.validate().map_err(|_| {
            RuntimeClientError::fatal(
                "forward_projection_config_invalid",
                "project_policy_forward",
            )
        })?;
        let request = RuntimeForwardProjectionRequest::new(
            encode_policy_document(
                RuntimePlanningDocumentKind::EvaluationFacts,
                facts,
                "project_policy_forward",
            )?,
            encode_policy_document(
                RuntimePlanningDocumentKind::EvaluationResources,
                resources,
                "project_policy_forward",
            )?,
            encode_policy_document(
                RuntimePlanningDocumentKind::EvaluationTime,
                &time,
                "project_policy_forward",
            )?,
            seed,
            encode_policy_document(
                RuntimePlanningDocumentKind::ForwardProjectionConfig,
                &config,
                "project_policy_forward",
            )?,
        )
        .map_err(|_| {
            RuntimeClientError::fatal(
                "forward_projection_request_invalid",
                "project_policy_forward",
            )
        })?;
        let operation = RuntimeOperation::ProjectPolicyForward {
            request: Box::new(request),
        };
        match self.execute("project_policy_forward", operation)? {
            RuntimeResult::PolicyForwardProjected { projection } => self.decode_policy_document(
                &projection,
                RuntimePlanningDocumentKind::ForwardProjection,
                "project_policy_forward",
            ),
            _ => Err(self.unexpected_result("project_policy_forward")),
        }
    }

    pub fn assess_predictive_maintenance(
        &self,
        request: PredictiveMaintenanceRequest,
    ) -> RuntimeClientResult<MaintenanceAssessment> {
        match self.execute(
            "assess_predictive_maintenance",
            RuntimeOperation::AssessPredictiveMaintenance {
                query: Box::new(request.transport),
            },
        )? {
            RuntimeResult::PredictiveMaintenanceAssessed { assessment } => self
                .decode_policy_document(
                    &assessment,
                    RuntimePlanningDocumentKind::MaintenanceAssessmentV2,
                    "assess_predictive_maintenance",
                ),
            _ => Err(self.unexpected_result("assess_predictive_maintenance")),
        }
    }

    pub fn compile_proposal(
        &self,
        proposal: CatalogProposal,
    ) -> RuntimeClientResult<ProposalPreview> {
        proposal
            .validate()
            .map_err(|_| RuntimeClientError::fatal("proposal_invalid", "compile_proposal"))?;
        match self.execute(
            "compile_proposal",
            RuntimeOperation::CompileProposal {
                proposal: Box::new(proposal),
            },
        )? {
            RuntimeResult::ProposalEvaluated { preview } => Ok(preview),
            _ => Err(self.unexpected_result("compile_proposal")),
        }
    }

    pub fn promote_proposal(
        &self,
        proposal: CatalogProposal,
    ) -> RuntimeClientResult<ProposalPromotion> {
        proposal
            .validate()
            .map_err(|_| RuntimeClientError::fatal("proposal_invalid", "promote_proposal"))?;
        match self.execute(
            "promote_proposal",
            RuntimeOperation::PromoteProposal {
                proposal: Box::new(proposal),
            },
        )? {
            RuntimeResult::ProposalPromoted { promotion } => Ok(promotion),
            _ => Err(self.unexpected_result("promote_proposal")),
        }
    }

    pub fn query_events(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        let deadline = Instant::now()
            .checked_add(MAX_COMPLETE_EVENT_QUERY_DURATION)
            .ok_or_else(|| {
                RuntimeClientError::fatal(
                    "runtime_event_query_time_limit_invalid",
                    "query_runtime_events",
                )
            })?;
        let io_timeout = self.connection("query_runtime_events")?.io_timeout;
        let mut events = Vec::new();
        let mut event_ids = BTreeSet::<EventId>::new();
        let mut cursor = None;
        let mut snapshot_ledger_position = None;
        let mut last_sequence = 0_u64;
        for _ in 0..MAX_COMPLETE_EVENT_QUERY_PAGES {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    RuntimeClientError::fatal(
                        "runtime_event_query_time_limit_exceeded",
                        "query_runtime_events",
                    )
                })?;
            if remaining.is_zero() {
                return Err(RuntimeClientError::fatal(
                    "runtime_event_query_time_limit_exceeded",
                    "query_runtime_events",
                ));
            }
            let request =
                RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, cursor.clone())
                    .map_err(|_| {
                    RuntimeClientError::fatal(
                        "runtime_event_query_page_request_invalid",
                        "query_runtime_events",
                    )
                })?;
            let page = self.query_event_page_with_timeout(
                query.clone(),
                profile,
                request,
                Some(io_timeout.min(remaining)),
            )?;
            if Instant::now() >= deadline {
                return Err(RuntimeClientError::fatal(
                    "runtime_event_query_time_limit_exceeded",
                    "query_runtime_events",
                ));
            }
            match snapshot_ledger_position {
                Some(expected) if expected != page.snapshot_ledger_position() => {
                    return Err(RuntimeClientError::fatal(
                        "runtime_event_query_pagination_invalid",
                        "query_runtime_events",
                    ));
                }
                None => snapshot_ledger_position = Some(page.snapshot_ledger_position()),
                Some(_) => {}
            }
            for event in page.events() {
                if event.sequence <= last_sequence || !event_ids.insert(event.event_id) {
                    return Err(RuntimeClientError::fatal(
                        "runtime_event_query_pagination_invalid",
                        "query_runtime_events",
                    ));
                }
                if events.len() == MAX_COMPLETE_EVENT_QUERY_EVENTS {
                    return Err(RuntimeClientError::fatal(
                        "runtime_event_query_event_limit_exceeded",
                        "query_runtime_events",
                    ));
                }
                last_sequence = event.sequence;
                events.push(event.clone());
            }
            if !page.has_more() {
                return Ok(events);
            }
            if events.len() == MAX_COMPLETE_EVENT_QUERY_EVENTS {
                return Err(RuntimeClientError::fatal(
                    "runtime_event_query_event_limit_exceeded",
                    "query_runtime_events",
                ));
            }
            let next = page.next_cursor().cloned().ok_or_else(|| {
                RuntimeClientError::fatal(
                    "runtime_event_query_pagination_invalid",
                    "query_runtime_events",
                )
            })?;
            let previous_sequence = cursor.as_ref().map_or(0, |value| value.after_sequence());
            if next.after_sequence() != last_sequence || next.after_sequence() <= previous_sequence
            {
                return Err(RuntimeClientError::fatal(
                    "runtime_event_query_pagination_invalid",
                    "query_runtime_events",
                ));
            }
            cursor = Some(next);
        }
        Err(RuntimeClientError::fatal(
            "runtime_event_query_page_limit_exceeded",
            "query_runtime_events",
        ))
    }

    /// Projects one completed scheduler/policy/contained-task run from authoritative ledger links.
    pub fn summarize_run(&self, run_id: RunId) -> RuntimeClientResult<Value> {
        let events = self.query_complete_run_events(run_id)?;
        project_run_summary(run_id, &events)
    }

    fn query_complete_run_events(&self, run_id: RunId) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        let query = EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        };
        let mut events = Vec::new();
        let mut event_ids = BTreeSet::<EventId>::new();
        let mut cursor = None;
        let mut snapshot_ledger_position = None;
        let mut last_sequence = 0_u64;
        let mut resident_bytes = 0_usize;
        for _ in 0..MAX_RUN_SUMMARY_PAGES {
            let request =
                RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, cursor.clone())
                    .map_err(|_| {
                    RuntimeClientError::fatal("run_summary_page_request_invalid", "summarize_run")
                })?;
            let page =
                self.query_event_page(query.clone(), ProjectionProfile::Forensic, request)?;
            match snapshot_ledger_position {
                Some(expected) if expected != page.snapshot_ledger_position() => {
                    return Err(RuntimeClientError::fatal(
                        "run_summary_snapshot_changed",
                        "summarize_run",
                    ));
                }
                None => snapshot_ledger_position = Some(page.snapshot_ledger_position()),
                Some(_) => {}
            }
            for event in page.events() {
                if event.sequence <= last_sequence || !event_ids.insert(event.event_id) {
                    return Err(RuntimeClientError::fatal(
                        "run_summary_pagination_invalid",
                        "summarize_run",
                    ));
                }
                if events.len() == MAX_RUN_SUMMARY_EVENTS {
                    return Err(RuntimeClientError::fatal(
                        "run_summary_event_limit_exceeded",
                        "summarize_run",
                    ));
                }
                let event_bytes = serde_json::to_vec(event).map_err(|_| {
                    RuntimeClientError::fatal("run_summary_event_encode_failed", "summarize_run")
                })?;
                resident_bytes = resident_bytes
                    .checked_add(event_bytes.len())
                    .filter(|total| *total <= MAX_RUN_SUMMARY_RESIDENT_BYTES)
                    .ok_or_else(|| {
                        RuntimeClientError::fatal(
                            "run_summary_resident_limit_exceeded",
                            "summarize_run",
                        )
                    })?;
                last_sequence = event.sequence;
                events.push(event.clone());
            }
            if !page.has_more() {
                return Ok(events);
            }
            if events.len() == MAX_RUN_SUMMARY_EVENTS {
                return Err(RuntimeClientError::fatal(
                    "run_summary_event_limit_exceeded",
                    "summarize_run",
                ));
            }
            let next = page.next_cursor().cloned().ok_or_else(|| {
                RuntimeClientError::fatal("run_summary_pagination_invalid", "summarize_run")
            })?;
            let previous_sequence = cursor.as_ref().map_or(0, |value| value.after_sequence());
            if next.after_sequence() != last_sequence || next.after_sequence() <= previous_sequence
            {
                return Err(RuntimeClientError::fatal(
                    "run_summary_pagination_invalid",
                    "summarize_run",
                ));
            }
            cursor = Some(next);
        }
        Err(RuntimeClientError::fatal(
            "run_summary_page_limit_exceeded",
            "summarize_run",
        ))
    }

    pub fn query_event_page(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
        page: RuntimeEventQueryPageRequest,
    ) -> RuntimeClientResult<RuntimeEventQueryPage> {
        self.query_event_page_with_timeout(query, profile, page, None)
    }

    fn query_event_page_with_timeout(
        &self,
        query: EventQuery,
        profile: ProjectionProfile,
        page: RuntimeEventQueryPageRequest,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeEventQueryPage> {
        match self.execute_with_timeout(
            "query_runtime_events",
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            },
            response_timeout,
        )? {
            RuntimeResult::EventPage { page } => Ok(page),
            _ => Err(self.unexpected_result("query_runtime_events")),
        }
    }

    pub fn subscribe_events(
        &self,
        request: RuntimeSubscriptionRequest,
    ) -> RuntimeClientResult<RuntimeEventBatch> {
        request.validate().map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_subscription_request_invalid",
                "subscribe_runtime_events",
            )
        })?;
        let response_timeout = self
            .connection("subscribe_runtime_events")?
            .io_timeout
            .checked_add(Duration::from_millis(request.wait_ms()))
            .ok_or_else(|| {
                RuntimeClientError::fatal(
                    "runtime_subscription_timeout_invalid",
                    "subscribe_runtime_events",
                )
            })?;
        match self.execute_with_timeout(
            "subscribe_runtime_events",
            RuntimeOperation::SubscribeEvents { request },
            Some(response_timeout),
        )? {
            RuntimeResult::EventBatch { batch } => Ok(batch),
            _ => Err(self.unexpected_result("subscribe_runtime_events")),
        }
    }

    pub fn begin_authoring_session(&self) -> RuntimeClientResult<RuntimeAuthoringSession> {
        let connection = self.connection("begin_resource_authoring")?;
        if connection.actor != EventActor::Lab || connection.source != EventSource::Lab {
            return Err(RuntimeClientError::fatal(
                "runtime_authoring_origin_invalid",
                "begin_resource_authoring",
            ));
        }
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_identifier_issue_failed",
                "begin_resource_authoring",
            )
        })?;
        drop(connection);
        Ok(RuntimeAuthoringSession {
            client: self.clone(),
            correlation,
        })
    }

    pub fn begin_debug_session(&self) -> RuntimeClientResult<RuntimeDebugSession> {
        let connection = self.connection("begin_runtime_debug")?;
        if connection.actor != EventActor::Lab || connection.source != EventSource::Lab {
            return Err(RuntimeClientError::fatal(
                "runtime_debug_origin_invalid",
                "begin_runtime_debug",
            ));
        }
        let correlation = connection.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "begin_runtime_debug")
        })?;
        drop(connection);
        Ok(RuntimeDebugSession {
            client: self.clone(),
            correlation,
        })
    }

    fn execute(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
    ) -> RuntimeClientResult<RuntimeResult> {
        self.execute_with_timeout(operation_name, operation, None)
    }

    fn execute_with_timeout(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeResult> {
        let receipt = self.execute_receipt(operation_name, operation, response_timeout)?;
        let Some(result) = receipt.result().cloned() else {
            return Err(self.unexpected_result(operation_name));
        };
        Ok(result)
    }

    fn execute_receipt(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        let mut connection = self.connection(operation_name)?;
        let request = connection.request(operation_name, operation.clone())?;
        self.exchange_receipt(
            &mut connection,
            operation_name,
            operation,
            request,
            response_timeout,
        )
    }

    fn execute_receipt_with_correlation(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        correlation: IssuedCorrelationId,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        let mut connection = self.connection(operation_name)?;
        let request =
            connection.request_with_correlation(operation_name, operation.clone(), correlation)?;
        self.exchange_receipt(
            &mut connection,
            operation_name,
            operation,
            request,
            response_timeout,
        )
    }

    fn exchange_receipt(
        &self,
        connection: &mut RuntimeConnection,
        operation_name: &'static str,
        operation: RuntimeOperation,
        request: RuntimeRequest,
        response_timeout: Option<Duration>,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        let result = (|| {
            if let Some(error) = &connection.terminal_error {
                return Err(error.clone());
            }
            let response_timeout = response_timeout.unwrap_or_else(|| {
                receipt_response_timeout(
                    &operation,
                    connection.io_timeout,
                    connection.backend_open_timeout,
                )
            });
            let maximum_frame_bytes = connection.maximum_frame_bytes;
            let receipt_deadline = match &operation {
                RuntimeOperation::RunContainedTask { request, .. } => {
                    Some(ReceiptReadDeadline::after(
                        Duration::from_millis(request.response_deadline_ms()),
                        "runtime_receipt_timeout",
                    ))
                }
                _ => None,
            };
            if connection
                .stream
                .set_read_timeout(Some(response_timeout))
                .is_err()
            {
                return Err(connection.latch(RuntimeClientError::fatal(
                    "runtime_read_timeout_failed",
                    operation_name,
                )));
            }
            let exchange_result = exchange::<_, RuntimeReceipt>(
                &mut connection.stream,
                &request,
                maximum_frame_bytes,
                receipt_deadline,
                Some(&request),
            );
            if connection
                .stream
                .set_read_timeout(Some(connection.io_timeout))
                .is_err()
            {
                return Err(connection.latch(RuntimeClientError::fatal(
                    "runtime_read_timeout_restore_failed",
                    operation_name,
                )));
            }
            let receipt = match exchange_result {
                Ok(receipt) => receipt,
                Err(error)
                    if matches!(&operation, RuntimeOperation::RunContainedTask { .. })
                        && error.code() == "runtime_receipt_timeout" =>
                {
                    return Err(RuntimeClientError::fatal(
                        "runtime_contained_task_response_timeout",
                        operation_name,
                    )
                    .with_related(error));
                }
                Err(error) => return Err(connection.latch(error)),
            };
            if receipt.validate().is_err() {
                #[cfg(feature = "test-observation")]
                record_active(
                    ObservationStage::ReceiptValidationResult,
                    ObservationOperation::Other,
                    ObservationThreadRole::Client,
                    ObservationOutcome::Failure,
                    Some(&request),
                    Some(&receipt),
                    None,
                );
                return Err(connection.latch(RuntimeClientError::fatal(
                    "runtime_receipt_invalid",
                    operation_name,
                )));
            }
            #[cfg(feature = "test-observation")]
            record_active(
                ObservationStage::ReceiptValidationResult,
                ObservationOperation::Other,
                ObservationThreadRole::Client,
                ObservationOutcome::Success,
                Some(&request),
                Some(&receipt),
                None,
            );
            if receipt.request_id() != request.request_id()
                || receipt.correlation_id() != request.correlation_id()
            {
                return Err(connection.latch(RuntimeClientError::fatal(
                    "runtime_receipt_identity_mismatch",
                    operation_name,
                )));
            }
            if let Some(error) = receipt.error_projection() {
                let error = RuntimeClientError::rejected(operation_name, error.clone());
                return Err(if error.is_fatal() {
                    connection.latch(error)
                } else {
                    error
                });
            }
            if receipt.result().is_none() {
                return Err(connection.latch(RuntimeClientError::fatal(
                    "runtime_result_missing",
                    operation_name,
                )));
            }
            Ok(receipt)
        })();
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientTerminalResult,
            ObservationOperation::Other,
            ObservationThreadRole::Client,
            if result.is_ok() {
                ObservationOutcome::Success
            } else {
                ObservationOutcome::Failure
            },
            Some(&request),
            result.as_ref().ok(),
            None,
        );
        result
    }

    fn issue_correlation(
        &self,
        operation: &'static str,
    ) -> RuntimeClientResult<IssuedCorrelationId> {
        self.connection(operation)?
            .ids
            .mint_correlation_id()
            .map_err(|_| RuntimeClientError::fatal("runtime_identifier_issue_failed", operation))
    }

    fn decode_policy_document<T>(
        &self,
        document: &RuntimePlanningDocument,
        kind: RuntimePlanningDocumentKind,
        operation: &'static str,
    ) -> RuntimeClientResult<T>
    where
        T: DeserializeOwned,
    {
        document.decode(kind).map_err(|_| {
            let error =
                RuntimeClientError::fatal("runtime_planning_document_decode_failed", operation);
            match self.connection(operation) {
                Ok(mut connection) => connection.latch(error),
                Err(lock_error) => lock_error,
            }
        })
    }

    fn flow_output(
        &self,
        receipt: RuntimeReceipt,
        correlation_id: CorrelationId,
    ) -> RuntimeClientResult<RuntimeFlowOutput> {
        let events = self
            .query_events(
                EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .map_err(|error| {
                RuntimeClientError::after_commit(
                    "runtime_projection_failed_after_terminal",
                    "query_runtime_flow_projection",
                    receipt.clone(),
                    error,
                )
            })?;
        let official_ocr_projection = resolve_official_ocr_projection(
            &self.shared.state_root,
            &receipt,
            correlation_id,
            &events,
        )
        .map_err(|error| {
            RuntimeClientError::after_commit(
                "runtime_official_ocr_projection_failed_after_terminal",
                "project_runtime_official_ocr",
                receipt.clone(),
                error,
            )
        })?;
        Ok(RuntimeFlowOutput {
            receipt,
            events,
            official_ocr_projection,
        })
    }

    fn unexpected_result(&self, operation: &'static str) -> RuntimeClientError {
        let error = RuntimeClientError::fatal("runtime_result_unexpected", operation);
        match self.connection(operation) {
            Ok(mut connection) => connection.latch(error),
            Err(lock_error) => lock_error,
        }
    }

    fn connection(
        &self,
        operation: &'static str,
    ) -> RuntimeClientResult<MutexGuard<'_, RuntimeConnection>> {
        self.shared
            .connection
            .lock()
            .map_err(|_| RuntimeClientError::fatal("runtime_connection_poisoned", operation))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OcrObservationEnvelope {
    schema_version: String,
    task_id: TaskId,
    run_id: RunId,
    frame_id: FrameId,
    frame_index: u32,
    observation: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OcrComparisonEnvelope {
    schema_version: String,
    task_id: TaskId,
    run_id: RunId,
    final_frame_id: FrameId,
    report: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OcrObservationPayload {
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    confidence: Option<Value>,
    #[serde(default)]
    blocks: Option<Value>,
    #[serde(default)]
    execution: Option<OcrProviderEvidence>,
    #[serde(default)]
    targets: Option<Vec<OcrTargetObservationPayload>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OcrTargetObservationPayload {
    target_id: String,
    text: String,
    confidence: Option<Value>,
    blocks: Value,
    execution: OcrProviderEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct OcrProviderEvidence {
    invocation_id: String,
    session_id: String,
    session_generation: u64,
    requested_provider: RuntimeOfficialOcrProviderKind,
    resolved_provider: RuntimeOfficialOcrProviderKind,
    requested_cuda_ordinal: Option<u32>,
    requested_cuda_identity: Option<String>,
    resolved_cuda_ordinal: Option<u32>,
    resolved_cuda_identity: Option<String>,
    provider_implementation: String,
    provider_binary_sha256: String,
    runtime_version: String,
    model_ref: String,
    model_sha256: String,
    cpu_ep_registered: bool,
    cpu_fallback_disabled: bool,
    fallback_forbidden: bool,
    fallback_observed: Option<bool>,
    complete: bool,
}

pub(crate) fn resolve_official_ocr_projection(
    state_root: &Path,
    receipt: &RuntimeReceipt,
    correlation_id: CorrelationId,
    events: &[ProjectedEvent],
) -> RuntimeClientResult<Option<RuntimeOfficialOcrProjection>> {
    let Some(RuntimeResult::ContainedTaskCompleted {
        run_id,
        task_id,
        outcome: TaskOutcome::Success,
        ..
    }) = receipt.result()
    else {
        return Ok(None);
    };
    let marker_expected = official_ocr_marker_expected(events, run_id, task_id)?;
    let candidates = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::ArtifactCreated | EventType::ArtifactVerified
            )
        })
        .flat_map(|event| {
            event
                .artifacts
                .iter()
                .map(move |reference| (event, reference))
        })
        .filter(|(_, reference)| {
            reference.kind == ArtifactKind::DiagnosticJson
                && reference.producer == ArtifactProducer::CapturePipeline
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return if marker_expected {
            Err(official_ocr_error("runtime_official_ocr_evidence_missing"))
        } else {
            Ok(None)
        };
    }
    if candidates.len() > MAX_COMPLETE_EVENT_QUERY_EVENTS {
        return Err(official_ocr_limit_error());
    }

    let started = Instant::now();
    let canonical_root = fs::canonicalize(state_root)
        .map_err(|_| official_ocr_error("runtime_official_ocr_state_root_unavailable"))?;
    let mut total_bytes = 0_u64;
    let mut recognized_artifacts = 0_usize;
    let mut lifecycle = BTreeMap::new();
    let mut observations = Vec::new();
    let mut provider_evidence = Vec::new();
    let mut comparison = None;

    for (event, reference) in candidates {
        check_official_ocr_duration(started)?;
        reference
            .validate()
            .map_err(|_| official_ocr_error("runtime_official_ocr_artifact_invalid"))?;
        if event.links.task_id() != Some(task_id)
            || event.links.run_id() != Some(run_id)
            || event.links.correlation_id() != Some(&correlation_id)
            || event.links.frame_id() != reference.frame_id()
            || reference.run_id.as_ref() != Some(run_id)
            || reference.correlation_id.as_ref() != Some(&correlation_id)
            || reference.retention_class != RetentionClass::DebugFull
            || reference.redaction_state != ArtifactRedactionState::NotRequired
        {
            return Err(official_ocr_error(
                "runtime_official_ocr_artifact_identity_mismatch",
            ));
        }
        let entry = lifecycle
            .entry(reference.artifact_id)
            .or_insert((reference, None, None));
        if entry.0 != reference {
            return Err(official_ocr_error(
                "runtime_official_ocr_artifact_identity_conflict",
            ));
        }
        let sequence = match event.event_type {
            EventType::ArtifactCreated => &mut entry.1,
            EventType::ArtifactVerified => &mut entry.2,
            _ => unreachable!("candidate event type is filtered above"),
        };
        if sequence.replace((event.sequence, event.event_id)).is_some() {
            return Err(official_ocr_error(
                "runtime_official_ocr_artifact_duplicate",
            ));
        }
    }

    let logical_artifacts = lifecycle
        .into_values()
        .map(|(reference, created, verified)| {
            let (created_sequence, created_event_id) = created.ok_or_else(|| {
                official_ocr_error("runtime_official_ocr_artifact_lifecycle_incomplete")
            })?;
            let (verified_sequence, verified_event_id) = verified.ok_or_else(|| {
                official_ocr_error("runtime_official_ocr_artifact_lifecycle_incomplete")
            })?;
            if created_sequence >= verified_sequence {
                return Err(official_ocr_error(
                    "runtime_official_ocr_artifact_lifecycle_invalid",
                ));
            }
            Ok((
                reference,
                created_sequence,
                created_event_id,
                verified_sequence,
                verified_event_id,
            ))
        })
        .collect::<RuntimeClientResult<Vec<_>>>()?;
    for (reference, created_sequence, created_event_id, verified_sequence, verified_event_id) in
        logical_artifacts
    {
        check_official_ocr_duration(started)?;
        let object_key = reference
            .object_key()
            .ok_or_else(|| official_ocr_error("runtime_official_ocr_artifact_path_missing"))?;
        if reference.byte_count() > MAX_OFFICIAL_OCR_ARTIFACT_BYTES {
            return Err(official_ocr_limit_error());
        }
        total_bytes = total_bytes
            .checked_add(reference.byte_count())
            .ok_or_else(official_ocr_limit_error)?;
        if total_bytes > MAX_OFFICIAL_OCR_TOTAL_BYTES {
            return Err(official_ocr_limit_error());
        }
        let path = state_root.join(object_key);
        let canonical_path = fs::canonicalize(&path)
            .map_err(|_| official_ocr_error("runtime_official_ocr_artifact_read_failed"))?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(official_ocr_error(
                "runtime_official_ocr_artifact_path_invalid",
            ));
        }
        let metadata = fs::metadata(&canonical_path)
            .map_err(|_| official_ocr_error("runtime_official_ocr_artifact_read_failed"))?;
        if !metadata.is_file() || metadata.len() != reference.byte_count() {
            return Err(official_ocr_error(
                "runtime_official_ocr_artifact_length_mismatch",
            ));
        }
        let bytes = fs::read(&canonical_path)
            .map_err(|_| official_ocr_error("runtime_official_ocr_artifact_read_failed"))?;
        if canonical_sha256(&bytes) != reference.sha256() {
            return Err(official_ocr_error(
                "runtime_official_ocr_artifact_digest_mismatch",
            ));
        }
        let value = serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| official_ocr_error("runtime_official_ocr_payload_malformed"))?;
        match value.get("schema_version").and_then(Value::as_str) {
            Some(OCR_OBSERVATION_SCHEMA) => {
                recognized_artifacts = recognized_artifacts
                    .checked_add(1)
                    .ok_or_else(official_ocr_limit_error)?;
                let envelope = serde_json::from_value::<OcrObservationEnvelope>(value)
                    .map_err(|_| official_ocr_error("runtime_official_ocr_payload_malformed"))?;
                if envelope.schema_version != OCR_OBSERVATION_SCHEMA
                    || envelope.task_id != *task_id
                    || envelope.run_id != *run_id
                    || reference.frame_id() != Some(&envelope.frame_id)
                {
                    return Err(official_ocr_error(
                        "runtime_official_ocr_payload_identity_mismatch",
                    ));
                }
                let targets =
                    parse_official_ocr_observation(envelope.frame_index, &envelope.observation)?;
                let mut target_ids = targets
                    .iter()
                    .map(|(_, target_id, _)| target_id.clone())
                    .collect::<Vec<_>>();
                target_ids.sort();
                provider_evidence.extend(targets);
                observations.push(RuntimeOfficialOcrObservation {
                    frame_id: envelope.frame_id,
                    frame_index: envelope.frame_index,
                    artifact: reference.clone(),
                    artifact_created_event_id: created_event_id,
                    artifact_created_sequence: created_sequence,
                    artifact_verified_event_id: verified_event_id,
                    artifact_verified_sequence: verified_sequence,
                    target_ids,
                });
            }
            Some(OCR_COMPARISON_ENVELOPE_SCHEMA) => {
                recognized_artifacts = recognized_artifacts
                    .checked_add(1)
                    .ok_or_else(official_ocr_limit_error)?;
                let envelope = serde_json::from_value::<OcrComparisonEnvelope>(value)
                    .map_err(|_| official_ocr_error("runtime_official_ocr_payload_malformed"))?;
                if envelope.schema_version != OCR_COMPARISON_ENVELOPE_SCHEMA
                    || envelope.task_id != *task_id
                    || envelope.run_id != *run_id
                    || reference.frame_id() != Some(&envelope.final_frame_id)
                {
                    return Err(official_ocr_error(
                        "runtime_official_ocr_payload_identity_mismatch",
                    ));
                }
                if comparison
                    .replace((
                        reference.clone(),
                        created_sequence,
                        created_event_id,
                        verified_sequence,
                        verified_event_id,
                        envelope.report,
                    ))
                    .is_some()
                {
                    return Err(official_ocr_error(
                        "runtime_official_ocr_comparison_duplicate",
                    ));
                }
            }
            _ => {}
        }
    }
    check_official_ocr_duration(started)?;
    if recognized_artifacts == 0 {
        return if marker_expected {
            Err(official_ocr_error("runtime_official_ocr_evidence_missing"))
        } else {
            Ok(None)
        };
    }
    if recognized_artifacts > MAX_OFFICIAL_OCR_ARTIFACTS {
        return Err(official_ocr_limit_error());
    }
    let (
        comparison_artifact,
        comparison_artifact_created_sequence,
        comparison_artifact_created_event_id,
        comparison_artifact_verified_sequence,
        comparison_artifact_verified_event_id,
        comparison_value,
    ) = comparison.ok_or_else(|| official_ocr_error("runtime_official_ocr_comparison_missing"))?;
    if observations.is_empty() {
        return Err(official_ocr_error(
            "runtime_official_ocr_observation_missing",
        ));
    }
    observations.sort_by_key(|observation| observation.frame_index);
    for (expected, observation) in observations.iter().enumerate() {
        if observation.frame_index
            != u32::try_from(expected).map_err(|_| official_ocr_limit_error())?
        {
            return Err(official_ocr_error(
                "runtime_official_ocr_observation_order_invalid",
            ));
        }
    }
    let (summary, report) = parse_official_ocr_comparison(&comparison_value, &observations)?;
    let provider_execution = project_official_ocr_provider(provider_evidence)?;
    Ok(Some(RuntimeOfficialOcrProjection {
        schema_version: OFFICIAL_OCR_PROJECTION_SCHEMA,
        run_id: *run_id,
        task_id: *task_id,
        comparison_artifact,
        comparison_artifact_created_event_id,
        comparison_artifact_created_sequence,
        comparison_artifact_verified_event_id,
        comparison_artifact_verified_sequence,
        observations,
        summary,
        provider_execution,
        comparison: report,
    }))
}

fn official_ocr_marker_expected(
    events: &[ProjectedEvent],
    run_id: &RunId,
    task_id: &TaskId,
) -> RuntimeClientResult<bool> {
    let mut expected = false;
    for event in events.iter().filter(|event| {
        event.event_type == EventType::TaskCompleted
            && event.links.run_id() == Some(run_id)
            && event.links.task_id() == Some(task_id)
    }) {
        let EventPayload::Task(TaskPayload::Semantic(payload)) = full_payload(event)? else {
            return Err(official_ocr_error(
                "runtime_official_ocr_terminal_payload_invalid",
            ));
        };
        if let TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Success,
            scheduling_disposition: Some(disposition),
            ..
        } = payload.fact()
        {
            expected |= disposition.outcome_key() == "comparison_recorded";
        }
    }
    Ok(expected)
}

fn parse_official_ocr_observation(
    frame_index: u32,
    value: &Value,
) -> RuntimeClientResult<Vec<(u32, String, OcrProviderEvidence)>> {
    if frame_index >= MAX_OFFICIAL_OCR_FRAMES {
        return Err(official_ocr_limit_error());
    }
    let payload = serde_json::from_value::<OcrObservationPayload>(value.clone())
        .map_err(|_| official_ocr_error("runtime_official_ocr_observation_malformed"))?;
    let direct_shape = payload.target_id.is_some()
        || payload.text.is_some()
        || payload.confidence.is_some()
        || payload.blocks.is_some()
        || payload.execution.is_some();
    match (direct_shape, payload.targets) {
        (true, None) => {
            let target_id = payload
                .target_id
                .ok_or_else(|| official_ocr_error("runtime_official_ocr_observation_malformed"))?;
            let execution = payload.execution.ok_or_else(|| {
                official_ocr_error("runtime_official_ocr_provider_evidence_missing")
            })?;
            validate_official_ocr_target(&target_id, &execution)?;
            Ok(vec![(frame_index, target_id, execution)])
        }
        (false, Some(targets)) if !targets.is_empty() && targets.len() <= 32 => {
            let mut seen = BTreeSet::new();
            targets
                .into_iter()
                .map(|target| {
                    let _ = (&target.text, &target.confidence, &target.blocks);
                    validate_official_ocr_target(&target.target_id, &target.execution)?;
                    if !seen.insert(target.target_id.clone()) {
                        return Err(official_ocr_error("runtime_official_ocr_target_duplicate"));
                    }
                    Ok((frame_index, target.target_id, target.execution))
                })
                .collect()
        }
        _ => Err(official_ocr_error(
            "runtime_official_ocr_observation_malformed",
        )),
    }
}

fn validate_official_ocr_target(
    target_id: &str,
    evidence: &OcrProviderEvidence,
) -> RuntimeClientResult<()> {
    let provider_binding_valid = matches!(
        (
            evidence.requested_provider,
            evidence.resolved_provider,
            evidence.cpu_ep_registered,
            evidence.cpu_fallback_disabled,
        ),
        (
            RuntimeOfficialOcrProviderKind::Cpu,
            RuntimeOfficialOcrProviderKind::Cpu,
            true,
            false,
        ) | (
            RuntimeOfficialOcrProviderKind::Cuda,
            RuntimeOfficialOcrProviderKind::Cuda,
            false,
            true,
        )
    );
    if target_id.trim().is_empty()
        || evidence.invocation_id.trim().is_empty()
        || evidence.session_id.trim().is_empty()
        || evidence.provider_implementation.trim().is_empty()
        || evidence.runtime_version.trim().is_empty()
        || evidence.model_ref.trim().is_empty()
        || !is_sha256_hex(&evidence.provider_binary_sha256)
        || !is_sha256_hex(&evidence.model_sha256)
        || !provider_binding_valid
        || !evidence.complete
        || !evidence.fallback_forbidden
        || evidence.fallback_observed.is_some()
    {
        return Err(official_ocr_error(
            "runtime_official_ocr_provider_evidence_invalid",
        ));
    }
    Ok(())
}

fn parse_official_ocr_comparison(
    value: &Value,
    observations: &[RuntimeOfficialOcrObservation],
) -> RuntimeClientResult<(RuntimeOfficialOcrSummary, RuntimeOfficialOcrComparison)> {
    let report = serde_json::from_value::<RuntimeOfficialOcrComparison>(value.clone())
        .map_err(|_| official_ocr_error("runtime_official_ocr_comparison_malformed"))?;
    let schema_valid = match report.schema_version.as_str() {
        OCR_COMPARISON_SCHEMA_V1 => {
            report.mapping_evidence.is_none() && report.classification_contract.is_none()
        }
        OCR_COMPARISON_SCHEMA_V2 => {
            report.mapping_evidence.is_some() && report.classification_contract.is_some()
        }
        _ => false,
    };
    if !schema_valid
        || report.frames_collected == 0
        || report.frames_collected > MAX_OFFICIAL_OCR_FRAMES
        || usize::try_from(report.frames_collected).ok() != Some(observations.len())
        || report.observed.len() > MAX_OFFICIAL_OCR_ITEMS
        || report.truth.len() > MAX_OFFICIAL_OCR_ITEMS
        || report.missed.len() > MAX_OFFICIAL_OCR_ITEMS
        || report.unexpected.len() > MAX_OFFICIAL_OCR_ITEMS
        || report.duplicates.len() > MAX_OFFICIAL_OCR_ITEMS
        || report.outcome_key.trim().is_empty()
        || report.truth_set_path.trim().is_empty()
        || !is_sha256_hex(&report.truth_set_sha256)
    {
        return Err(official_ocr_error(
            "runtime_official_ocr_comparison_invalid",
        ));
    }
    let _ = (
        &report.target_id,
        &report.target_ids,
        report.items_collected,
        report.discarded_empty_items,
        report.total_observed_utf8_bytes,
        report.exact_match,
        report.normalization,
        report.comparison,
    );

    let mut canonical_names = Vec::with_capacity(report.observed.len());
    let mut canonical_occurrences = BTreeMap::new();
    for observed in &report.observed {
        if observed.value.trim().is_empty()
            || observed.occurrences == 0
            || usize::try_from(observed.occurrences).ok() != Some(observed.confidences.len())
            || canonical_names
                .last()
                .is_some_and(|prior| prior >= &observed.value)
        {
            return Err(official_ocr_error(
                "runtime_official_ocr_comparison_invalid",
            ));
        }
        canonical_names.push(observed.value.clone());
        canonical_occurrences.insert(observed.value.clone(), observed.occurrences);
    }
    let expected_duplicates = canonical_occurrences
        .iter()
        .filter(|(_, occurrences)| **occurrences > 1)
        .map(|(name, occurrences)| (name.clone(), *occurrences))
        .collect::<Vec<_>>();
    let duplicates = report
        .duplicates
        .iter()
        .map(|duplicate| (duplicate.value.clone(), duplicate.occurrences))
        .collect::<Vec<_>>();
    if duplicates != expected_duplicates {
        return Err(official_ocr_error(
            "runtime_official_ocr_comparison_invalid",
        ));
    }

    let mappings = report.mapping_evidence.as_deref().unwrap_or_default();
    if mappings.len() > MAX_OFFICIAL_OCR_MAPPING_FACTS {
        return Err(official_ocr_limit_error());
    }
    let mut unmatched_raw_readings = Vec::new();
    for mapping in mappings {
        if mapping.frame_index >= report.frames_collected
            || mapping.retry_index > 1
            || mapping.target_id.trim().is_empty()
            || mapping
                .canonical
                .as_ref()
                .is_some_and(|value| value.is_empty())
        {
            return Err(official_ocr_error(
                "runtime_official_ocr_mapping_evidence_invalid",
            ));
        }
        let _ = (
            mapping.confidence,
            mapping.candidate_count,
            &mapping.candidates,
        );
        if mapping.disposition == RuntimeOfficialOcrMappingDisposition::UnmatchedAfterRetry
            && mapping.canonical.is_none()
        {
            unmatched_raw_readings.push(RuntimeOfficialOcrUnmatchedReading {
                frame_index: mapping.frame_index,
                retry_index: mapping.retry_index,
                target_id: mapping.target_id.clone(),
                raw_text: mapping.raw_text.clone(),
                normalized_text: mapping.normalized_text.clone(),
                disposition: mapping.disposition,
            });
        }
    }

    let mut screen_coverage = Vec::with_capacity(observations.len());
    for observation in observations {
        screen_coverage.push(RuntimeOfficialOcrScreenCoverage {
            frame_index: observation.frame_index,
            target_ids: observation.target_ids.clone(),
        });
    }
    Ok((
        RuntimeOfficialOcrSummary {
            unique_canonical_count: canonical_names.len(),
            canonical_names,
            screen_coverage,
            duplicates: duplicates
                .into_iter()
                .map(|(name, occurrences)| RuntimeOfficialOcrDuplicate { name, occurrences })
                .collect(),
            unmatched_raw_readings,
        },
        report,
    ))
}

fn project_official_ocr_provider(
    mut records: Vec<(u32, String, OcrProviderEvidence)>,
) -> RuntimeClientResult<RuntimeOfficialOcrProviderExecution> {
    if records.is_empty() || records.len() > MAX_OFFICIAL_OCR_ITEMS {
        return Err(official_ocr_error(
            "runtime_official_ocr_provider_evidence_missing",
        ));
    }
    records.sort_by(|left, right| {
        (left.0, left.1.as_str(), left.2.invocation_id.as_str()).cmp(&(
            right.0,
            right.1.as_str(),
            right.2.invocation_id.as_str(),
        ))
    });
    let binding = records[0].2.clone();
    let mut invocation_ids = BTreeSet::new();
    let mut evidence = Vec::with_capacity(records.len());
    for (frame_index, target_id, record) in records {
        if !invocation_ids.insert(record.invocation_id.clone())
            || !same_official_ocr_provider_binding(&binding, &record)
        {
            return Err(official_ocr_error(
                "runtime_official_ocr_provider_evidence_mismatch",
            ));
        }
        evidence.push(RuntimeOfficialOcrProviderEvidence {
            frame_index,
            target_id,
            invocation_id: record.invocation_id,
        });
    }
    Ok(RuntimeOfficialOcrProviderExecution {
        requested_provider: binding.requested_provider,
        actual_provider: binding.resolved_provider,
        requested_cuda_ordinal: binding.requested_cuda_ordinal,
        requested_cuda_identity: binding.requested_cuda_identity,
        actual_cuda_ordinal: binding.resolved_cuda_ordinal,
        actual_cuda_identity: binding.resolved_cuda_identity,
        provider_implementation: binding.provider_implementation,
        provider_binary_sha256: binding.provider_binary_sha256,
        runtime_version: binding.runtime_version,
        model_ref: binding.model_ref,
        model_sha256: binding.model_sha256,
        cpu_ep_registered: binding.cpu_ep_registered,
        cpu_fallback_disabled: binding.cpu_fallback_disabled,
        fallback_forbidden: binding.fallback_forbidden,
        fallback_observed: binding.fallback_observed,
        strict_no_fallback: binding.fallback_forbidden && binding.fallback_observed.is_none(),
        complete: binding.complete,
        session_id: binding.session_id,
        session_generation: binding.session_generation,
        evidence,
    })
}

fn same_official_ocr_provider_binding(
    left: &OcrProviderEvidence,
    right: &OcrProviderEvidence,
) -> bool {
    left.session_id == right.session_id
        && left.session_generation == right.session_generation
        && left.requested_provider == right.requested_provider
        && left.resolved_provider == right.resolved_provider
        && left.requested_cuda_ordinal == right.requested_cuda_ordinal
        && left.requested_cuda_identity == right.requested_cuda_identity
        && left.resolved_cuda_ordinal == right.resolved_cuda_ordinal
        && left.resolved_cuda_identity == right.resolved_cuda_identity
        && left.provider_implementation == right.provider_implementation
        && left.provider_binary_sha256 == right.provider_binary_sha256
        && left.runtime_version == right.runtime_version
        && left.model_ref == right.model_ref
        && left.model_sha256 == right.model_sha256
        && left.cpu_ep_registered == right.cpu_ep_registered
        && left.cpu_fallback_disabled == right.cpu_fallback_disabled
        && left.fallback_forbidden == right.fallback_forbidden
        && left.fallback_observed == right.fallback_observed
        && left.complete == right.complete
}

pub(crate) fn canonical_sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn check_official_ocr_duration(started: Instant) -> RuntimeClientResult<()> {
    if started.elapsed() > MAX_OFFICIAL_OCR_PROJECTION_DURATION {
        Err(official_ocr_limit_error())
    } else {
        Ok(())
    }
}

fn official_ocr_error(code: &'static str) -> RuntimeClientError {
    RuntimeClientError::fatal(code, "project_runtime_official_ocr")
}

fn official_ocr_limit_error() -> RuntimeClientError {
    official_ocr_error("runtime_official_ocr_projection_limit_exceeded")
}

fn encode_policy_document<T>(
    kind: RuntimePlanningDocumentKind,
    value: &T,
    operation: &'static str,
) -> RuntimeClientResult<RuntimePlanningDocument>
where
    T: Serialize,
{
    RuntimePlanningDocument::encode(kind, value).map_err(|_| {
        RuntimeClientError::fatal("runtime_planning_document_encode_failed", operation)
    })
}

impl RuntimeProjectClient {
    pub fn connect(config: RuntimeClientConfig) -> RuntimeClientResult<Self> {
        RuntimeClient::connect(config).map(|client| Self { client })
    }

    pub fn runtime_info(&self) -> &RuntimeInfo {
        self.client.runtime_info()
    }

    pub fn status(&self) -> RuntimeClientResult<RuntimeControlPlaneStatus> {
        self.client.status()
    }

    pub fn snapshot(&self) -> RuntimeClientResult<ProjectLedgerSnapshot> {
        self.client
            .project_snapshot(ProjectInterfaceRequest::current())
    }

    pub fn snapshot_page(
        &self,
        limit: u16,
        cursor: Option<ProjectDecisionPageCursor>,
    ) -> RuntimeClientResult<ProjectLedgerSnapshot> {
        let page = ProjectDecisionPageRequest::new(limit, cursor).map_err(|_| {
            RuntimeClientError::fatal("runtime_project_page_invalid", "runtime_project_interface")
        })?;
        let request = ProjectInterfaceRequest::current()
            .with_decision_page(page)
            .map_err(|_| {
                RuntimeClientError::fatal(
                    "runtime_project_page_invalid",
                    "runtime_project_interface",
                )
            })?;
        self.client.project_snapshot(request)
    }

    pub fn snapshot_with_versions(
        &self,
        accepted_versions: Vec<String>,
    ) -> RuntimeClientResult<ProjectLedgerSnapshot> {
        let request = ProjectInterfaceRequest::new(accepted_versions).map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_project_versions_invalid",
                "runtime_project_interface",
            )
        })?;
        self.client.project_snapshot(request)
    }
}

impl RuntimeAuthoringSession {
    pub const fn correlation_id(&self) -> CorrelationId {
        *self.correlation.transport()
    }

    pub fn append(&self, event: ResourceAuthoringEvent) -> RuntimeClientResult<TerminalEvent> {
        event.validate().map_err(|_| {
            RuntimeClientError::fatal(
                "runtime_authoring_event_invalid",
                "record_resource_authoring_event",
            )
        })?;
        let expected_phase = event.phase();
        let receipt = self.client.execute_receipt_with_correlation(
            "record_resource_authoring_event",
            RuntimeOperation::RecordAuthoringEvent { event },
            self.correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::AuthoringEventRecorded { phase }) if *phase == expected_phase
        ) {
            return Err(self
                .client
                .unexpected_result("record_resource_authoring_event"));
        }
        receipt.terminal().ok_or_else(|| {
            self.client
                .unexpected_result("record_resource_authoring_event")
        })
    }

    pub fn query_events(
        &self,
        profile: ProjectionProfile,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        self.client.query_events(
            EventQuery {
                correlation_id: Some(self.correlation_id()),
                ..EventQuery::default()
            },
            profile,
        )
    }
}

impl RuntimeDebugSession {
    pub const fn correlation_id(&self) -> CorrelationId {
        *self.correlation.transport()
    }

    pub fn debug_package(
        &self,
        request: PackageDebugRequest,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        request.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_debug_package_invalid", "debug_package")
        })?;
        let timeout = self
            .client
            .connection("debug_package")?
            .backend_open_timeout;
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_package",
            RuntimeOperation::DebugPackage { request },
            self.correlation,
            Some(timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::PackageDebugCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("debug_package"));
        }
        Ok(receipt)
    }

    pub fn export_evidence(
        &self,
        request: RuntimeEvidenceExportRequest,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        request.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_evidence_export_invalid", "export_evidence")
        })?;
        let timeout = self
            .client
            .connection("export_evidence")?
            .backend_open_timeout;
        let receipt = self.client.execute_receipt_with_correlation(
            "export_evidence",
            RuntimeOperation::ExportEvidence { request },
            self.correlation,
            Some(timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::EvidenceExportCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("export_evidence"));
        }
        Ok(receipt)
    }

    pub fn observe_readonly(&self, instance_alias: &str) -> RuntimeClientResult<RuntimeReceipt> {
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_observe_readonly",
            RuntimeOperation::ObserveReadonly {
                instance_alias: instance_alias.to_string(),
            },
            self.correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::ReadonlyObservationCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("debug_observe_readonly"));
        }
        Ok(receipt)
    }

    pub fn capture_sequence(
        &self,
        instance_alias: &str,
        spec: CaptureSequenceSpec,
    ) -> RuntimeClientResult<RuntimeReceipt> {
        spec.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_capture_sequence_invalid", "debug_capture_sequence")
        })?;
        let response_timeout = {
            let connection = self.client.connection("debug_capture_sequence")?;
            capture_sequence_response_timeout(connection.backend_open_timeout, spec)?
        };
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_capture_sequence",
            RuntimeOperation::CaptureSequence {
                instance_alias: instance_alias.to_string(),
                spec,
            },
            self.correlation,
            Some(response_timeout),
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::CaptureSequenceCompleted { .. })
        ) {
            return Err(self.client.unexpected_result("debug_capture_sequence"));
        }
        Ok(receipt)
    }

    pub fn acquire_lease(&self, instance_alias: &str) -> RuntimeClientResult<LeaseToken> {
        let connection = self.client.connection("debug_acquire_lease")?;
        let holder = connection.ids.mint_holder_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", "debug_acquire_lease")
        })?;
        drop(connection);
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_acquire_lease",
            RuntimeOperation::acquire_lease(instance_alias, holder),
            self.correlation,
            None,
        )?;
        match receipt.result() {
            Some(RuntimeResult::LeaseGranted { token }) => Ok(token.clone()),
            _ => Err(self.client.unexpected_result("debug_acquire_lease")),
        }
    }

    pub fn renew_lease(&self, token: &LeaseToken) -> RuntimeClientResult<LeaseToken> {
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_renew_lease",
            RuntimeOperation::RenewLease {
                token: token.clone(),
            },
            self.correlation,
            None,
        )?;
        match receipt.result() {
            Some(RuntimeResult::LeaseRenewed { token }) => Ok(token.clone()),
            _ => Err(self.client.unexpected_result("debug_renew_lease")),
        }
    }

    pub fn input(&self, token: &LeaseToken, action: InputAction) -> RuntimeClientResult<ActionId> {
        let response_timeout = {
            let connection = self.client.connection("debug_runtime_input")?;
            input_response_timeout(connection.io_timeout, &action)?
        };
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_runtime_input",
            RuntimeOperation::Input {
                token: token.clone(),
                action,
            },
            self.correlation,
            Some(response_timeout),
        )?;
        match receipt.result() {
            Some(RuntimeResult::InputCommitted { action_id }) => Ok(*action_id),
            _ => Err(self.client.unexpected_result("debug_runtime_input")),
        }
    }

    pub fn release_lease(&self, token: &LeaseToken) -> RuntimeClientResult<()> {
        let receipt = self.client.execute_receipt_with_correlation(
            "debug_release_lease",
            RuntimeOperation::ReleaseLease {
                token: token.clone(),
            },
            self.correlation,
            None,
        )?;
        match receipt.result() {
            Some(RuntimeResult::LeaseReleased {
                instance_id,
                lease_id,
            }) if *instance_id == token.instance_id() && *lease_id == token.lease_id() => Ok(()),
            _ => Err(self.client.unexpected_result("debug_release_lease")),
        }
    }

    pub fn record_event(&self, event: RuntimeDebugEvent) -> RuntimeClientResult<TerminalEvent> {
        event.validate().map_err(|_| {
            RuntimeClientError::fatal("runtime_debug_event_invalid", "record_runtime_debug_event")
        })?;
        let expected_phase = event.phase();
        let receipt = self.client.execute_receipt_with_correlation(
            "record_runtime_debug_event",
            RuntimeOperation::RecordDebugEvent { event },
            self.correlation,
            None,
        )?;
        if !matches!(
            receipt.result(),
            Some(RuntimeResult::DebugEventRecorded { phase }) if *phase == expected_phase
        ) {
            return Err(self.client.unexpected_result("record_runtime_debug_event"));
        }
        receipt
            .terminal()
            .ok_or_else(|| self.client.unexpected_result("record_runtime_debug_event"))
    }

    pub fn query_events(
        &self,
        profile: ProjectionProfile,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        self.client.query_events(
            EventQuery {
                correlation_id: Some(self.correlation_id()),
                ..EventQuery::default()
            },
            profile,
        )
    }
}

impl fmt::Debug for RuntimeAuthoringSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeAuthoringSession")
            .field("correlation", &"<opaque-correlation>")
            .finish()
    }
}

impl fmt::Debug for RuntimeDebugSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeDebugSession")
            .field("correlation", &"<opaque-correlation>")
            .finish()
    }
}

impl fmt::Debug for RuntimeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeClient")
            .field("runtime_info", &"<validated-loopback-runtime>")
            .finish()
    }
}

impl RuntimeConnection {
    fn latch(&mut self, error: RuntimeClientError) -> RuntimeClientError {
        if self.terminal_error.is_none() {
            self.terminal_error = Some(error);
        }
        self.terminal_error
            .clone()
            .expect("terminal error was set above")
    }

    fn request(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
    ) -> RuntimeClientResult<RuntimeRequest> {
        let correlation = self.ids.mint_correlation_id().map_err(|_| {
            RuntimeClientError::fatal("runtime_identifier_issue_failed", operation_name)
        })?;
        self.request_with_correlation(operation_name, operation, correlation)
    }

    fn request_with_correlation(
        &self,
        operation_name: &'static str,
        operation: RuntimeOperation,
        correlation: IssuedCorrelationId,
    ) -> RuntimeClientResult<RuntimeRequest> {
        let request = RuntimeRequest::new(
            self.ids.mint_request_id().map_err(|_| {
                RuntimeClientError::fatal("runtime_identifier_issue_failed", operation_name)
            })?,
            correlation,
            None,
            self.actor,
            self.source,
            unix_ms_now()?,
            operation,
        )
        .map_err(|_| RuntimeClientError::fatal("runtime_request_invalid", operation_name))?;
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientRequestCreated,
            ObservationOperation::Other,
            ObservationThreadRole::Client,
            ObservationOutcome::Success,
            Some(&request),
            None,
            None,
        );
        Ok(request)
    }
}

fn project_run_summary(run_id: RunId, events: &[ProjectedEvent]) -> RuntimeClientResult<Value> {
    if events.is_empty() {
        return Err(RuntimeClientError::fatal(
            "run_summary_not_found",
            "summarize_run",
        ));
    }
    let intent_event = exactly_one_run_event(events, EventType::PolicyDispatchIntent)?;
    let admitted_event = exactly_one_run_event(events, EventType::PolicyDispatchAdmitted)?;
    let lease_event = exactly_one_run_event(events, EventType::LeaseGranted)?;
    let release_event = exactly_one_run_event(events, EventType::LeaseReleased)?;
    let execution_event = exactly_one_run_event(events, EventType::PolicyExecutionRecorded)?;
    let completed_event = exactly_one_run_event(events, EventType::PolicyDispatchCompleted)?;
    let execution = match full_payload(execution_event)? {
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload)) => payload,
        _ => {
            return Err(RuntimeClientError::fatal(
                "run_summary_payload_mismatch",
                "summarize_run",
            ));
        }
    };
    let task_id = intent_event.links.task_id().copied().ok_or_else(|| {
        RuntimeClientError::fatal("run_summary_identity_missing", "summarize_run")
    })?;
    let correlation_id = intent_event
        .links
        .correlation_id()
        .copied()
        .ok_or_else(|| {
            RuntimeClientError::fatal("run_summary_identity_missing", "summarize_run")
        })?;
    if events.iter().any(|event| {
        event.links.run_id() != Some(&run_id)
            || event.links.task_id() != Some(&task_id)
            || event.links.correlation_id() != Some(&correlation_id)
    }) {
        return Err(RuntimeClientError::fatal(
            "run_summary_identity_mismatch",
            "summarize_run",
        ));
    }
    let lease_id = lease_event.links.lease_id().copied().ok_or_else(|| {
        RuntimeClientError::fatal("run_summary_identity_missing", "summarize_run")
    })?;
    for event in [release_event, execution_event, completed_event] {
        if event.links.lease_id() != Some(&lease_id) {
            return Err(RuntimeClientError::fatal(
                "run_summary_lease_mismatch",
                "summarize_run",
            ));
        }
    }
    let intent = policy_dispatch_payload(intent_event, EventType::PolicyDispatchIntent)?;
    let admitted = policy_dispatch_payload(admitted_event, EventType::PolicyDispatchAdmitted)?;
    let completed = policy_dispatch_payload(completed_event, EventType::PolicyDispatchCompleted)?;
    if intent.decision_id() != admitted.decision_id()
        || intent.decision_id() != completed.decision_id()
        || intent.decision_id() != execution.decision_id()
        || intent.admission().is_some()
        || admitted.admission().is_none()
        || completed.admission() != admitted.admission()
    {
        return Err(RuntimeClientError::fatal(
            "run_summary_policy_mismatch",
            "summarize_run",
        ));
    }
    if is_policy_settlement_interrupted(execution.outcome()) {
        if release_event.sequence <= lease_event.sequence
            || events.iter().any(|event| {
                matches!(
                    event.event_type,
                    EventType::LabRequest
                        | EventType::TaskRequested
                        | EventType::TaskCompleted
                        | EventType::TaskFailed
                        | EventType::TaskCancelled
                        | EventType::TaskEffectIntent
                        | EventType::TaskEffectCompleted
                        | EventType::InputIntent
                        | EventType::InputCommitted
                        | EventType::InputFailed
                )
            })
        {
            return Err(RuntimeClientError::fatal(
                "run_summary_settlement_interrupted_invalid",
                "summarize_run",
            ));
        }
        return Ok(json!({
            "schema_version": "actingcommand.run-summary.v1",
            "status": "policy_settlement_interrupted",
            "run_id": run_id,
            "task_id": task_id,
            "correlation_id": correlation_id,
            "decision_id": intent.decision_id(),
            "operation_id": intent.operation_id(),
            "instance_id": intent.instance_id(),
            "package_digest": intent.package_digest(),
            "procedure_binding_digest": intent.procedure_binding_digest(),
            "reason_chain": {
                "id": intent.reason_chain_id(),
                "reasons": intent.reasons()
            },
            "catalog": {
                "hash": intent.catalog_hash(),
                "version": intent.catalog_version()
            },
            "admission": admitted.admission(),
            "lease": {
                "lease_id": lease_id,
                "grant_sequence": lease_event.sequence,
                "release_sequence": release_event.sequence
            },
            "outcome": {
                "kind": "policy_settlement_interrupted",
                "policy": execution.outcome(),
                "result": "original_cause_unavailable",
                "original_cause": "unavailable"
            },
            "execution_provenance": {
                "kind": "settlement_recovery",
                "device_access": false,
                "account_access": false,
                "production_input": false,
                "actual_effect_count": 0,
                "simulated_effect_count": 0
            },
            "effect": "not_performed",
            "actual_effect_count": 0,
            "simulated_effect_count": 0,
            "event_count": events.len(),
            "completed_sequence": completed_event.sequence
        }));
    }
    let (task_event_type, status, result) = match execution.outcome() {
        PolicyExecutionOutcome::Succeeded { .. } => {
            (EventType::TaskCompleted, "simulated_completed", None)
        }
        PolicyExecutionOutcome::Failed { .. } => {
            (EventType::TaskFailed, "simulated_failed", Some("failed"))
        }
    };
    let task_event = exactly_one_run_event(events, task_event_type)?;
    if task_event.links.lease_id() != Some(&lease_id) {
        return Err(RuntimeClientError::fatal(
            "run_summary_lease_mismatch",
            "summarize_run",
        ));
    }
    let lab_request_event = exactly_one_run_event(events, EventType::LabRequest)?;
    let lab_request_id = lab_request_event
        .links
        .request_id()
        .copied()
        .ok_or_else(|| {
            RuntimeClientError::fatal("run_summary_identity_missing", "summarize_run")
        })?;
    let receipt_request_id = task_event.links.request_id().copied().ok_or_else(|| {
        RuntimeClientError::fatal("run_summary_identity_missing", "summarize_run")
    })?;
    if lab_request_id != receipt_request_id {
        return Err(RuntimeClientError::fatal(
            "run_summary_receipt_request_mismatch",
            "summarize_run",
        ));
    }
    validate_task_terminal(task_event, execution.outcome())?;
    validate_admitted_package(events, &lab_request_id, intent.package_digest())?;
    let simulated_effect_count = validate_fixture_simulation(events)?;
    let result = result.unwrap_or(if simulated_effect_count == 0 {
        "no_op"
    } else {
        "would_effect"
    });
    Ok(json!({
        "schema_version": "actingcommand.run-summary.v1",
        "status": status,
        "run_id": run_id,
        "task_id": task_id,
        "correlation_id": correlation_id,
        "decision_id": intent.decision_id(),
        "operation_id": intent.operation_id(),
        "instance_id": intent.instance_id(),
        "package_digest": intent.package_digest(),
        "procedure_binding_digest": intent.procedure_binding_digest(),
        "reason_chain": {
            "id": intent.reason_chain_id(),
            "reasons": intent.reasons()
        },
        "catalog": {
            "hash": intent.catalog_hash(),
            "version": intent.catalog_version()
        },
        "admission": admitted.admission(),
        "lease": {
            "lease_id": lease_id,
            "grant_sequence": lease_event.sequence,
            "release_sequence": release_event.sequence
        },
        "request": {
            "lab_request_id": lab_request_id,
            "receipt_request_id": receipt_request_id,
            "terminal_event_id": task_event.event_id,
            "terminal_sequence": task_event.sequence
        },
        "outcome": {
            "kind": "fixture_simulation",
            "policy": execution.outcome(),
            "result": result
        },
        "execution_provenance": {
            "kind": "fixture_simulation",
            "device_access": false,
            "account_access": false,
            "production_input": false,
            "actual_effect_count": 0,
            "simulated_effect_count": simulated_effect_count
        },
        "effect": result,
        "actual_effect_count": 0,
        "simulated_effect_count": simulated_effect_count,
        "event_count": events.len(),
        "completed_sequence": completed_event.sequence
    }))
}

fn validate_task_terminal(
    task_event: &ProjectedEvent,
    execution: &PolicyExecutionOutcome,
) -> RuntimeClientResult<()> {
    let EventPayload::Task(TaskPayload::Semantic(payload)) = full_payload(task_event)? else {
        return Err(RuntimeClientError::fatal(
            "run_summary_payload_mismatch",
            "summarize_run",
        ));
    };
    let TaskSemanticFact::TerminalCommitted {
        outcome,
        failure_code,
        ..
    } = payload.fact()
    else {
        return Err(RuntimeClientError::fatal(
            "run_summary_payload_mismatch",
            "summarize_run",
        ));
    };
    let matches = match execution {
        PolicyExecutionOutcome::Succeeded { .. } => {
            *outcome == actingcommand_contract::TaskOutcome::Success && failure_code.is_none()
        }
        PolicyExecutionOutcome::Failed { failure } => {
            *outcome == actingcommand_contract::TaskOutcome::Failure
                && failure_code.as_deref() == Some(failure.error_code.as_str())
        }
    };
    if !matches {
        return Err(RuntimeClientError::fatal(
            "run_summary_policy_mismatch",
            "summarize_run",
        ));
    }
    Ok(())
}

fn is_policy_settlement_interrupted(execution: &PolicyExecutionOutcome) -> bool {
    matches!(
        execution,
        PolicyExecutionOutcome::Failed { failure }
            if failure.error_code == "policy_settlement_interrupted"
                && failure.original_class == PolicyFailureClass::Severe
                && failure.effective_class == PolicyFailureClass::Severe
                && failure.disposition == PolicyFailureDisposition::PausedTask
                && failure.retry_attempt == 0
                && failure.retry_at_unix_ms.is_none()
                && !failure.reported_success
                && failure.runtime_ms == 0
    )
}

fn validate_admitted_package(
    events: &[ProjectedEvent],
    request_id: &RequestId,
    policy_package_digest: &str,
) -> RuntimeClientResult<()> {
    let mut admitted_package_sha256 = Vec::new();
    for event in events
        .iter()
        .filter(|event| event.event_type == EventType::TaskRequested)
    {
        let EventPayload::Task(TaskPayload::Semantic(payload)) = full_payload(event)? else {
            continue;
        };
        let TaskSemanticFact::PackageAdmitted { package_sha256, .. } = payload.fact() else {
            continue;
        };
        if event.links.request_id() != Some(request_id) {
            return Err(RuntimeClientError::fatal(
                "run_summary_package_request_mismatch",
                "summarize_run",
            ));
        }
        admitted_package_sha256.push(package_sha256.as_str());
    }
    let [admitted_package_sha256] = admitted_package_sha256.as_slice() else {
        return Err(RuntimeClientError::fatal(
            "run_summary_package_fact_count_invalid",
            "summarize_run",
        ));
    };
    let policy_package_sha256 = policy_package_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| {
            RuntimeClientError::fatal("run_summary_package_digest_invalid", "summarize_run")
        })?;
    if policy_package_sha256 != *admitted_package_sha256 {
        return Err(RuntimeClientError::fatal(
            "run_summary_package_digest_mismatch",
            "summarize_run",
        ));
    }
    Ok(())
}

fn validate_fixture_simulation(events: &[ProjectedEvent]) -> RuntimeClientResult<usize> {
    if events
        .iter()
        .any(|event| event.origin.source() == EventSource::Device)
    {
        return Err(RuntimeClientError::fatal(
            "run_summary_device_event_forbidden",
            "summarize_run",
        ));
    }
    let simulation_origin = |event: &ProjectedEvent| {
        event.origin.source() == EventSource::Lab
            && event.origin.module() == OriginModule::Actinglab
    };
    let capture_events = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::CaptureRequested
                    | EventType::CaptureCompleted
                    | EventType::CaptureFailed
            )
        })
        .collect::<Vec<_>>();
    if !capture_events
        .iter()
        .any(|event| event.event_type == EventType::CaptureCompleted)
        || capture_events.iter().any(|event| !simulation_origin(event))
    {
        return Err(RuntimeClientError::fatal(
            "run_summary_simulation_provenance_invalid",
            "summarize_run",
        ));
    }
    let input_events = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::InputIntent | EventType::InputCommitted | EventType::InputFailed
            )
        })
        .collect::<Vec<_>>();
    if input_events.iter().any(|event| !simulation_origin(event))
        || input_events
            .iter()
            .any(|event| event.event_type == EventType::InputFailed)
    {
        return Err(RuntimeClientError::fatal(
            "run_summary_simulation_provenance_invalid",
            "summarize_run",
        ));
    }
    let input_intent_count = input_events
        .iter()
        .filter(|event| event.event_type == EventType::InputIntent)
        .count();
    let committed_inputs = input_events
        .iter()
        .filter(|event| event.event_type == EventType::InputCommitted)
        .map(|event| match full_payload(event)? {
            EventPayload::Input(InputPayload::Committed(payload))
                if payload.effect_disposition() == EffectDisposition::NotPerformed =>
            {
                Ok(())
            }
            _ => Err(RuntimeClientError::fatal(
                "run_summary_simulation_effect_invalid",
                "summarize_run",
            )),
        })
        .collect::<RuntimeClientResult<Vec<_>>>()?
        .len();
    let simulated_effect_count = events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEffectCompleted)
        .count();
    if input_intent_count != simulated_effect_count || committed_inputs != simulated_effect_count {
        return Err(RuntimeClientError::fatal(
            "run_summary_simulation_effect_invalid",
            "summarize_run",
        ));
    }
    Ok(simulated_effect_count)
}

fn exactly_one_run_event(
    events: &[ProjectedEvent],
    event_type: EventType,
) -> RuntimeClientResult<&ProjectedEvent> {
    let mut matching = events.iter().filter(|event| event.event_type == event_type);
    let event = matching.next().ok_or_else(|| {
        RuntimeClientError::fatal("run_summary_event_count_invalid", "summarize_run")
    })?;
    if matching.next().is_some() {
        return Err(RuntimeClientError::fatal(
            "run_summary_event_count_invalid",
            "summarize_run",
        ));
    }
    Ok(event)
}

fn full_payload(event: &ProjectedEvent) -> RuntimeClientResult<&EventPayload> {
    match &event.payload {
        ProjectionPayload::Full(payload) => Ok(payload.as_ref()),
        _ => Err(RuntimeClientError::fatal(
            "run_summary_projection_incomplete",
            "summarize_run",
        )),
    }
}

fn policy_dispatch_payload(
    event: &ProjectedEvent,
    event_type: EventType,
) -> RuntimeClientResult<&actingcommand_contract::PolicyDispatchPayload> {
    let payload = full_payload(event)?;
    match (event_type, payload) {
        (
            EventType::PolicyDispatchIntent,
            EventPayload::Policy(PolicyPayload::DispatchIntent(payload)),
        )
        | (
            EventType::PolicyDispatchAdmitted,
            EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)),
        )
        | (
            EventType::PolicyDispatchCompleted,
            EventPayload::Policy(PolicyPayload::DispatchCompleted(payload)),
        ) => Ok(payload),
        _ => Err(RuntimeClientError::fatal(
            "run_summary_payload_mismatch",
            "summarize_run",
        )),
    }
}

fn read_runtime_info(state_root: &Path) -> RuntimeClientResult<RuntimeInfo> {
    let path = state_root.join(RUNTIME_INFO_FILE);
    let metadata = fs::metadata(&path)
        .map_err(|_| RuntimeClientError::fatal("runtime_info_unavailable", "discover_runtime"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RUNTIME_INFO_BYTES {
        return Err(RuntimeClientError::fatal(
            "runtime_info_invalid",
            "discover_runtime",
        ));
    }
    let bytes = fs::read(path)
        .map_err(|_| RuntimeClientError::fatal("runtime_info_read_failed", "discover_runtime"))?;
    let info = serde_json::from_slice::<RuntimeInfo>(&bytes)
        .map_err(|_| RuntimeClientError::fatal("runtime_info_decode_failed", "discover_runtime"))?;
    info.validate()
        .map_err(|_| RuntimeClientError::fatal("runtime_info_invalid", "discover_runtime"))?;
    Ok(info)
}

fn unix_ms_now() -> RuntimeClientResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RuntimeClientError::fatal("runtime_clock_invalid", "create_request"))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| RuntimeClientError::fatal("runtime_clock_overflow", "create_request"))
}

fn connect_runtime_stream(
    address: std::net::SocketAddr,
    io_timeout: Duration,
    operation: &'static str,
) -> RuntimeClientResult<TcpStream> {
    let stream = TcpStream::connect_timeout(&address, io_timeout)
        .map_err(|_| RuntimeClientError::fatal("runtime_connect_failed", operation))?;
    stream
        .set_read_timeout(Some(io_timeout))
        .map_err(|_| RuntimeClientError::fatal("runtime_read_timeout_failed", operation))?;
    stream
        .set_write_timeout(Some(io_timeout))
        .map_err(|_| RuntimeClientError::fatal("runtime_write_timeout_failed", operation))?;
    stream
        .set_nodelay(true)
        .map_err(|_| RuntimeClientError::fatal("runtime_tcp_nodelay_failed", operation))?;
    Ok(stream)
}

pub(super) fn receipt_response_timeout(
    operation: &RuntimeOperation,
    io_timeout: Duration,
    backend_open_timeout: Duration,
) -> Duration {
    match operation {
        RuntimeOperation::AcquireLease { .. }
        | RuntimeOperation::ObserveReadonly { .. }
        | RuntimeOperation::SafeReset { .. } => backend_open_timeout,
        _ => io_timeout,
    }
}

fn contained_task_response_timeout(
    io_timeout: Duration,
    configured_task_timeout: Duration,
) -> RuntimeClientResult<Duration> {
    configured_task_timeout
        .checked_add(io_timeout)
        .ok_or_else(|| {
            RuntimeClientError::fatal(
                "runtime_contained_task_timeout_overflow",
                "run_contained_task",
            )
        })
}

fn contained_task_recovery_outcome(
    primary: RuntimeClientError,
    recovery: RuntimeClientResult<()>,
) -> RuntimeClientError {
    match recovery {
        Ok(()) => primary,
        Err(recovery)
            if recovery.code() == "runtime_contained_task_busy"
                || recovery.projection().is_some_and(|projection| {
                    matches!(
                        projection.code,
                        RuntimeErrorCode::ContainedTaskBusy | RuntimeErrorCode::LeaseBusy
                    )
                }) =>
        {
            recovery.with_related(primary)
        }
        Err(recovery) => primary.with_related(recovery),
    }
}

fn input_response_timeout(
    io_timeout: Duration,
    action: &InputAction,
) -> RuntimeClientResult<Duration> {
    let duration_ms = match action {
        InputAction::LongTap { duration_ms, .. } | InputAction::Swipe { duration_ms, .. } => {
            *duration_ms
        }
        InputAction::SingleTouchDragWithVerticalBrakeV1 {
            horizontal_duration_ms,
            corner_hold_ms,
            brake_duration_ms,
            ..
        } => horizontal_duration_ms
            .checked_add(*corner_hold_ms)
            .and_then(|value| value.checked_add(*brake_duration_ms))
            .ok_or_else(|| {
                RuntimeClientError::fatal("runtime_input_timeout_overflow", "runtime_input")
            })?,
        _ => 0,
    };
    io_timeout
        .checked_add(Duration::from_millis(duration_ms))
        .ok_or_else(|| RuntimeClientError::fatal("runtime_input_timeout_overflow", "runtime_input"))
}

fn capture_sequence_response_timeout(
    backend_open_timeout: Duration,
    spec: CaptureSequenceSpec,
) -> RuntimeClientResult<Duration> {
    let planned_wait_ms = spec.planned_wait_ms().map_err(|_| {
        RuntimeClientError::fatal("runtime_capture_sequence_invalid", "capture_sequence")
    })?;
    backend_open_timeout
        .checked_add(Duration::from_millis(planned_wait_ms))
        .ok_or_else(|| {
            RuntimeClientError::fatal(
                "runtime_capture_sequence_timeout_overflow",
                "capture_sequence",
            )
        })
}

#[cfg(test)]
mod run_summary_package_tests {
    use super::validate_admitted_package;
    use actingcommand_contract::{
        AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventSeverity,
        EventSource, IdentifierIssuer, IssuedRequestId, OriginModule, ProjectedEvent,
        ProjectionPayload, SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
        TaskPayloadDraft, TaskSemanticFact,
    };

    struct RejectSecrets;

    impl SecretFingerprinter for RejectSecrets {
        fn fingerprint(
            &self,
            _field: SecretField,
            _original: &str,
        ) -> Result<Sha256Fingerprint, SanitizationError> {
            panic!("package admission facts do not contain secrets")
        }
    }

    fn digest(byte: char) -> (String, String) {
        let raw = byte.to_string().repeat(64);
        let canonical = format!("sha256:{raw}");
        (raw, canonical)
    }

    fn package_event(
        issuer: &IdentifierIssuer,
        request_id: IssuedRequestId,
        package_sha256: String,
        sequence: u64,
    ) -> ProjectedEvent {
        let sanitized = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            sequence,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default().with_request_id(request_id),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::PackageAdmitted {
                    package_label: "package".to_string(),
                    task_label: "task".to_string(),
                    package_sha256,
                    response_deadline_monotonic_ms: Some(60_000),
                },
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(&RejectSecrets)
        .expect("sanitize package admission");
        ProjectedEvent {
            schema_version: sanitized.schema_version().to_string(),
            sequence,
            event_id: *sanitized.event_id(),
            timestamp_unix_ms: sanitized.timestamp_unix_ms(),
            event_type: sanitized.event_type(),
            severity: sanitized.severity(),
            sensitivity: sanitized.sensitivity(),
            origin: sanitized.origin().clone(),
            links: sanitized.links().clone(),
            payload_schema: sanitized.payload_schema().to_string(),
            payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn exactly_one_matching_package_admission_is_required() {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let request_id = issuer.mint_request_id().expect("request id");
        let (raw, canonical) = digest('a');
        let events = [package_event(&issuer, request_id, raw, 1)];
        validate_admitted_package(&events, request_id.transport(), &canonical)
            .expect("matching admitted package");
    }

    #[test]
    fn missing_package_admission_fails_closed() {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let request_id = issuer.mint_request_id().expect("request id");
        let (_, canonical) = digest('a');
        let error = validate_admitted_package(&[], request_id.transport(), &canonical)
            .expect_err("missing package admission");
        assert_eq!(error.code(), "run_summary_package_fact_count_invalid");
    }

    #[test]
    fn duplicate_package_admission_fails_closed() {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let request_id = issuer.mint_request_id().expect("request id");
        let (raw, canonical) = digest('a');
        let events = [
            package_event(&issuer, request_id, raw.clone(), 1),
            package_event(&issuer, request_id, raw, 2),
        ];
        let error = validate_admitted_package(&events, request_id.transport(), &canonical)
            .expect_err("duplicate package admission");
        assert_eq!(error.code(), "run_summary_package_fact_count_invalid");
    }

    #[test]
    fn mismatched_package_admission_fails_closed() {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let request_id = issuer.mint_request_id().expect("request id");
        let (_, canonical) = digest('a');
        let (other, _) = digest('b');
        let events = [package_event(&issuer, request_id, other, 1)];
        let error = validate_admitted_package(&events, request_id.transport(), &canonical)
            .expect_err("mismatched package admission");
        assert_eq!(error.code(), "run_summary_package_digest_mismatch");
    }
}

#[cfg(test)]
mod run_summary_settlement_tests {
    use super::{is_policy_settlement_interrupted, project_run_summary};
    use actingcommand_contract::{
        AuditInput, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
        EventOrigin, EventPayloadDraft, EventSeverity, EventSource, IdentifierIssuer,
        LeasePayloadDraft, OriginModule, PerformanceContext, PolicyActivitySample,
        PolicyAdmissionRecord, PolicyBudgetReceipt, PolicyDispatchEventData,
        PolicyExecutionEventData, PolicyExecutionOutcome, PolicyFailureClass,
        PolicyFailureDisposition, PolicyFailureRecord, PolicyPayloadDraft, PolicyReasonRecord,
        ProjectedEvent, ProjectionPayload, RunId, SanitizationError, SecretField,
        SecretFingerprinter, Sha256Fingerprint,
    };

    struct RejectSecrets;

    impl SecretFingerprinter for RejectSecrets {
        fn fingerprint(
            &self,
            _field: SecretField,
            _original: &str,
        ) -> Result<Sha256Fingerprint, SanitizationError> {
            panic!("settlement fixture does not contain secrets")
        }
    }

    fn projected(
        issuer: &IdentifierIssuer,
        sequence: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> ProjectedEvent {
        let sanitized = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            sequence,
            severity,
            origin,
            links,
            payload,
        )
        .sanitize(&RejectSecrets)
        .expect("sanitize settlement fixture");
        ProjectedEvent {
            schema_version: sanitized.schema_version().to_owned(),
            sequence,
            event_id: *sanitized.event_id(),
            timestamp_unix_ms: sanitized.timestamp_unix_ms(),
            event_type: sanitized.event_type(),
            severity: sanitized.severity(),
            sensitivity: sanitized.sensitivity(),
            origin: sanitized.origin().clone(),
            links: sanitized.links().clone(),
            payload_schema: sanitized.payload_schema().to_owned(),
            payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
            artifacts: Vec::new(),
        }
    }

    fn settlement_fixture() -> (RunId, Vec<ProjectedEvent>) {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instance_id = issuer.mint_instance_id().expect("instance id");
        let request_id = issuer.mint_request_id().expect("request id");
        let correlation_id = issuer.mint_correlation_id().expect("correlation id");
        let task_id = issuer.mint_task_id().expect("task id");
        let run_id = issuer.mint_run_id().expect("run id");
        let lease_id = issuer.mint_lease_id().expect("lease id");
        let dispatch = PolicyDispatchEventData {
            decision_id: "decision:settlement".to_owned(),
            task_id: "task:settlement".to_owned(),
            instance_id: "instance:settlement".to_owned(),
            operation_id: "operation:settlement".to_owned(),
            package_digest: format!("sha256:{}", "a".repeat(64)),
            procedure_binding_digest: format!("sha256:{}", "b".repeat(64)),
            reason_chain_id: "reason:settlement".to_owned(),
            reasons: vec![PolicyReasonRecord {
                code: "scheduled".to_owned(),
                detail: "deterministic settlement fixture".to_owned(),
            }],
            catalog_hash: format!("sha256:{}", "c".repeat(64)),
            catalog_version: 1,
            input_ledger_position: 1,
            fact_snapshot_id: "snapshot:settlement".to_owned(),
            approval_fact_ids: Vec::new(),
            urgency_milli: 100,
        };
        let admission = PolicyAdmissionRecord {
            activity: PolicyActivitySample {
                profile_id: "profile:settlement".to_owned(),
                local_day: 1,
                window_id: "window:settlement".to_owned(),
                admitted_at_unix_ms: 1,
                seed: 7,
                interval_ms: 1_000,
                next_eligible_unix_ms: 1_001,
            },
            budget: PolicyBudgetReceipt {
                task_daily_used: 1,
                task_daily_limit: 1,
                task_window_used: 1,
                task_window_limit: 1,
                task_runtime_reserved_ms: 1,
                task_runtime_limit_ms: 1,
                activity_daily_used: 1,
                activity_daily_limit: 1,
                activity_window_used: 1,
                activity_window_limit: 1,
                activity_runtime_reserved_ms: 1,
                activity_runtime_limit_ms: 1,
            },
        };
        let failure = PolicyFailureRecord {
            error_code: "policy_settlement_interrupted".to_owned(),
            reported_success: false,
            original_class: PolicyFailureClass::Severe,
            effective_class: PolicyFailureClass::Severe,
            consecutive_same_error: 1,
            escalation_streak: 1,
            performance_tax_exempt: false,
            retry_attempt: 0,
            disposition: PolicyFailureDisposition::PausedTask,
            retry_at_unix_ms: None,
            runtime_ms: 0,
            sensitive: false,
            perf_context: Box::new(PerformanceContext::unavailable(4)),
        };
        let execution = PolicyExecutionEventData {
            decision_id: dispatch.decision_id.clone(),
            task_id: dispatch.task_id.clone(),
            instance_id: dispatch.instance_id.clone(),
            observed_at_unix_ms: 4,
            outcome: PolicyExecutionOutcome::Failed { failure },
        };
        let policy_links = EventLinksDraft::default()
            .with_instance_id(instance_id)
            .with_request_id(request_id)
            .with_correlation_id(correlation_id)
            .with_task_id(task_id)
            .with_run_id(run_id)
            .with_action_id(issuer.mint_action_id().expect("intent action"));
        let lease_links = policy_links
            .clone()
            .with_lease_id(lease_id)
            .with_action_id(issuer.mint_action_id().expect("lease action"));
        let scheduler_origin = EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
        );
        let events = vec![
            projected(
                &issuer,
                1,
                EventSeverity::Info,
                scheduler_origin.clone(),
                policy_links.clone(),
                PolicyPayloadDraft::dispatch_intent(dispatch.clone(), AuditInput::new()).into(),
            ),
            projected(
                &issuer,
                2,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Scheduler,
                    OriginModule::Scheduler,
                    EventActor::Scheduler,
                ),
                lease_links.clone(),
                LeasePayloadDraft::granted(
                    EventAction::LeaseAcquire,
                    EffectDisposition::Performed,
                    AuditInput::new(),
                )
                .into(),
            ),
            projected(
                &issuer,
                3,
                EventSeverity::Info,
                scheduler_origin.clone(),
                policy_links,
                PolicyPayloadDraft::dispatch_admitted(
                    dispatch.clone(),
                    admission.clone(),
                    AuditInput::new(),
                )
                .into(),
            ),
            projected(
                &issuer,
                4,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Scheduler,
                    OriginModule::Scheduler,
                    EventActor::Scheduler,
                ),
                lease_links.clone(),
                LeasePayloadDraft::released(
                    EventAction::LeaseRelease,
                    EffectDisposition::Performed,
                    AuditInput::new(),
                )
                .into(),
            ),
            projected(
                &issuer,
                5,
                EventSeverity::Error,
                scheduler_origin.clone(),
                lease_links.clone(),
                PolicyPayloadDraft::execution_recorded(execution, AuditInput::new()).into(),
            ),
            projected(
                &issuer,
                6,
                EventSeverity::Info,
                scheduler_origin,
                lease_links,
                PolicyPayloadDraft::dispatch_completed(dispatch, admission, AuditInput::new())
                    .into(),
            ),
        ];
        (*run_id.transport(), events)
    }

    #[test]
    fn severe_settlement_interruption_has_a_typed_summary_without_task_replay() {
        let (run_id, events) = settlement_fixture();

        let summary = project_run_summary(run_id, &events).expect("settlement summary");

        assert_eq!(summary["status"], "policy_settlement_interrupted");
        assert_eq!(summary["outcome"]["kind"], "policy_settlement_interrupted");
        assert_eq!(summary["outcome"]["result"], "original_cause_unavailable");
        assert_eq!(summary["outcome"]["original_cause"], "unavailable");
        assert_eq!(summary["actual_effect_count"], 0);
        assert_eq!(summary["simulated_effect_count"], 0);
    }

    #[test]
    fn only_the_exact_severe_interruption_shape_enters_the_settlement_lane() {
        let failure = PolicyFailureRecord {
            error_code: "policy_settlement_interrupted".to_owned(),
            reported_success: false,
            original_class: PolicyFailureClass::Recoverable,
            effective_class: PolicyFailureClass::Recoverable,
            consecutive_same_error: 1,
            escalation_streak: 1,
            performance_tax_exempt: false,
            retry_attempt: 0,
            disposition: PolicyFailureDisposition::Continue,
            retry_at_unix_ms: None,
            runtime_ms: 0,
            sensitive: false,
            perf_context: Box::new(PerformanceContext::unavailable(1)),
        };
        assert!(!is_policy_settlement_interrupted(
            &PolicyExecutionOutcome::Failed { failure }
        ));
    }
}
