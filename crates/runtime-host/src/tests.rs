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
// Workflow #411 A3 (R-1, v0.12.1): a routine entry the pause gate holds does not refuse a
// shutdown.
mod gate_0121_a3;
// #411 A7 (v0.12.1), register §1.98: a failed capture open in a preparation reaches the ladder.
mod gate_0121_a7;
// #411 A7 task path (v0.12.2, host key 108), register §1.106: a contained task's capture failure
// on one instance fails only that run and goes to the ladder; the Runtime goes on.
mod gate_0122_a7t;
// #385 P-k1b (v0.12.x, host key 110), register §1.107: the host hands each step its instance's
// pacing factor, keeps its own deadline on the wall clock and settles on budget time.
mod gate_0122_pk1b;
// #411 A15 (v0.12.1-rc.2, host key 106): an unconfirmed step's terminal and receipt name the
// page of its last frame and that frame.
mod gate_rc2_a15;
// Workflow #308 SEL-L3b, register §1.58: the drag with a pause and a brake through the host.
mod gate_308_l3b;
// Workflow #308 SEL-L4b, gate key host 77: a scan select ends its run on_empty through the host.
mod gate_308_l4b;
mod gate_336_pkgs2;
// Workflow #336 PKG-S3: bindings without a package digest, failure identity v2 and the lift of
// a pause by a package's content hash.
mod gate_336_pkgs3;
// Workflow #336 PKG-S4: a configured prerequisite package is the package its `package_id` names.
mod gate_336_pkgs4;
mod gate_369s5;
// Workflow #369 S6c: the configured emulator start at daemon start, one claim per instance.
mod gate_369_s6c;
// Workflow #369 S6c review low (b): a failed resume or self-check preparation with no ladder
// releases the queued startup claim.
mod gate_369_s6clow;
mod gate_372k2;
// Workflow #372 K4: the occurrences lost to a `missed` grace or to a later occurrence.
mod gate_372k4;
// Workflow #372 K5: accounting follows the active catalog's task identity (`continues_from`).
mod gate_372k5;
// Workflow #373 NAV-3a: page-graph prerequisite packages and the standard-pack registry.
mod gate_373_nav3a;
// Workflow #373 NAV-3b, test rows N3b-3, N3b-4, N3b-5, N3b-7 and N3b-8.
mod gate_373_nav3b;
// Workflow #373 NAV-5: the entry gate's full-match routing (host key 97).
mod gate_373_nav5;
// Workflow #373 NAV-6: shared interrupt pages and the standard-pack root reader.
mod gate_373_nav6;
mod gate_375_r3;
mod gate_381a;
mod gate_381b;
// Workflow #382 H-1-rest (test plan v2.2 §3.10): the install transition's deadlines on the
// manual Runtime clock (HOST-I2), G1 and G3.
mod gate_382_h1;
// Workflow #382 H-4 (test plan v2.2 §3.6): the recovery ladder's G8 and G9 rows.
mod gate_382_h4;
// Workflow #382 H-5 (test plan v2.2 §3.6): host wave B.
mod gate_382_h5;
// Workflow #378 A6a: the latch rule, `ledger_opened` and `owner_released`.
mod gate_378_a6a;
// Workflow #378 SHUT-FU: the install CommitShutdown lets lifecycle work in flight end first.
mod gate_378_shutfu;
// Workflow #383 AO-7, test row AO7-b.
mod gate_383;
// Workflow #383 AO-1, the acceptance row of additions §8.3: a forced write failure's dump is
// imported once by the restart and deleted.
mod gate_383_ao1;
// Workflow #383 AO-4q, test row AO4-c: status reads append no ledger record.
mod gate_383_ao4q;
// Workflow #383 SH-1, register §1.18: the reserved single-touch path drag is refused.
mod gate_383_sh1;
// Workflow #384 D2-S2: reading marks, recognition package bindings and the reader warnings.
mod gate_384_d2s2;
// Workflow #384 D2-S3: resource adjustments.
mod gate_384_d2s3;
// Workflow #384 D2-CHAIN and D2-S4: the fact refresh end to end, and the dispatch's reader check.
mod gate_384_d2s4;
// Workflow #384 D2-S5 and D2-S4's T31, T32: the transition recorder, its read, the stale intent
// and the ledger the fact refresh leaves alone.
mod gate_384_d2s5;
// Workflow #385 P-m: the shadow measurement and its status read.
mod gate_385_pm;
// Workflow #385 PM-CFG: the monitor, its sampler and the controller read the monitor's row.
mod gate_385_pmcfg;
// Workflow #386 SR-2a, test row T6d: a start reads the condition endings and blackout
// suppressions a later build wrote.
mod gate_386_sr2a;
// Workflow #386 SR-3, test rows T10, T11, T13 and T31: the suspension of a task package that
// admission refuses with a suspend code.
mod gate_386_sr3;
// Workflow #387 D1b: D1-M3 to D1-M7.
mod gate_387_d1b;
// Workflow #387 D1c: the ladder's host rows (H-4 G6, G7; D1-L2, D1-L3, D1-L4).
mod gate_387_d1c;
// Workflow #387 D1c-F: a direct input's invalid segmented swipe is refused by its contract.
mod gate_387_d1cf;
// Workflow #387 D1c-G: a ladder's unconfirmed ADB cleanup fences its instance, a frame without
// an extent ends only its run, a vision provider failure starts no ladder at once.
mod gate_387_d1cg;
// Workflow #389 MCP-4, register W2: the instance fact read records nothing.
mod gate_389_mcp4;
// Workflow #389 MCP-9a, register A5: `config.catalog` names the active policy catalog
// generation, its record, its latest approval decision and its predecessor.
mod gate_389_mcp9a;
// Workflow #389 U-2: the door binds each connection to its channel and marks, never refuses.
mod gate_389_u2;
// Workflow #390 OUT-1 (forced compromises O-03, O-04, O-05): outcome subsets, unproduced
// outcomes and zero-input packages with several outcomes.
mod gate_390_out1;
// Workflow #390 EXEC-1 (forced compromise E-04): a guard refusal falls to the page's next
// candidate operation.
mod gate_390_exec1;
// Workflow #390 EXEC-3 (forced compromise E-02): a page-graph task is rerun at once by its
// failure identity.
mod gate_390_exec3;
// Workflow #390 OUT-2 (forced compromises O-01, O-02, S-01): designated effect sets, their
// counts and caps.
mod gate_390_out2;
// Workflow #390 POOL-1 (forced compromise S-04): a `ledger_fact` pool takes its data from
// readings, its base reset by a reading and projected by the pool's regeneration rule.
mod gate_390_pool1;
// Workflow #390 READ-1 (forced compromise O-08): a fraction reading's denominator is published
// under its second reading key.
mod gate_390_read1;
// Workflow #390 DRILL-1 (forced compromise T-08): a person's drill of an instance's ladder is
// recorded like a real one, tagged `drill`, and acts on the device only when it says so.
mod gate_390_drill1;
// Workflow #390 九处-1 (corrected ruling 4 of wave 18): the status shows each emulator as the
// Runtime last read it, with the reading's time.
mod gate_390_nine1;
mod gate_392_daa;
mod gate_398_rd1a;
mod gate_398_rd1b;
// Workflow #398 RD-2: a handset's wired device link through the host.
mod gate_398_rd2;
// Workflow #398 RD-4: ladder profiles, a handset's held rung entry and a start beyond a profile.
mod gate_398_rd4;
// Workflow #398 RD-5b: a handset's device state, its holds and the start gate through the host.
mod gate_398_rd5b;
// Workflow #403: an unconfirmed close of one instance's resources fences only that instance.
mod gate_403;
mod gate_epoch_close;
// Test plan v2.2 E-4 G4b (exec key 12): bundle v3's offline verdict and the Runtime's admission.
mod gate_e4;
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
