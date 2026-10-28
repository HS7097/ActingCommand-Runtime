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
// #411 A52 (FLK-07's cause; host keys 171, 172), register §1.175: a close lease the instance's
// queue refuses fails only the capture's work; a close under a lapsed lease still halts the host.
mod gate_411_a52;
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
// #411 A22 (v0.12.3, host key 117), register §1.124: a direct run refuses a recovery binding that
// no path of it can use.
mod gate_411_a22;
// #411 A36b (0.12.x): a run whose diagnostic stream's seal finds its staged bytes changed fails
// only itself; the host keeps serving.
mod gate_411_a36b;
// #411 A36c (0.12.9): a PP-OCR diagnostic archive whose seal finds its staged report changed
// fails only the work that owns it, which records its terminal; the host keeps serving.
mod gate_411_a36c;
// #411 A44 writer (host key 174), register §1.173: the stored `task.recognition_completed` carries
// each candidate page's best score and verdict (ledger revision 4 row LR-8).
mod gate_411_a44;
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
// emu-watch (Alice's ruling 35): a pause never holds the emulator start; the Runtime polls its
// emulators and starts a configured one it finds stopped, within a bound.
mod gate_372k2;
mod gate_emu_watch;
// IDLE-1 (Alice's ruling 41): the idle planner on a manual clock.
mod gate_idle1;
// IDLE-1 (#419): a failed idle stop is contained to its instance and the Runtime goes on.
mod gate_idle1_host;
// #405 IDLE-1P: the closed instances and their wakes survive a restart, and a reading alone never
// wakes a closed game.
mod gate_idle1p;
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
// #411 B37 (A35-S4, host keys 162 and 163), register §1.169: a home entry's return-home layer or
// a navigation recovery chain step that ends busy ends the run with `contained_task_entry_busy`.
mod gate_411_b37_entry;
// Workflow #373 NAV-6: shared interrupt pages and the standard-pack root reader.
mod gate_373_nav6;
// Workflow #357 DB-1: the shared tables of a standard-pack game root at startup and in
// check-config's report.
mod gate_357_db1;
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
// #411 A51 (host key 130): a ledger read the writer does not answer in time latches nothing,
// and the first latched fatal reaches the daemon's log at once.
mod gate_411_a51;
// P0-LO: the start's ledger open runs under its configured deadline, and an open slower than
// its warning line gives `ledger_open_slow`.
mod gate_p0_lo;
// Workflow #383 AO-4q, test row AO4-c: status reads append no ledger record.
mod gate_383_ao4q;
// Workflow #383 SH-1, register §1.18: the reserved single-touch path drag was refused; its test
// retired with Workflow #406 T3 (its replacements are in gate_406_t3).
mod gate_383_sh1;
// Workflow #384 D2-S2: reading marks, recognition package bindings and the reader warnings.
mod gate_384_d2s2;
// Workflow #384 D2-S3: resource adjustments.
mod gate_384_d2s3;
// Workflow #411 B1 (slice BT-B1): frames that do not agree give no value, end to end.
mod gate_411_bt_b1;
// Workflow #384 D2-CHAIN and D2-S4: the fact refresh end to end, and the dispatch's reader check.
mod gate_384_d2s4;
// Workflow #384 D2-S5 and D2-S4's T31, T32: the transition recorder, its read, the stale intent
// and the ledger the fact refresh leaves alone.
mod gate_384_d2s5;
// Workflow #385 P-m: the shadow measurement and its status read.
mod gate_385_pm;
// Workflow #385 PM-CFG: the monitor, its sampler and the controller read the monitor's row.
mod gate_385_pmcfg;
// Workflow #427-B: the shared run limits bind in every pacing mode through the policy cycle, and
// the schedule explanation read answers their numbers.
mod gate_427b;
// Workflow #386 SR-2a, test row T6d: a start reads the condition endings and blackout
// suppressions a later build wrote.
mod gate_386_sr2a;
// Workflow #386 SR-3, test rows T10, T11, T13 and T31: the suspension of a task package that
// admission refuses with a suspend code.
mod gate_386_sr3;
// Workflow #386 SR-4a2 (SCH-M §3.2, §6 row SR-4a2): the policy's blackout positions 4 and 7, the
// day boundary, the admission re-check and the random delay's cut.
mod gate_386_sr4a2;
// Workflow #386 SR-4a3, test rows T17, T17b, T17c and T46: complete silence at the ladder, the
// startup package, the emulator autostart and the monitor probe; client runs pass.
mod gate_386_sr4a3;
// Workflow #387 D1b: D1-M3 to D1-M7.
mod gate_387_d1b;
// Workflow #387 D1c: the ladder's host rows (H-4 G6, G7; D1-L2, D1-L3, D1-L4).
mod gate_387_d1c;
// Workflow #387 D1c-F: a direct input's invalid segmented swipe is refused by its contract.
mod gate_387_d1cf;
// Workflow #387 D1c-G: a ladder's unconfirmed ADB cleanup fences its instance, a frame without
// an extent ends only its run, a vision provider failure starts no ladder at once.
mod gate_387_d1cg;
// #411 B37 (A35-S1, host keys 133 to 136), register §1.152: a return-home run that ends on a
// `busy` mapping is not recovered; the ladder holds and runs it again, or `on_exhausted` decides.
mod gate_411_b37;
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
// Workflow #392 DA-4: a game root's display declaration at startup and in a run's preparation.
mod gate_392_da4;
// Workflow #392 DA-5a: the identity test of the ordered task schema check.
mod gate_392_da5a;
// Workflow #392 DA-2a: the run's display map, its rebuild on a changed frame and the input
// prerequisite under a pure scale.
mod gate_392_da2a;
// Workflow #392 DA-4b-D (Alice's ruling 69): the declared safe area, its live changes and its
// refusals.
mod gate_392_da4bd;
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
// #411 A38: a fence that waits for a person re-checks itself; an emulator control command that
// times out reads the status first; the wait is visible in status.
mod gate_411_a38;
// #411 A35-S5 (host key 410), register §1.170: the status shows a recovery ladder that holds
// instead of climbing: what it waits for, how long it has held and its limit.
mod gate_411_a35_s5;
// Workflow #405 D1 (host key 122), register §1.135: once an instance has its random delay
// enabled, every task the scheduler dispatches there first waits a random time.
mod gate_405_d1;
// SCH-M CF-2: a countdown mark publishes its frame time plus the countdown as a timestamp_ms
// fact, and fact_time_reached turns true at that instant.
mod gate_405_cf2;
// #411 A23 (host key 118), register §1.125: a run whose own package dispatched no step after its
// prerequisite layer is judged on its own recognition.
mod gate_411_a23;
// #411 A24 (host key 125), register §1.139: `actingd suspended` judges clock slots against the
// ledger's latest time, the maximum timestamp.
mod gate_411_a24;
// #411 A34: a late capacity sample is re-sampled before an admission refuses on it; a run whose
// capture evidence meets a capacity refusal still commits its terminal.
mod gate_411_a34;
// #411 A45: an application entry return-home layer that meets its declared error page ends with
// `contained_task_error_page_reached`, never `contained_task_state_invalid`.
mod gate_411_a45;
// #411 A46: one instance's application-backend failure fences that instance; the host, its
// policy monitor's setup and closeout, and the other instances go on.
mod gate_411_a46;
// #411 A47: a ladder whose application stop fails keeps its key and climbs to its next rung.
mod gate_411_a47;
// #411 A64: a run's resource close that meets its lapsed lease fences only its instance; the
// Runtime, the run's own terminal and the other instances go on.
mod gate_411_a64;
// ISO-1 (#419, ISO-M): the failure scope core: the scope classification, `contain_failure` at a
// boundary (fence, fail, latch, a failed fence latches) and the receipt's `host_halted` (A54).
mod gate_419_iso1;
// ISO-2 (#419, ISO-M): a device or kernel failure of one instance stays with that instance: the
// scheduled and host package run boundaries, the session, geometry and frame failures, the
// widened A46 fence and the retained-session closes of the emulator rung, the autostart, an
// operator's restart, a pause, a handset and a lease end.
mod gate_419_iso2;
// ISO-2b (#419, ISO-M R1): a device session that dies twice in a row holds only its instance;
// a Nemu input that the run's deadline, its cancel or its lease stopped fails with that work's
// own code, not as a device failure.
mod gate_419_iso2b;
// W2-E (#419, ISO-M R1, rows E13, A05): an input that meets its session closing, its endpoint
// changed or its frame no longer committed fails only the input and does not pause its task.
mod gate_419_w2e;
// #411 A35 (A35-S2, host keys 156 to 161), register §1.168: after a listed return-home failure
// the ladder probes the screen; while it still changes, the ladder holds instead of restarting.
mod gate_411_a35;
// #411 A48: a self-check reads the emulator status first: a running emulator the Runtime holds
// stopped is recognized again, a stopped one is `instance_not_running`, an unrecognized device
// says why.
mod gate_411_a48;
// #411 A49: a read-only observe between a run's recognition and its input does not displace the
// run's frame; the observe still captures a frame of its own.
mod gate_411_a49;
// #411 A50 (host keys 144-147), register §1.158: a capture open that cannot open says why; an
// operator start or restart queues the startup package unless a pause holds the instance; a
// restart launches only after the old VM has exited; the recovery preparation reads the
// emulator status first.
mod gate_411_a50;
// Workflow #411 B15 R4 (host key 113), register §1.114: the host runs declared remedies (table,
// journal, plan, admission check, stop, lift and expiry signals).
mod gate_411_b15h;
// Workflow #411 B15 R1 (host key 119): kernel admission by path and the metadata reader of the
// remedy declarations take a package that provides capabilities with parameters.
mod gate_411_b15p_host;
// Workflow #411 B15 R5 (host key 114), register §1.115: a run reads its parameters as read-only
// facts, never stored; `session.dispatch.` is refused to publishers.
mod gate_411_b15r;
// Workflow #411 B8 (host key 129), register §1.150: a policy run reads its dispatch's schedule
// values as `session.dispatch.schedule.*`; a direct run has none.
mod gate_411_b8;
// Workflow #411 B15 R6 (host key 115), register §1.116: an instance's remedy choices, their
// family, their write rule and their identity.
mod gate_411_b15w;
// Workflow #411 B15 R7 (host key 116), register §1.117: `actingd suspended` lists the stopped
// remedy rounds; check-config notes the remedies that cannot be used.
mod gate_411_b15w2;
// #411 E2 (0.12.9): a guard refusal names the page and the frame it judged on (T2, A30); a Lab
// operation naming a recognition target id fails with the type mismatch (T3, A37).
mod gate_411_e2;
// #411 Lab L5a (host key 120), register §1.130: every online Lab observation takes the frames its
// package's target consensus needs, as one transaction.
mod gate_411_lab_l5;
// #411 Lab L5b (host key 121), register §1.130: `observe --capture` projects a multi-frame
// candidate layout over its own series.
mod gate_411_lab_l5b;
// #411 A18 option ④ (host key 128), register §1.147: online `do`'s arrival samples start every
// 500 ms, start to start.
mod gate_411_bt_l18;
// Workflow #406 T12 (host key 123), register §1.136: the touch keys in effect and the landing
// history of single-region presses.
mod gate_406_t12;
// Workflow #406 T3 (host key 124), register §1.137: a run performs the path drag the kernel
// generates, on a segmented backend and on the Nemu input.
mod gate_406_t3;

