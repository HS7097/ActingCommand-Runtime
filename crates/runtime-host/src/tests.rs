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
    ExecutionKernel, InputFrameContext, ObservedFrame, PreparedContainedTask,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
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

// Workflow #381 B (HOST-I1): the scripted emulator controller shared with execution-kernel and
// actingctl's gate binary.
#[allow(dead_code)]
#[path = "../../../tests/support/emulator_controller.rs"]
mod emulator_controller;

// Shared fixture fragments retain one private test scope.
include!("tests/support/backend.rs");
include!("tests/support/startup.rs");
include!("tests/support/mapped_runs.rs");
include!("tests/support/packages.rs");
include!("tests/support/planning.rs");
include!("tests/support/policy.rs");
include!("tests/support/runtime.rs");
include!("tests/support/vision.rs");

mod agent_sessions;
mod approvals;
mod capture_failure;
mod capture_floor;
mod capture_sequence;
mod client_events;
mod clock_slot_report;
mod contained_tasks;
mod crash_recovery;
mod detection;
mod facts;
mod forward_projection;
mod gate_369s5;
mod gate_372k2;
// Workflow #373 NAV-3b, test rows N3b-3, N3b-4, N3b-5, N3b-7 and N3b-8.
mod gate_373_nav3b;
// Workflow #373 NAV-6: shared interrupt pages and the standard-pack root reader.
mod gate_373_nav6;
mod gate_375_r3;
mod gate_381a;
mod gate_381b;
// Workflow #383 AO-7, test row AO7-b.
mod gate_383;
// Workflow #383 AO-4q, test row AO4-c: status reads append no ledger record.
mod gate_383_ao4q;
// Workflow #383 SH-1, register §1.18: the reserved single-touch path drag is refused.
mod gate_383_sh1;
// Workflow #387 D1b: D1-M3 to D1-M7.
mod gate_387_d1b;
// Workflow #387 D1c: the ladder's host rows (H-4 G6, G7; D1-L2, D1-L3, D1-L4).
mod gate_387_d1c;
mod gate_392_daa;
mod gate_398_rd1a;
mod gate_398_rd1b;
// Workflow #403: an unconfirmed close of one instance's resources fences only that instance.
mod gate_403;
mod gate_epoch_close;
mod gate_p0;
mod home_entry;
mod input;
mod input_failure;
mod instance_workers;
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
mod package_pins;
mod planning;
mod policy_admission;
mod policy_budget;
mod policy_completion;
mod project_interface;
mod proposals;
// Workflow #378 A4: receipt v2 outcomes and the class detail.
mod receipt_outcome;
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
