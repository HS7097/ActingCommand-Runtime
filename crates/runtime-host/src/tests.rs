// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ipc::{DEFAULT_RUNTIME_MAX_FRAME_BYTES, FrameRead, read_frame, write_frame};
use crate::monitor::MONITOR_FILE_NAME;
use crate::time::unix_ms_now;
use actingcommand_artifact_store::{ArtifactStore, read_projected_verified};
use actingcommand_contract::ArtifactProducer;
use actingcommand_contract::{
    AgentAttentionState, AgentPayload, AgentResponseDisposition, AgentSessionId,
    AgentSessionResponse, AgentWakeKind, ApplicationLifecycleAction, ApprovalDecisionRecord,
    ApprovalDisposition, ApprovalTarget, ArtifactKind, AuthoritativeSchedulingOutcome,
    CapturePayload, CaptureSequenceSpec, CatalogDeclarationPatch, CatalogPayload, CatalogProposal,
    ClientActionKind, ClientActionRecord, ClientActionValue, ContainedTaskRecoveryBinding,
    ContainedTaskRequest, CorrelationId, EffectDisposition, EventActor, EventPayload, EventQuery,
    EventSeverity, EventSource, EventType, FactContent, FactRecord, FactScope, FactTtlPolicy,
    FactTtlSource, FactValue as ContractFactValue, INPUT_EXECUTION_PLAN_PROFILE_MAA_2_0,
    INPUT_EXECUTION_PLAN_VERSION, IdentifierIssuer, InputAction, InputExecutionPlanEvent,
    InputPayload, InputSamplingAlgorithm, InstanceFactContext, InstanceId, IssuedCorrelationId,
    LeaseId, LeasePriority, LeaseQueuePolicy, LeaseQueueStatus, LeaseToken,
    MAX_RUNTIME_PLANNING_DOCUMENT_BYTES, MonitorDiagnosis, MonitorDisposition, MonitorObservation,
    MonitorPayload, MonitorRecoveryCoordinationReason, MonitorRecoveryKind, OriginModule,
    PerformanceControlLevel, PerformanceMonitorHealth, PinnedFrameReason, PolicyExecutionOutcome,
    PolicyFailureClass, PolicyFailureDisposition, PolicyPayload, PolicyPlanningSignalEventData,
    PolicyPlanningSignalKind, ProjectDecisionPageRequest, ProjectDecisionState,
    ProjectInterfaceRequest, ProjectLedgerSnapshot, ProjectedArtifactReference, ProjectedEvent,
    ProjectionPayload, ProjectionProfile, ProposalClass, ProposalDisposition, ProposalDocument,
    ProposalKind, ProposalPatchOperation, PublicEventPayload, RUNTIME_INFO_FILE, ReleasePayload,
    ReleaseResourceVersion, ReleaseTransitionKind, ResourceAuthoringEvent, ResourceAuthoringPhase,
    RunId, RuntimeCaptureBackend, RuntimeErrorCode, RuntimeEventQueryCursor,
    RuntimeEventQueryPageRequest, RuntimeForwardProjectionRequest, RuntimeMonitorPolicy,
    RuntimeOperation, RuntimePlanningDocument, RuntimePlanningDocumentKind, RuntimeReceipt,
    RuntimeReceiptState, RuntimeReleaseSet, RuntimeRequest, RuntimeResult,
    RuntimeStrategicReportRequest, SchedulingDisposition, SchedulingEffectEvidence,
    SchedulingOutcomeDeclaration, SchedulingOutcomeIdentity, Sensitivity, StatePayload,
    StateRecoveryAction, StateValidationResult, TaskEntryRecognitionPhase,
    TaskEntryTargetDisposition, TaskId, TaskOutcome, TaskPayload, TaskSemanticFact,
    TaskTemplateInstantiation, TerminalEvent,
};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceErrorCategory, DeviceErrorSensitivity,
    DeviceResult, Frame, InputBackend, PixelFormat, PreparedSegmentedSwipePlan,
    SegmentedSwipeEvent,
};
use actingcommand_execution_kernel::{
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTrace, ExecutionBackendProvenance,
    ExecutionKernel, ExternalExpectedSha256, PreparedContainedTask,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, PersistedEvent};
use actingcommand_policy::{
    ActivityDocument, CatalogDocumentSource, CatalogSources, CohortBudgets, Comparison,
    DecisionReasonChain, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FactValue, ForwardProjectionConfig, HostResourceSnapshot, InstanceSnapshot, LoadProfile,
    MaintenanceDisposition, MaintenanceTrendPolicy, MetricRef, ObservedFact, ObservedOutcome,
    OutlierMetric, OutlierPolicy, PoolValueSnapshot, PredicateSpec, ScopeSelector, StrategicBand,
    StrategicEvidencePointer, StrategicGoal, StrategicInstanceAssessment, StrategicReport,
    StrategicTemplate, TasksDocument, compile_catalog,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult, VisionProvider,
    VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_runtime_state::{
    RUNTIME_STATE_DATABASE_FILE, ReleaseArtifactSources, RuntimeStateStore,
};
use actingcommand_scheduler::{
    ConnectionId, DEFAULT_LEASE_TTL_MS, DEFAULT_MAX_CLIENT_HEARTBEAT_INTERVAL_MS, SchedulerConfig,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

// Shared fixture fragments retain one private test scope.
include!("tests/support/backend.rs");
include!("tests/support/mapped_runs.rs");
include!("tests/support/packages.rs");
include!("tests/support/planning.rs");
include!("tests/support/policy.rs");
include!("tests/support/runtime.rs");
include!("tests/support/vision.rs");

mod agent_sessions;
mod approvals;
mod capture_failure;
mod capture_sequence;
mod client_events;
mod contained_tasks;
mod crash_recovery;
mod detection;
mod facts;
mod forward_projection;
mod home_entry;
mod input;
mod input_failure;
mod lease_fencing;
mod lease_queue;
mod lifecycle;
mod maintenance;
mod mapped_outcomes;
mod mapped_recovery;
mod mapped_validation;
mod monitor;
mod ocr_diagnostics;
mod ocr_fields;
mod online_observation;
mod planning;
mod policy_admission;
mod policy_budget;
mod policy_completion;
mod project_interface;
mod proposals;
mod releases;
mod sampling;
mod scheduled_execution;
mod scheduled_retry;
mod signatures;
mod stability;
mod strategy;
mod task_deadlines;
mod task_replay;
mod teardown;
mod vision;