// #412 Lab B-3 S2 (host keys 152 to 155), register §1.162: the host runs one declared operation
// over the Lab lease and writes its LR-10 records.
mod gate_412_b3_s2;
// LN-2 (Workflow #422, LEAN-L model §3.2 step 5): the instance fact store loads from the tagged
// `state` rows, equal to the whole-ledger replay; a doctored row stops the load.
mod gate_422_ln2;
// LN-3 (Workflow #422, LEAN-L model §3.6): each change of the decision memory and each wake
// obligation writes one value record with the complete state, and its `state` row names it.
mod gate_422_ln3;
// LN-4 (Workflow #422, LEAN-L model §3.2, §3.6): the decision memory, the agent dispatcher and the
// monitor registry load from their tagged records; a start without the marker seeds them once.
mod gate_422_ln4;
// LN-8c (Workflow #422, LEAN-L model §7.1 A3): the start's runtime-fact recovery reads its record
// events in bounded pages and rebuilds the store one read of them rebuilds.
mod gate_422_ln8c;
// Workflow #427-A fix 1 (review M1): a handed-off run whose worker panicked is settled online by
// the start's rule once its lease lapsed, once, and its instance is free again.
mod gate_427a_m1;
// #422 LN-TUNE: a second start reads no planning history before the planning checkpoint.
mod gate_ln_tune;
// #422 LN-9: a finding of the ledger's scrub stops the Runtime as a Global failure.
mod gate_ln9;
// #422 LN-WS: the incident's reads answer while an append commits behind them, and an append the
// writer does not answer in time fails naming the ledger's code.
mod gate_ln_ws;
// #419 DIAG-RUN: a task diagnostic's read the writer does not answer in time fails only its run;
// a fatal ledger failure of that read still stops the Runtime.
mod gate_diag_run;
// #419 LN-WS-F: the reconciliation's and the proposals' recurring ledger reads stay bounded, and
// one the writer does not answer in time fails only its own settlement step or request; a live
// settlement's read after its commit stays fatal (fix round 1).
mod gate_ln_ws_f;
// #422 IDLE-OV: a QueryEvents page whose read its budget stopped answers `ledger_read_failure`
// (`runtime_unavailable`, failed), not `protocol_invalid`.
mod gate_422_idle_ov;
// #422 W1-8: Status, MonitorStatus and RuntimeFactSnapshot read the published ledger head and
// answer while the writer is held (P30); PerformanceStatus shows the reader pool's pages (P29).
mod gate_w1_8;
// #421 S2-IF: `StartReport` names an instruction kept over a changed default base-changed and
// restored, never dropped; a key gone or superseded is dropped.
mod gate_421_s2if;

mod gate_epoch_close;
// #411 A65: the start settles the open direct runs of a previous epoch larger than one unpaged
// read, and of the epochs before a start that failed after appending its start event.
mod gate_411_a65;
// #411 P51: the next start closes no scope of an epoch's client actions (MCP's `client.action`
// after each Lab observation), so its closes do not grow with them.
mod gate_411_p51;
// #411 A67-R: a start whose memory sample leaves no frame budget after the OS reserve goes on, and
// records the floor warning once in the daemon's log and once in the ledger.
mod gate_411_a67r;
// #71 FLK-07, slice HARNESS-DIAG (host key 168): a receipt the test client cannot read names the
// fatal the Runtime under test latched and its shutdown flag.
mod gate_411_harness_diag;
// Test plan v2.2 E-4 G4b (exec key 12): bundle v3's offline verdict and the Runtime's admission.
mod gate_e4;
// SCH-M ED-2 (frozen model v1.0 §3.4, §6 row ED-2): the run-duration samples of the instance store
// record `health.run_duration` and the expected duration E in the evaluation and its admission.
mod gate_ed2;
// Workflow #386 SR-6b, MCP-19a (host keys 126 and 127), register §1.143: the latest evaluation's
// not-run explanation in the policy host's memory and its read.
mod gate_mcp19a;
mod gate_p0;
// ROUT-L RL-1b (frozen model v1.0 §6.1 row RL-1b, §7.1 A2-A6, A8, A9): the scheduler workbench.
mod gate_rl1b;
// ROUT-L RL-1c (#411; work order `rd-1027/brief-rl1c.md` C-2, C-5): the admission after start and
// the suspended report in composer mode.
mod gate_rl1c;
// ROUT-L RL-1c fix round 1 (#411; work order `rd-1027/brief-rl1c-fix1.md`, review H1 and M1): the
// suspended report's bounded ledger read and the composition's problems as its warnings.
mod gate_rl1c_fix1;
// ROUT-L RL-1c fix round 2 (#411; work order `rd-1027/brief-rl1c-fix2.md`, re-review M2 and L3):
// the suspended report's smaller selection, its deadline path and a run it leaves unread.
mod gate_rl1c_fix2;
// ROUT-L RL-1c follow-ups (#411 L1 and review 2 L1; 0.12 wave 1 slice W1-9): a run's read within
// the deadline, and the bounded walk for the daemon start that leaves the rest answered.
mod gate_rl1c_follow;
mod home_entry;
mod input;
mod input_failure;
mod instance_workers;
// LN-1 (Workflow #422, LEAN-L model §7.2): the scale harness on the synthetic ledger; from LN-8 on
// it gates the model's targets (§7.1 A1 to A3).
mod lean_scale;
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
