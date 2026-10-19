// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned catalog generations and replayable policy admission state.

mod catalog_preview;
pub(crate) mod clock_slot;
pub use catalog_preview::{
    CatalogPreview, CatalogPreviewPhases, CatalogPreviewRequest, preview_policy_catalog_transition,
};
mod catalog_transaction;
pub(crate) use catalog_transaction::catalog_ledger_error;
mod planning_transaction;
use crate::codes::HostLocation;
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Location, Outcome, VocabularyToken};
use planning_transaction::planning_state_error;
pub(crate) use planning_transaction::planning_transaction_error;

use crate::catalog_plan::CatalogLineage;
use crate::codes::HostCode;
use crate::policy_control::{
    PolicyControlState, PolicyExecutionInput, PolicyExecutionTiming, active_activity_window,
    is_availability_denial,
};
use crate::recovery_read::{RECOVERY_PAGE_EVENTS, read_event_types};
use crate::suspension_lift::SuspensionLiftView;
use crate::{
    InstanceArbitrationRank, PerformanceControlWorkload, ProcedureManifest, RuntimeHostError,
    RuntimeHostResult,
};
use actingcommand_contract::{
    CatalogPayload, CorrelationId, EventPayload, EventQuery, EventType, InstanceId,
    IssuedCorrelationId, IssuedRunId, IssuedTaskId, LeaseId, LeaseToken, OwnerEpoch,
    PerformanceContext, PolicyAdmissionRecord, PolicyConditionSettlement,
    PolicyDetectionBudgetRecord, PolicyDispatchEventData, PolicyExecutionEventData,
    PolicyExecutionOutcome, PolicyPayload, PolicyPlanningSignalEventData, PolicyPlanningSignalKind,
    PolicyReasonRecord, ProjectDecisionPageRequest, RequestId, RunId, RuntimeErrorCode,
    RuntimeRequest, TaskId,
};
use actingcommand_ledger::{GlobalLedger, PersistedEvent};
use actingcommand_policy::{
    ActivityProfile, CandidateEligibility, CatalogDocumentSource, CatalogSources, CompiledCatalog,
    CompletedActivityWindow, DecisionReason, DecisionReasonChain, DetectionSuggestion,
    DispatchIntent, DispatchPrerequisites, EligibilityState, EvaluationFacts, EvaluationResources,
    EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader, InstanceSnapshot,
    MAX_EVALUATION_INSTANCES, MAX_TASK_FAILURE_STREAK, MAX_TASK_LAST_DURATION_MS, PolicyEvaluation,
    REMEDY_JOURNAL_MAX_RUNS, RemedyInputs, RemedyJournal, RemedyPlan, RemedyRun, RemedySettlement,
    SchedulingDecisionState, ScopeSelector, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState,
    activity_window_at, compile_catalog, evaluate_with_remedies, plan_remedies,
};
use actingcommand_runtime_state::{PlanningQuotaUsage as DetectionQuotaUsage, RuntimeStateStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const CATALOG_STATE_SCHEMA: &str = "actingcommand.catalog-state.v1";
const LEGACY_CATALOG_POINTER_SCHEMA: &str = "actingcommand.catalog-pointer-file.v1";
const ACTIVE_POINTER_FILE: &str = "active.json";
const ACTIVE_POINTER_STATE_KEY: &str = "policy.catalog.active";
const GENERATIONS_DIR: &str = "generations";
const MAX_POINTER_BYTES: usize = 16 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
// Published generations are immutable, so verified compiled catalogs are memoised by hash.
const MAX_CATALOG_MEMO_GENERATIONS: usize = 8;
const DEFAULT_DEBOUNCE_MS: u64 = 250;
const DEFAULT_COOLDOWN_MS: u64 = 1_000;
const DEFAULT_RECONCILIATION_INTERVAL_MS: u64 = 60_000;
const DEFAULT_CLOCK_JUMP_THRESHOLD_MS: u64 = 5_000;
// One evaluation can reference at most one current activity window per bounded instance.
const MAX_DETECTION_QUOTA_CACHE_WINDOWS: usize = MAX_EVALUATION_INSTANCES;
static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogGeneration {
    schema_version: String,
    catalog_id: String,
    catalog_version: u64,
    catalog_hash: String,
    sources: Vec<CatalogSourceRecord>,
}

impl CatalogGeneration {
    pub fn catalog_id(&self) -> &str {
        &self.catalog_id
    }

    pub const fn catalog_version(&self) -> u64 {
        self.catalog_version
    }

    pub fn catalog_hash(&self) -> &str {
        &self.catalog_hash
    }

    #[cfg(test)]
    pub(crate) fn for_plan_test(
        catalog_id: &str,
        catalog_version: u64,
        catalog_hash: &str,
    ) -> Self {
        Self {
            schema_version: CATALOG_STATE_SCHEMA.to_owned(),
            catalog_id: catalog_id.to_owned(),
            catalog_version,
            catalog_hash: catalog_hash.to_owned(),
            sources: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogSourceRecord {
    kind: String,
    source_uri: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogPointer {
    schema_version: String,
    generation: CatalogGeneration,
}

#[derive(Debug, Clone)]
pub struct PolicyCadence {
    pub debounce_ms: u64,
    pub cooldown_ms: u64,
    pub reconciliation_interval_ms: u64,
    pub clock_jump_threshold_ms: u64,
}

impl Default for PolicyCadence {
    fn default() -> Self {
        Self {
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            cooldown_ms: DEFAULT_COOLDOWN_MS,
            reconciliation_interval_ms: DEFAULT_RECONCILIATION_INTERVAL_MS,
            clock_jump_threshold_ms: DEFAULT_CLOCK_JUMP_THRESHOLD_MS,
        }
    }
}

impl PolicyCadence {
    pub(crate) fn validate(&self) -> RuntimeHostResult<()> {
        if self.debounce_ms == 0
            || self.cooldown_ms < self.debounce_ms
            || self.reconciliation_interval_ms < self.cooldown_ms
            || self.clock_jump_threshold_ms == 0
        {
            return Err(fatal(
                HostCode::InvalidPolicyCadence,
                HostLocation::InitializePolicyRuntime,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyTrigger {
    FactsChanged,
    ResourcesChanged,
    CatalogChanged,
    Reconciliation,
    Recovery,
    ClockObserved { previous_unix_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyRecomputeKind {
    Incremental,
    Full,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyRecomputeReason {
    Event,
    StartupOrRecovery,
    CatalogActivation,
    ClockJump,
    Reconciliation,
    Debounce,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyRecomputeDirective {
    pub kind: PolicyRecomputeKind,
    pub reason: PolicyRecomputeReason,
    pub eligible_at_unix_ms: u64,
}

/// Separates cadence requests from the current full scan without changing the pure evaluator API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEvaluationExecution {
    FullCatalogScan,
}

/// Deterministic input cardinalities used to budget a policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyEvaluationCost {
    pub catalog_records: u64,
    pub catalog_tasks: u64,
    pub fact_records: u64,
    pub outcome_records: u64,
    pub task_state_records: u64,
    pub instance_records: u64,
    pub resource_records: u64,
    pub task_instance_pairs: u64,
    pub work_units: u64,
}

/// Runtime-side timing and cost evidence for one non-deferred policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyEvaluationMeasurement {
    pub sampled_at_monotonic_ms: u64,
    pub elapsed_micros: u64,
    pub requested_recompute: PolicyRecomputeKind,
    pub execution: PolicyEvaluationExecution,
    pub cost: PolicyEvaluationCost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyCycle {
    pub directive: PolicyRecomputeDirective,
    pub evaluation: Option<PolicyEvaluation>,
    pub pending_dispatch_intents: Vec<DispatchIntent>,
    pub detection_planning_signals: Vec<PolicyPlanningSignalEventData>,
    /// Workflow #372 K2: the clock-slot records this cycle appends after its detection signals.
    pub clock_slot_signals: Vec<PolicyPlanningSignalEventData>,
    pub measurement: Option<PolicyEvaluationMeasurement>,
    /// Workflow #384 D2-S5 (D2 model §5.1 MP-3): the fact refresh recorder's transitions at this
    /// cycle's yield point, in memory only: actingd writes them as daemon lines, and no
    /// decision reads them. Empty for a deferred cycle.
    pub fact_refresh_transitions: Vec<crate::FactRefreshTransition>,
}

pub(crate) struct PolicyEvaluationContext<'a> {
    pub(crate) procedure_manifest: &'a ProcedureManifest,
    pub(crate) time: EvaluationTime,
    pub(crate) seed: u64,
    pub(crate) trigger: PolicyTrigger,
    pub(crate) sampled_at_monotonic_ms: u64,
    /// Workflow #191 ps1: the deferral code of an operator scheduling pause covering the
    /// instance alias, if any.
    pub(crate) scheduling_pause: &'a dyn Fn(&str) -> Option<HostCode>,
    /// Workflow #369 W-1: what the instance's one queue shows as the cycle starts, if anything
    /// keeps a dispatch from it.
    pub(crate) instance_occupancy: &'a dyn Fn(&str) -> Option<InstanceOccupancy>,
    /// Workflow #336 L6 (§12.7): the configuration a paused pair's suspension is lifted
    /// against, exactly as admission judges it.
    pub(crate) suspension_lift: &'a SuspensionLiftView<'a>,
    /// Workflow #386 SR-3: the task package suspensions, judged with the same lift checks.
    pub(crate) pack_admission: &'a crate::pack_admission::PackAdmission,
    /// Workflow #384 D2-S4: what the scheduler's fact refresh reads from the Runtime.
    pub(crate) fact_refresh: FactRefreshContext<'a>,
    /// Workflow #411 B15 R4: the remedy keys in effect as the cycle starts.
    pub(crate) remedy: &'a crate::remedy::RemedyContext,
}

/// Workflow #384 D2-S4 (D2 model §3.1, §3.4): the `scheduling.fact_refresh.*` settings in effect
/// as the cycle starts, the recognition maintenance packages the start kept, and L, when the
/// start finished loading the instance facts.
pub(crate) struct FactRefreshContext<'a> {
    pub(crate) settings: actingcommand_policy::FactRefreshSettings,
    pub(crate) bindings: &'a BTreeMap<String, crate::fact_refresh::FactRefreshBinding>,
    pub(crate) instance_facts_loaded_at_unix_ms: u64,
}

/// Workflow #369 W-1: what keeps a policy dispatch from an instance as a cycle starts. Policy
/// dispatches only to an instance with no holder, no eligible waiting claim and no takeover
/// cooldown (Q-3 (6)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstanceOccupancy {
    /// A claim holds the instance, or an eligible claim waits for it: `holder_kind` is the
    /// holder, or the waiting claim that receives the key next; `count` is the number of
    /// eligible waiting claims.
    Held {
        holder_kind: actingcommand_scheduler::ClaimKind,
        count: usize,
    },
    /// The instance is free but its takeover cooldown runs until `until_unix_ms`.
    Cooldown { until_unix_ms: u64 },
}

/// Correlates an admission request; Runtime rebuilds approval authority and current time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAdmissionContext {
    pub fact_ledger_position: u64,
    pub fact_snapshot_id: String,
    pub approval_fact_ids: BTreeSet<String>,
    pub fencing_owner_epoch: OwnerEpoch,
    pub now_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDispatchAdmission {
    Granted {
        context: Box<PolicyRunContext>,
    },
    ReplaySuppressed {
        decision_id: String,
        catalog: CatalogGeneration,
        original_intent_sequence: u64,
    },
}

/// Non-forgeable in-process authority for one admitted policy run.
///
/// The scheduler mints this context once and the scheduled contained-task path consumes it
/// without acquiring another lease or reconstructing identity from strings or logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRunContext {
    request: RuntimeRequest,
    correlation_id: IssuedCorrelationId,
    run_id: IssuedRunId,
    task_id: IssuedTaskId,
    catalog: CatalogGeneration,
    token: LeaseToken,
    admission: PolicyAdmissionRecord,
    intent: DispatchIntent,
    reason_chain: DecisionReasonChain,
}

impl PolicyRunContext {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        request: RuntimeRequest,
        correlation_id: IssuedCorrelationId,
        run_id: IssuedRunId,
        task_id: IssuedTaskId,
        catalog: CatalogGeneration,
        token: LeaseToken,
        admission: PolicyAdmissionRecord,
        intent: DispatchIntent,
        reason_chain: DecisionReasonChain,
    ) -> RuntimeHostResult<Self> {
        if reason_chain.decision_id != intent.decision_id
            || reason_chain.id != intent.reason_chain_id
            || catalog.catalog_hash() != intent.catalog_hash
            || catalog.catalog_version() != intent.catalog_version
        {
            return Err(RuntimeHostError::fatal(
                HostCode::PolicyRunIdentityMismatch,
                HostLocation::BuildPolicyRunContext,
                RuntimeErrorCode::RuntimeHalted,
            ));
        }
        Ok(Self {
            request,
            correlation_id,
            run_id,
            task_id,
            catalog,
            token,
            admission,
            intent,
            reason_chain,
        })
    }

    pub fn decision_id(&self) -> &str {
        &self.intent.decision_id
    }

    pub const fn run_id(&self) -> RunId {
        *self.run_id.transport()
    }

    pub const fn task_id(&self) -> TaskId {
        *self.task_id.transport()
    }

    pub const fn admission_request_id(&self) -> RequestId {
        self.request.request_id()
    }

    pub const fn correlation_id(&self) -> CorrelationId {
        *self.correlation_id.transport()
    }

    pub fn instance_alias(&self) -> &str {
        &self.intent.instance_id
    }

    pub fn operation_id(&self) -> &str {
        &self.intent.operation_id
    }

    pub(crate) fn catalog_task_id(&self) -> &str {
        &self.intent.task_id
    }

    pub fn procedure_ref(&self) -> &str {
        &self.intent.procedure_ref
    }

    pub fn reason_chain(&self) -> &DecisionReasonChain {
        &self.reason_chain
    }

    pub const fn lease_token(&self) -> &LeaseToken {
        &self.token
    }

    pub const fn admission(&self) -> &PolicyAdmissionRecord {
        &self.admission
    }

    pub const fn catalog(&self) -> &CatalogGeneration {
        &self.catalog
    }

    pub(crate) const fn request(&self) -> &RuntimeRequest {
        &self.request
    }

    pub(crate) const fn issued_correlation_id(&self) -> IssuedCorrelationId {
        self.correlation_id
    }

    pub(crate) const fn issued_run_id(&self) -> IssuedRunId {
        self.run_id
    }

    pub(crate) const fn issued_task_id(&self) -> IssuedTaskId {
        self.task_id
    }
}

pub(crate) enum PolicyExecutionPreparation {
    New(PolicyExecutionEventData),
    Replay(PolicyExecutionEventData),
}

#[derive(Clone)]
pub(crate) struct LoadedCatalog {
    generation: CatalogGeneration,
    compiled: CompiledCatalog,
    sources: CatalogSources,
}

impl LoadedCatalog {
    pub(crate) fn generation(&self) -> &CatalogGeneration {
        &self.generation
    }

    pub(crate) fn sources(&self) -> &CatalogSources {
        &self.sources
    }

    pub(crate) fn compiled(&self) -> &CompiledCatalog {
        &self.compiled
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenDispatch {
    data: PolicyDispatchEventData,
    admission: Option<PolicyAdmissionRecord>,
    execution: Option<PolicyExecutionEventData>,
    completed_run: Option<CompletedPolicyRunIdentity>,
    intent_sequence: u64,
    admitted_sequence: Option<u64>,
    rejected_sequence: Option<u64>,
    completed_sequence: Option<u64>,
    lifecycle: DispatchLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedPolicyRunIdentity {
    pub(crate) decision_id: String,
    pub(crate) catalog_task_id: String,
    pub(crate) instance_alias: String,
    pub(crate) instance_id: InstanceId,
    pub(crate) admission_request_id: RequestId,
    pub(crate) correlation_id: CorrelationId,
    pub(crate) task_id: TaskId,
    pub(crate) run_id: RunId,
    pub(crate) lease_id: LeaseId,
    pub(crate) execution_outcome: PolicyExecutionOutcome,
    pub(crate) completion_sequence: u64,
    pub(crate) activity_window_id: String,
    /// Workflow #390 OUT-1 (FCM O-04): the outcome keys the run's own catalog referenced for
    /// its task. Admission refuses a key the package does not declare, so the package declared
    /// every one of them.
    pub(crate) referenced_outcome_keys: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyOutcomeKeySnapshot {
    pub(crate) generation: Option<CatalogGeneration>,
    pub(crate) keys: BTreeMap<String, BTreeSet<String>>,
    pub(crate) completed_runs: BTreeMap<(String, String), CompletedPolicyRunIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyDispatchProjectionState {
    Intent,
    Admitted,
    Rejected,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyDispatchProjection {
    pub(crate) data: PolicyDispatchEventData,
    pub(crate) state: PolicyDispatchProjectionState,
    pub(crate) intent_sequence: u64,
}

pub(crate) struct PolicyDispatchPage {
    pub(crate) dispatches: Vec<PolicyDispatchProjection>,
    pub(crate) snapshot_ledger_position: u64,
    pub(crate) requested_limit: u16,
    pub(crate) has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchLifecycle {
    Intent,
    Admitted,
    Rejected,
    Completed,
}

impl SeenDispatch {
    fn projected_state_at(&self, ledger_position: u64) -> PolicyDispatchProjectionState {
        if self
            .completed_sequence
            .is_some_and(|sequence| sequence <= ledger_position)
        {
            PolicyDispatchProjectionState::Completed
        } else if self
            .rejected_sequence
            .is_some_and(|sequence| sequence <= ledger_position)
        {
            PolicyDispatchProjectionState::Rejected
        } else if self
            .admitted_sequence
            .is_some_and(|sequence| sequence <= ledger_position)
        {
            PolicyDispatchProjectionState::Admitted
        } else {
            PolicyDispatchProjectionState::Intent
        }
    }
}

struct PolicyCadenceState {
    config: PolicyCadence,
    full_recompute_required: bool,
    last_event_unix_ms: Option<u64>,
    last_full_recompute_unix_ms: Option<u64>,
    cooldown_until_unix_ms: u64,
}

impl PolicyCadenceState {
    fn new(config: PolicyCadence) -> RuntimeHostResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            full_recompute_required: true,
            last_event_unix_ms: None,
            last_full_recompute_unix_ms: None,
            cooldown_until_unix_ms: 0,
        })
    }

    fn catalog_changed(&mut self) {
        self.full_recompute_required = true;
    }

    fn observe(
        &mut self,
        trigger: PolicyTrigger,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<PolicyRecomputeDirective> {
        if now_unix_ms == 0 {
            return Err(request(
                HostCode::InvalidPolicyTime,
                HostLocation::EvaluatePolicyCycle,
            ));
        }
        if self.full_recompute_required || trigger == PolicyTrigger::Recovery {
            return self.full(now_unix_ms, PolicyRecomputeReason::StartupOrRecovery);
        }
        if let PolicyTrigger::ClockObserved { previous_unix_ms } = trigger {
            let forward_delta = now_unix_ms.saturating_sub(previous_unix_ms);
            if now_unix_ms < previous_unix_ms || forward_delta > self.config.clock_jump_threshold_ms
            {
                return self.full(now_unix_ms, PolicyRecomputeReason::ClockJump);
            }
        }
        if trigger == PolicyTrigger::CatalogChanged {
            return self.full(now_unix_ms, PolicyRecomputeReason::CatalogActivation);
        }
        if trigger == PolicyTrigger::Reconciliation {
            let due = self
                .last_full_recompute_unix_ms
                .and_then(|last| last.checked_add(self.config.reconciliation_interval_ms))
                .ok_or_else(|| {
                    fatal(
                        HostCode::PolicyTimeOverflow,
                        HostLocation::EvaluateReconciliationTrigger,
                    )
                })?;
            if now_unix_ms >= due {
                return self.full(now_unix_ms, PolicyRecomputeReason::Reconciliation);
            }
            return Ok(PolicyRecomputeDirective {
                kind: PolicyRecomputeKind::Deferred,
                reason: PolicyRecomputeReason::Reconciliation,
                eligible_at_unix_ms: due,
            });
        }
        if now_unix_ms < self.cooldown_until_unix_ms {
            return Ok(PolicyRecomputeDirective {
                kind: PolicyRecomputeKind::Deferred,
                reason: PolicyRecomputeReason::Cooldown,
                eligible_at_unix_ms: self.cooldown_until_unix_ms,
            });
        }
        let debounce_until = self
            .last_event_unix_ms
            .and_then(|last| last.checked_add(self.config.debounce_ms))
            .unwrap_or(now_unix_ms);
        self.last_event_unix_ms = Some(now_unix_ms);
        if now_unix_ms < debounce_until {
            return Ok(PolicyRecomputeDirective {
                kind: PolicyRecomputeKind::Deferred,
                reason: PolicyRecomputeReason::Debounce,
                eligible_at_unix_ms: debounce_until,
            });
        }
        self.cooldown_until_unix_ms = now_unix_ms
            .checked_add(self.config.cooldown_ms)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyTimeOverflow,
                    HostLocation::EvaluatePolicyEvent,
                )
            })?;
        Ok(PolicyRecomputeDirective {
            kind: PolicyRecomputeKind::Incremental,
            reason: PolicyRecomputeReason::Event,
            eligible_at_unix_ms: now_unix_ms,
        })
    }

    fn full(
        &mut self,
        now_unix_ms: u64,
        reason: PolicyRecomputeReason,
    ) -> RuntimeHostResult<PolicyRecomputeDirective> {
        self.full_recompute_required = false;
        self.last_event_unix_ms = Some(now_unix_ms);
        self.last_full_recompute_unix_ms = Some(now_unix_ms);
        self.cooldown_until_unix_ms = now_unix_ms
            .checked_add(self.config.cooldown_ms)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyTimeOverflow,
                    HostLocation::EvaluateFullPolicyCycle,
                )
            })?;
        Ok(PolicyRecomputeDirective {
            kind: PolicyRecomputeKind::Full,
            reason,
            eligible_at_unix_ms: now_unix_ms,
        })
    }
}

/// One (task, instance) pair's latest settled policy run as its settlement facts describe it
/// (Workflow #308 slice 5b). Runs of one pair are serialized by the instance lease, so
/// admission order is settlement order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicySettlement {
    pub(crate) catalog_task_id: String,
    pub(crate) instance_alias: String,
    /// `runtime_ms` of a success; `observed_at_unix_ms - admitted_at_unix_ms` of a failure,
    /// 0 when a settlement reconciled from ledger timestamps precedes its admission.
    pub(crate) duration_ms: u64,
    pub(crate) succeeded: bool,
    /// Consecutive failed runs ending at this run; 0 after a success.
    pub(crate) failure_streak: u64,
    pub(crate) completed_at_unix_ms: u64,
}

/// How one evaluated decision moves its pair's eligibility age (Workflow #308 slice 5b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EligibilityVerdict {
    /// Eligible, Selected or `score_deferred`, with no dispatch in flight: keep or start.
    Waiting,
    /// Eligible but deferred for another reason, or waiting with a dispatch in flight: keep
    /// an existing age, start none.
    Retained,
    /// Blocked or not eligible: the age ends.
    Dropped,
}

fn eligibility_verdict(decision: &TaskDecision, in_flight: bool) -> EligibilityVerdict {
    if decision.eligibility != EligibilityState::True
        || decision.state == SchedulingDecisionState::Blocked
    {
        return EligibilityVerdict::Dropped;
    }
    let waiting = match decision.state {
        SchedulingDecisionState::Eligible | SchedulingDecisionState::Selected => true,
        SchedulingDecisionState::Deferred => decision.reasons.iter().any(|reason| {
            reason
                .outcome
                .is(actingcommand_policy::PolicyCode::ScoreDeferred)
        }),
        SchedulingDecisionState::Blocked => false,
    };
    if waiting && !in_flight {
        EligibilityVerdict::Waiting
    } else {
        EligibilityVerdict::Retained
    }
}

/// Memory-only eligibility ages (Workflow #308 slice 5b, iron rule 13): the evaluation instant
/// at which each (task, instance) pair started waiting. Nothing here reaches the ledger; a
/// host starts with an empty map.
#[derive(Debug, Default)]
struct EligibilityAges {
    since: BTreeMap<(String, String), u64>,
    /// The last evaluated cycle's verdicts. They apply when the next evaluation starts, so an
    /// evaluation and the admissions of its intents project the same ages.
    pending: Option<EligibilityVerdicts>,
    /// Whether any cycle's verdicts were applied since the host opened.
    known: bool,
}

#[derive(Debug)]
struct EligibilityVerdicts {
    evaluated_at_unix_ms: u64,
    waiting: BTreeSet<(String, String)>,
    retained: BTreeSet<(String, String)>,
}

impl EligibilityAges {
    fn apply_pending(&mut self) {
        let Some(verdicts) = self.pending.take() else {
            return;
        };
        let mut since = BTreeMap::new();
        for key in verdicts.waiting {
            let started = self
                .since
                .get(&key)
                .copied()
                .unwrap_or(verdicts.evaluated_at_unix_ms);
            since.insert(key, started);
        }
        for key in verdicts.retained {
            if let Some(started) = self.since.get(&key) {
                since.insert(key, *started);
            }
        }
        self.since = since;
        self.known = true;
    }

    fn stage(
        &mut self,
        evaluation: &PolicyEvaluation,
        evaluated_at_unix_ms: u64,
        in_flight: &BTreeSet<(&str, &str)>,
    ) {
        let mut verdicts = EligibilityVerdicts {
            evaluated_at_unix_ms,
            waiting: BTreeSet::new(),
            retained: BTreeSet::new(),
        };
        for decision in &evaluation.decisions {
            let Some(instance_id) = decision.instance_id.as_deref() else {
                continue;
            };
            let key = (decision.task_id.clone(), instance_id.to_owned());
            match eligibility_verdict(
                decision,
                in_flight.contains(&(decision.task_id.as_str(), instance_id)),
            ) {
                EligibilityVerdict::Waiting => {
                    verdicts.waiting.insert(key);
                }
                EligibilityVerdict::Retained => {
                    verdicts.retained.insert(key);
                }
                EligibilityVerdict::Dropped => {}
            }
        }
        self.pending = Some(verdicts);
    }

    fn forget(&mut self, task_id: &str, instance_id: &str) {
        let key = (task_id.to_owned(), instance_id.to_owned());
        self.since.remove(&key);
        if let Some(pending) = &mut self.pending {
            pending.waiting.remove(&key);
            pending.retained.remove(&key);
        }
    }
}

/// Per instance, the highest `effective_milli` and the longest `aging_ms` over the ranked
/// Eligible or Selected decisions of one evaluation; an instance without one is absent.
fn instance_arbitration_ranks(
    evaluation: &PolicyEvaluation,
) -> BTreeMap<String, InstanceArbitrationRank> {
    let mut ranks = BTreeMap::<String, InstanceArbitrationRank>::new();
    for decision in &evaluation.decisions {
        if !matches!(
            decision.state,
            SchedulingDecisionState::Eligible | SchedulingDecisionState::Selected
        ) {
            continue;
        }
        let (Some(instance_id), Some(rank)) = (&decision.instance_id, &decision.rank) else {
            continue;
        };
        ranks
            .entry(instance_id.clone())
            .and_modify(|entry| {
                entry.utility_milli = entry.utility_milli.max(rank.effective_milli);
                entry.aging_ms = entry.aging_ms.max(rank.aging_ms);
            })
            .or_insert(InstanceArbitrationRank {
                utility_milli: rank.effective_milli,
                aging_ms: rank.aging_ms,
            });
    }
    ranks
}

/// The policy half of the Runtime host.
///
/// Dispatch projection (Workflow #191 U5-F1):
/// - I-M: once the host is shared, whenever `HostShared.policy` is idle, `seen_dispatches`,
///   `dispatch_order`, `pinned_dispatches` and `control` equal what `recover_dispatches` would
///   rebuild from every policy dispatch event in the ledger. `eligibility`, `arbitration` and
///   `confirmed_run_admissions` are memory-only and not part of it.
/// - I-W: once the host is shared, code that appends a policy dispatch event (intent, admitted,
///   rejected, completed, execution recorded) (a) takes `HostShared.policy` before the append
///   and holds it until the event is applied; (b) applies the appended event with the checks
///   recovery runs, and turns any apply error fatal and marks it before releasing the guard;
///   (c) when the append itself returns an error, so the ledger may or may not hold the event,
///   re-aligns the whole projection with `refresh_dispatches` inside the same guard, and
///   treats a failed refresh as in (b).
/// - Startup reconciliation runs before the host is shared and ends with a full refresh.
/// - Hence a reader that reads ledger position N and then takes the guard sees every policy
///   dispatch event up to N.
pub(crate) struct PolicyHost {
    store: CatalogStore,
    active: Option<LoadedCatalog>,
    /// Workflow #361 A: every generation that was active, rebuilt from the ledger at open and
    /// extended by each published transition.
    lineage: CatalogLineage,
    cadence: PolicyCadenceState,
    seen_dispatches: BTreeMap<String, SeenDispatch>,
    dispatch_order: BTreeMap<u64, String>,
    pinned_dispatches: BTreeMap<String, CatalogGeneration>,
    detection_quota: DetectionQuotaState,
    control: PolicyControlState,
    eligibility: EligibilityAges,
    /// Memory-only arbitration input per instance (Workflow #308 slice 5c): the latest cycle
    /// that had an Eligible or Selected decision for the instance sets its entry.
    arbitration: BTreeMap<String, InstanceArbitrationRank>,
    /// Memory-only re-projection cache (Workflow #313 goal 5, #317 item F): per (task,
    /// instance) pair, the completed run whose admission request the ledger confirmed and the
    /// terminal sequence it was read through. A host starts with none.
    confirmed_run_admissions: BTreeMap<(String, String), (CompletedPolicyRunIdentity, u64)>,
    /// Workflow #369 E3 (#670 safety net): the dispatches this start could not settle. They
    /// stay open, out of the pending settlements and the active workloads; a later start that
    /// settles them leaves them out of this set.
    quarantined_dispatches: BTreeSet<String>,
    /// Workflow #411 B15 R4 (model-b15 §7.8, §9.1): the remedy table, the outcome keys of the
    /// runs of tasks that declare a remedy, and the remedy signals recorded. Memory only; the
    /// runs of the remedy journal are `seen_dispatches`.
    remedies: crate::remedy::RemedyState,
    /// Workflow #386 SR-6b, MCP-19a (register §1.143 M19A-2): the latest evaluated cycle's
    /// not-run explanation. Memory only; a host starts with none.
    explanation: crate::schedule_explanation::ScheduleExplanationTable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DetectionQuotaState {
    windows: BTreeMap<(String, String), DetectionQuotaUsage>,
    recency: VecDeque<(String, String)>,
}

impl PolicyHost {
    pub(crate) fn open(
        state_root: &Path,
        state: Arc<RuntimeStateStore>,
        ledger: &GlobalLedger,
        cadence: PolicyCadence,
        events: &crate::events::RuntimeEvents,
    ) -> RuntimeHostResult<Self> {
        let store = CatalogStore::open(state_root, state)?;
        store.migrate_legacy_active_pointer(ledger, events)?;
        let (active, lineage) = store.load_verified_active(ledger)?;
        let mut host = Self {
            store,
            active,
            lineage,
            cadence: PolicyCadenceState::new(cadence)?,
            seen_dispatches: BTreeMap::new(),
            dispatch_order: BTreeMap::new(),
            pinned_dispatches: BTreeMap::new(),
            detection_quota: DetectionQuotaState::default(),
            control: PolicyControlState::default(),
            eligibility: EligibilityAges::default(),
            arbitration: BTreeMap::new(),
            confirmed_run_admissions: BTreeMap::new(),
            quarantined_dispatches: BTreeSet::new(),
            remedies: crate::remedy::RemedyState::default(),
            explanation: crate::schedule_explanation::ScheduleExplanationTable::default(),
        };
        host.recover_dispatches(ledger)?;
        host.recover_planning_signals(ledger)?;
        Ok(host)
    }

    pub(crate) fn stage(&self, sources: &CatalogSources) -> RuntimeHostResult<LoadedCatalog> {
        self.store.stage(sources)
    }

    pub(crate) fn load_generation(&self, hash: &str) -> RuntimeHostResult<LoadedCatalog> {
        self.store
            .load_generation(hash)
            .map(|loaded| LoadedCatalog::clone(&loaded))
    }

    pub(crate) fn active_generation(&self) -> Option<CatalogGeneration> {
        self.active
            .as_ref()
            .map(|catalog| catalog.generation.clone())
    }

    pub(crate) fn active_sources(&self) -> Option<CatalogSources> {
        self.active.as_ref().map(|catalog| catalog.sources.clone())
    }

    pub(crate) fn active_loaded(&self) -> Option<LoadedCatalog> {
        self.active.clone()
    }

    pub(crate) fn active_loaded_at(
        &self,
        ledger: &GlobalLedger,
        ledger_position: u64,
    ) -> RuntimeHostResult<Option<LoadedCatalog>> {
        let latest = ledger.latest_sequence().map_err(|_| {
            fatal(
                HostCode::CatalogProjectionQueryFailed,
                HostLocation::ProjectPolicyCatalog,
            )
        })?;
        if ledger_position == 0 || ledger_position > latest {
            return Err(request(
                HostCode::CatalogProjectionPositionInvalid,
                HostLocation::ProjectPolicyCatalog,
            ));
        }
        self.store.project_catalog_source(ledger, ledger_position)
    }

    pub(crate) fn project_dispatches(
        &self,
        current_ledger_position: u64,
        page_request: &ProjectDecisionPageRequest,
    ) -> RuntimeHostResult<PolicyDispatchPage> {
        page_request.validate().map_err(|_| {
            request(
                SharedCode::ProjectDecisionPageInvalid,
                HostLocation::ProjectPolicyDispatches,
            )
        })?;
        let snapshot_ledger_position = page_request
            .cursor()
            .map_or(current_ledger_position, |cursor| {
                cursor.snapshot_ledger_position()
            });
        if snapshot_ledger_position == 0 || snapshot_ledger_position > current_ledger_position {
            return Err(request(
                SharedCode::ProjectDecisionCursorInvalid,
                HostLocation::ProjectPolicyDispatches,
            ));
        }
        let limit = usize::from(page_request.limit());
        let mut dispatches = Vec::with_capacity(limit.saturating_add(1));
        if let Some(cursor) = page_request.cursor() {
            if self
                .dispatch_order
                .get(&cursor.before_intent_sequence())
                .is_none_or(|decision_id| decision_id != cursor.before_decision_id())
            {
                return Err(request(
                    SharedCode::ProjectDecisionCursorInvalid,
                    HostLocation::ProjectPolicyDispatches,
                ));
            }
            for (_, decision_id) in self
                .dispatch_order
                .range(..cursor.before_intent_sequence())
                .rev()
                .take(limit.saturating_add(1))
            {
                dispatches.push(self.project_dispatch(decision_id, snapshot_ledger_position)?);
            }
        } else {
            for (_, decision_id) in self
                .dispatch_order
                .range(..=snapshot_ledger_position)
                .rev()
                .take(limit.saturating_add(1))
            {
                dispatches.push(self.project_dispatch(decision_id, snapshot_ledger_position)?);
            }
        }
        let has_more = dispatches.len() > limit;
        dispatches.truncate(limit);
        Ok(PolicyDispatchPage {
            dispatches,
            snapshot_ledger_position,
            requested_limit: page_request.limit(),
            has_more,
        })
    }

    fn project_dispatch(
        &self,
        decision_id: &str,
        snapshot_ledger_position: u64,
    ) -> RuntimeHostResult<PolicyDispatchProjection> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchOrderCorrupt,
                HostLocation::ProjectPolicyDispatches,
            )
        })?;
        if dispatch.intent_sequence > snapshot_ledger_position {
            return Err(fatal(
                HostCode::PolicyDispatchSnapshotInvalid,
                HostLocation::ProjectPolicyDispatches,
            ));
        }
        Ok(PolicyDispatchProjection {
            data: dispatch.data.clone(),
            state: dispatch.projected_state_at(snapshot_ledger_position),
            intent_sequence: dispatch.intent_sequence,
        })
    }

    pub(crate) fn pending_dispatches(&self) -> Vec<PolicyDispatchProjection> {
        self.seen_dispatches
            .values()
            .filter(|dispatch| dispatch.lifecycle == DispatchLifecycle::Intent)
            .map(|dispatch| PolicyDispatchProjection {
                data: dispatch.data.clone(),
                state: PolicyDispatchProjectionState::Intent,
                intent_sequence: dispatch.intent_sequence,
            })
            .collect()
    }

    /// One snapshot per (task, instance) of the active catalog. The dispatch, terminal and
    /// settlement fields are position-exact: the latest admitted dispatch at or before
    /// `ledger_position`, and `last_duration_ms` / `failure_streak` from the runs completed at
    /// or before it, saturated at the evaluator's input bounds. `eligible_since_unix_ms` is the
    /// memory-only eligibility age (Workflow #308 slice 5b): reported only when
    /// `eligibility_as_of_unix_ms` names the evaluation instant, and only for an age that
    /// started strictly before it (a younger one is zero and reads as absent). A
    /// position-only projection reports none.
    pub(crate) fn task_runtime_snapshots(
        &self,
        ledger_position: u64,
        eligibility_as_of_unix_ms: Option<u64>,
    ) -> RuntimeHostResult<Vec<TaskRuntimeSnapshot>> {
        let Some(active) = self.active.as_ref() else {
            return Ok(Vec::new());
        };
        // Workflow #384 D2-S4: a fact refresh's clocks are kept like a catalog task's.
        let task_ids = active
            .compiled
            .catalog()
            .tasks
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .chain([FACT_REFRESH_TASK_ID])
            .collect::<BTreeSet<_>>();
        // Workflow #372 K5: the dispatches of an id a task continues count as that task's, so
        // its snapshot is that of its accounting group's latest run.
        let accounting = accounting_tasks(&active.compiled);
        let accounted = |task_id: &str| {
            task_ids
                .get(task_id)
                .copied()
                .or_else(|| accounting.get(task_id).copied())
        };
        let settlements = fold_settlements_by(
            self.seen_dispatches.values().filter(|dispatch| {
                accounted(dispatch.data.task_id.as_str()).is_some()
                    && dispatch
                        .completed_sequence
                        .is_some_and(|sequence| sequence <= ledger_position)
            }),
            &accounting,
        )?;
        let mut latest = BTreeMap::<(&str, &str), (u64, &SeenDispatch)>::new();
        for dispatch in self.seen_dispatches.values() {
            let Some(task_id) = accounted(dispatch.data.task_id.as_str()) else {
                continue;
            };
            let Some(sequence) = dispatch
                .admitted_sequence
                .filter(|sequence| *sequence <= ledger_position)
            else {
                continue;
            };
            let key = (task_id, dispatch.data.instance_id.as_str());
            if latest
                .get(&key)
                .is_none_or(|(previous, _)| *previous < sequence)
            {
                latest.insert(key, (sequence, dispatch));
            }
        }
        let mut snapshots = latest
            .into_iter()
            .map(|((task_id, instance_id), (_, dispatch))| {
                let admission = dispatch.admission.as_ref().ok_or_else(|| {
                    fatal(
                        HostCode::PolicyDispatchAdmissionMissing,
                        HostLocation::ProjectPolicyTaskState,
                    )
                })?;
                let (terminal_state, completed_window) = if dispatch
                    .completed_sequence
                    .is_some_and(|sequence| sequence <= ledger_position)
                {
                    let execution = dispatch.execution.as_ref().ok_or_else(|| {
                        fatal(
                            HostCode::PolicyExecutionOutcomeMissing,
                            HostLocation::ProjectPolicyTaskState,
                        )
                    })?;
                    let terminal_state = match execution.outcome {
                        PolicyExecutionOutcome::Succeeded { .. } => TaskTerminalState::Succeeded,
                        PolicyExecutionOutcome::Failed { .. } => TaskTerminalState::Failed,
                    };
                    (
                        Some(terminal_state),
                        Some(CompletedActivityWindow {
                            window_id: admission.activity.window_id.clone(),
                            completed_at_unix_ms: execution.observed_at_unix_ms,
                        }),
                    )
                } else {
                    (None, None)
                };
                let settlement = settlements.get(&(task_id.to_owned(), instance_id.to_owned()));
                Ok((
                    (task_id.to_owned(), instance_id.to_owned()),
                    TaskRuntimeSnapshot {
                        task_id: task_id.to_owned(),
                        instance_id: instance_id.to_owned(),
                        last_dispatched_unix_ms: Some(admission.activity.admitted_at_unix_ms),
                        eligible_since_unix_ms: None,
                        terminal_state,
                        completed_window,
                        last_duration_ms: settlement.map(|settlement| {
                            settlement.duration_ms.min(MAX_TASK_LAST_DURATION_MS)
                        }),
                        failure_streak: settlement.map(|settlement| {
                            u16::try_from(settlement.failure_streak)
                                .unwrap_or(u16::MAX)
                                .min(MAX_TASK_FAILURE_STREAK)
                        }),
                    },
                ))
            })
            .collect::<RuntimeHostResult<BTreeMap<_, _>>>()?;
        if let Some(as_of_unix_ms) = eligibility_as_of_unix_ms {
            for ((task_id, instance_id), since) in &self.eligibility.since {
                if *since >= as_of_unix_ms || !task_ids.contains(task_id.as_str()) {
                    continue;
                }
                snapshots
                    .entry((task_id.clone(), instance_id.clone()))
                    .or_insert_with(|| TaskRuntimeSnapshot {
                        task_id: task_id.clone(),
                        instance_id: instance_id.clone(),
                        last_dispatched_unix_ms: None,
                        eligible_since_unix_ms: None,
                        terminal_state: None,
                        completed_window: None,
                        last_duration_ms: None,
                        failure_streak: None,
                    })
                    .eligible_since_unix_ms = Some(*since);
            }
        }
        Ok(snapshots.into_values().collect())
    }

    /// Applies the previous evaluated cycle's eligibility verdicts (Workflow #308 slice 5b).
    /// Called once when an evaluation starts, before its inputs are projected.
    pub(crate) fn apply_eligibility_verdicts(&mut self) {
        self.eligibility.apply_pending();
    }

    /// The pairs the last evaluation started aging while no cycle's verdicts had been applied
    /// since the host opened: their age before this host is unknown. Empty afterwards.
    pub(crate) fn eligibility_unknown_pairs(&self) -> BTreeSet<(String, String)> {
        match (&self.eligibility.pending, self.eligibility.known) {
            (Some(pending), false) => pending.waiting.clone(),
            _ => BTreeSet::new(),
        }
    }

    /// The settlement of one pair's latest executed run, if it has one.
    pub(crate) fn latest_settlement(
        &self,
        task_id: &str,
        instance_alias: &str,
    ) -> RuntimeHostResult<Option<PolicySettlement>> {
        Ok(
            fold_settlements(self.seen_dispatches.values().filter(|dispatch| {
                dispatch.data.task_id == task_id && dispatch.data.instance_id == instance_alias
            }))?
            .remove(&(task_id.to_owned(), instance_alias.to_owned())),
        )
    }

    /// The latest settlement of the pair `decision_id` dispatched to, once that dispatch has
    /// an execution outcome.
    pub(crate) fn latest_settlement_for_decision(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<Option<PolicySettlement>> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            request(
                HostCode::PolicyDispatchUnknown,
                HostLocation::ReadPolicySettlement,
            )
        })?;
        if dispatch.execution.is_none() {
            return Ok(None);
        }
        self.latest_settlement(&dispatch.data.task_id, &dispatch.data.instance_id)
    }

    /// The settlement of every pair's latest executed run.
    pub(crate) fn latest_settlements(&self) -> RuntimeHostResult<Vec<PolicySettlement>> {
        Ok(fold_settlements(self.seen_dispatches.values())?
            .into_values()
            .collect())
    }

    /// Workflow #369 E3 (#670 safety net): leaves a dispatch the start could not settle open,
    /// out of the pending settlements and the active workloads.
    pub(crate) fn quarantine_dispatch(&mut self, decision_id: &str) {
        self.quarantined_dispatches.insert(decision_id.to_owned());
    }

    /// Coordinator ruling on #670: whether a run of the dispatch `decision_id` that ran
    /// `runtime_ms` exceeded its admitted runtime budget, by the policy's own rule.
    pub(crate) fn runtime_budget_exceeded(
        &self,
        decision_id: &str,
        runtime_ms: u64,
    ) -> RuntimeHostResult<bool> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchUnknown,
                HostLocation::ReadPolicyRuntimeBudget,
            )
        })?;
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchAdmissionMissing,
                HostLocation::ReadPolicyRuntimeBudget,
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
        crate::policy_control::runtime_budget_exceeded(&intent, admission, runtime_ms)
    }

    pub(crate) fn pending_dispatch_completions(&self) -> Vec<String> {
        self.seen_dispatches
            .iter()
            .filter(|(decision_id, dispatch)| {
                dispatch.lifecycle == DispatchLifecycle::Admitted
                    && dispatch.execution.is_some()
                    && !self.quarantined_dispatches.contains(*decision_id)
            })
            .map(|(decision_id, _)| decision_id.clone())
            .collect()
    }

    /// Workflow #369 E3 (#670 final review): the procedure ref of a dispatch's task in the
    /// catalog generation the dispatch pinned, which a package rebind does not change.
    pub(crate) fn dispatch_procedure_ref(&self, decision_id: &str) -> RuntimeHostResult<String> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchUnknown,
                HostLocation::ReadDispatchProcedureRef,
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        catalog
            .compiled
            .dispatch_task(&dispatch.data.task_id)
            .map(|task| task.procedure_ref.clone())
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyTaskMissing,
                    HostLocation::ReadDispatchProcedureRef,
                )
            })
    }

    pub(crate) fn pending_dispatch_outcomes(&self) -> Vec<String> {
        self.seen_dispatches
            .iter()
            .filter(|(decision_id, dispatch)| {
                dispatch.lifecycle == DispatchLifecycle::Admitted
                    && dispatch.execution.is_none()
                    && !self.quarantined_dispatches.contains(*decision_id)
            })
            .map(|(decision_id, _)| decision_id.clone())
            .collect()
    }

    pub(crate) fn prepare_active_transaction(
        &self,
        catalog: &LoadedCatalog,
        expected_active_hash: Option<&str>,
    ) -> Box<dyn actingcommand_ledger::LedgerTransactionWork> {
        Box::new(
            self.store.prepare_transition(
                catalog,
                expected_active_hash,
                self.active
                    .as_ref()
                    .map(|value| value.generation.catalog_hash.as_str())
                    == expected_active_hash,
            ),
        )
    }

    pub(crate) fn publish_active(&mut self, catalog: LoadedCatalog) {
        self.lineage.record(
            &catalog.generation.catalog_id,
            catalog.generation.catalog_version,
            &catalog.generation.catalog_hash,
        );
        self.active = Some(catalog);
        self.cadence.catalog_changed();
    }

    /// Workflow #361 A: every generation that was active in this ledger.
    pub(crate) fn lineage(&self) -> &CatalogLineage {
        &self.lineage
    }

    /// Workflow #386 SR-6b, MCP-19a (register §1.143): the latest evaluated cycle's not-run
    /// explanation for `instance` (every instance when absent), with the selected decision's
    /// dispatch state as the dispatch projection holds it now: `pending` before its intent is
    /// recorded.
    pub(crate) fn schedule_explanation(
        &self,
        instance: Option<&str>,
        candidates: bool,
    ) -> actingcommand_contract::RuntimeScheduleExplanation {
        use actingcommand_contract::ScheduleDispatchState;
        self.explanation
            .answer(instance, candidates, |decision_id| {
                match self
                    .seen_dispatches
                    .get(decision_id)
                    .map(|dispatch| dispatch.lifecycle)
                {
                    None => ScheduleDispatchState::Pending,
                    Some(DispatchLifecycle::Intent) => ScheduleDispatchState::Intent,
                    Some(DispatchLifecycle::Admitted) => ScheduleDispatchState::Admitted,
                    Some(DispatchLifecycle::Rejected) => ScheduleDispatchState::Rejected,
                    Some(DispatchLifecycle::Completed) => ScheduleDispatchState::Completed,
                }
            })
    }

    pub(crate) fn evaluate(
        &mut self,
        facts: &EvaluationFacts,
        resources: &EvaluationResources,
        context: PolicyEvaluationContext<'_>,
    ) -> RuntimeHostResult<PolicyCycle> {
        let PolicyEvaluationContext {
            procedure_manifest,
            time,
            seed,
            trigger,
            sampled_at_monotonic_ms,
            scheduling_pause,
            instance_occupancy,
            suspension_lift,
            pack_admission,
            fact_refresh,
            remedy,
        } = context;
        let directive = self.cadence.observe(trigger, time.unix_ms)?;
        if directive.kind == PolicyRecomputeKind::Deferred {
            return Ok(PolicyCycle {
                directive,
                evaluation: None,
                pending_dispatch_intents: Vec::new(),
                detection_planning_signals: Vec::new(),
                clock_slot_signals: Vec::new(),
                measurement: None,
                fact_refresh_transitions: Vec::new(),
            });
        }
        // Workflow #411 B15 R4 (model-b15 §9.1): the remedy table of the active catalog, built at
        // the first evaluation under it, and again at most once per interval while a round is
        // open or a stop is recorded.
        if let Some(active) = &self.active
            && self.remedies.table_due(
                active.generation.catalog_hash(),
                time.unix_ms,
                remedy.table_interval_ms,
            )
        {
            let table = crate::remedy::build_table(
                &active.compiled,
                Some(procedure_manifest),
                &facts.instances,
                time.unix_ms,
            );
            self.remedies.set_table(table);
        }
        let active = self.active.as_ref().ok_or_else(|| {
            request(
                SharedCode::PolicyCatalogUnavailable,
                HostLocation::EvaluatePolicyCycle,
            )
        })?;
        let cost = policy_evaluation_cost(&active.compiled, facts, resources)?;
        // Workflow #336 R22: the pairs a failure identity schedules for an immediate rerun.
        let immediate_retries = self
            .control
            .immediate_retry_pairs(&active.compiled, time.unix_ms)?;
        // Workflow #384 D2-S4: the fact refresh's readers, holds and settings for this cycle.
        let refresh_inputs = self.fact_refresh_inputs(
            &active.compiled,
            facts,
            &fact_refresh,
            scheduling_pause,
            instance_occupancy,
            time.unix_ms,
        )?;
        // Workflow #411 B15 R4 (model-b15 §9.2): the remedy plan of this cycle; empty when no
        // bound package declares a remedy or remedies are off.
        let (remedy_plan, remedy_decisions) =
            self.remedy_plan(&active.compiled, facts, time, remedy)?;
        let deferrals = clock_slot::Deferrals::default();
        let started = Instant::now();
        let mut evaluation = evaluate_with_remedies::<RuntimeHostError>(
            &active.compiled,
            facts,
            resources,
            time,
            seed,
            &immediate_retries,
            &refresh_inputs,
            &remedy_plan,
            deferrals.capture(|intent| {
                // Workflow #384 D2-S4: a fact refresh's deferrals name their gate and class.
                let refresh = intent.task_id == FACT_REFRESH_TASK_ID;
                // An operator scheduling pause, then a held instance or a takeover cooldown
                // (Workflow #191 ps1, #369 W-1).
                if let Some(hold) = crate::fact_refresh::instance_hold(
                    &intent.instance_id,
                    scheduling_pause(&intent.instance_id),
                    instance_occupancy(&intent.instance_id),
                ) {
                    return Ok(hold.eligibility(refresh));
                }
                // Workflow #386 SR-3 (scheduling model §2.4 item 3; `scheduling-blackout.md`,
                // "Order of the deferrals", position 3): the task package is suspended.
                if let Some(reason) = pack_admission.deferral(suspension_lift, intent, time.unix_ms)?
                {
                    return Ok(CandidateEligibility::Deferred {
                        reason,
                        next_wake_unix_ms: None,
                    });
                }
                // Workflow #411 B15 R4 (`scheduling-blackout.md`, "Order of the deferrals",
                // position 8a): a remedy round holds the task, or stopped it for a person, after
                // positions 1 to 7 and before availability.
                if let Some(deferral) = remedy_plan.deferral(&intent.task_id, &intent.instance_id) {
                    return Ok(CandidateEligibility::Deferred {
                        reason: deferral.reason.clone(),
                        next_wake_unix_ms: deferral.next_wake_unix_ms,
                    });
                }
                match self.control.preview_admission_lifted(
                    &active.compiled,
                    intent,
                    time.unix_ms,
                    Some(suspension_lift),
                ) {
                    Ok(_) => Ok(CandidateEligibility::Eligible),
                    Err(error) if is_availability_denial(&error) => {
                        let rejection = error.policy_rejection();
                        deferrals.note_budget(&rejection);
                        let reason = DecisionReason {
                            // The denial's own outcome: its host code with its values.
                            outcome: rejection.outcome.clone(),
                            detail: format!(
                                "Runtime control availability: budget={:?}; next_eligible_unix_ms={:?}",
                                rejection.budget, rejection.next_eligible_unix_ms
                            ),
                        };
                        match Code::relayed(error.code()).filter(|_| refresh) {
                            Some(gate) => Ok(crate::fact_refresh::refresh_denial(
                                gate,
                                rejection.budget.as_ref().map(|budget| budget.dimension),
                                reason,
                                rejection.next_eligible_unix_ms,
                            )),
                            None => Ok(CandidateEligibility::Deferred {
                                reason,
                                next_wake_unix_ms: rejection.next_eligible_unix_ms,
                            }),
                        }
                    }
                    Err(error) => Err(error),
                }
            }),
        )?;
        procedure_manifest.bind_evaluation(&mut evaluation)?;
        // Workflow #411 B15 R4 (model-b15 §7.5 to §7.7): the stops, lifts and expiries the plan
        // and the evaluation decided, each checked against the planning-signal projection before
        // the cycle's recorder writes it (once per id, across restarts too).
        let mut remedy_signals = Vec::new();
        for signal in remedy_plan.signals_after(&evaluation) {
            if self
                .store
                .load_planning_signal(&signal.signal_id)?
                .is_none()
            {
                remedy_signals.push(crate::remedy::signal_event(signal, time.unix_ms));
            }
        }
        let remedy_active = !remedy_plan.overrides.is_empty()
            || !remedy_plan.deferrals.is_empty()
            || !remedy_signals.is_empty();
        let elapsed_micros = u64::try_from(started.elapsed().as_micros()).map_err(|_| {
            fatal(
                HostCode::PolicyEvaluationMeasurementOverflow,
                HostLocation::MeasurePolicyEvaluation,
            )
        })?;
        let pending_dispatch_intents: Vec<DispatchIntent> = evaluation
            .dispatch_intents
            .iter()
            .filter(|intent| !self.seen_dispatches.contains_key(&intent.decision_id))
            .cloned()
            .collect();
        let detection_planning_signals = preview_detection_planning_signals(
            &self.store,
            &self.detection_quota,
            &active.compiled,
            facts,
            &evaluation,
            &pending_dispatch_intents,
            time.unix_ms,
        )?;
        let in_flight = self
            .seen_dispatches
            .values()
            .filter(|dispatch| {
                matches!(
                    dispatch.lifecycle,
                    DispatchLifecycle::Intent | DispatchLifecycle::Admitted
                )
            })
            .map(|dispatch| {
                (
                    dispatch.data.task_id.as_str(),
                    dispatch.data.instance_id.as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        let clock_slot_signals = clock_slot::preview(
            &self.store,
            &active.compiled,
            facts,
            resources,
            &evaluation,
            &pending_dispatch_intents,
            &in_flight,
            deferrals,
            time,
        )?;
        self.eligibility
            .stage(&evaluation, time.unix_ms, &in_flight);
        self.arbitration
            .extend(instance_arbitration_ranks(&evaluation));
        // Workflow #386 SR-6b, MCP-19a (register §1.143): the not-run explanation of this
        // evaluation, before the host adds its notes to the decisions.
        self.explanation
            .record(&evaluation, active.compiled.catalog_hash(), time.unix_ms);
        if let Some(decisions) = &remedy_decisions {
            self.remedies.retain_keys(decisions);
        }
        self.remedies
            .finish_evaluation(remedy_active, remedy_signals);
        let requested_recompute = directive.kind;
        Ok(PolicyCycle {
            directive,
            evaluation: Some(evaluation),
            pending_dispatch_intents,
            detection_planning_signals,
            clock_slot_signals,
            measurement: Some(PolicyEvaluationMeasurement {
                sampled_at_monotonic_ms,
                elapsed_micros,
                requested_recompute,
                execution: PolicyEvaluationExecution::FullCatalogScan,
                cost,
            }),
            // The yield point adds the transitions (`evaluate_policy_cycle_authoritative`).
            fact_refresh_transitions: Vec::new(),
        })
    }

    /// Workflow #384 D2-S4 (D2 model §3.4): the fact refresh inputs of a cycle: the kept
    /// recognition maintenance package of every instance in the snapshot, with the fact refresh
    /// dispatches admitted on it in its current activity day and the code of its latest failure
    /// while no success followed it; the instances the closure's positions 1-2 hold; the
    /// settings in effect; L.
    fn fact_refresh_inputs(
        &self,
        catalog: &CompiledCatalog,
        facts: &EvaluationFacts,
        context: &FactRefreshContext<'_>,
        scheduling_pause: &dyn Fn(&str) -> Option<HostCode>,
        instance_occupancy: &dyn Fn(&str) -> Option<InstanceOccupancy>,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<FactRefreshInputs> {
        let profiles = &catalog.catalog().activity.profiles;
        let mut readers = Vec::new();
        let mut instance_holds = BTreeMap::new();
        for instance in &facts.instances {
            if let Some(hold) = crate::fact_refresh::instance_hold(
                &instance.instance_id,
                scheduling_pause(&instance.instance_id),
                instance_occupancy(&instance.instance_id),
            ) {
                instance_holds.insert(instance.instance_id.clone(), hold.policy_hold());
            }
            let Some(binding) = context.bindings.get(&instance.instance_id) else {
                continue;
            };
            let window = match matching_activity_profile(profiles, instance) {
                Some(profile) => activity_window_at(profile, now_unix_ms).map_err(|_| {
                    fatal(
                        HostCode::PolicyActivityDayOverflow,
                        HostLocation::ResolveActivityWindow,
                    )
                })?,
                None => None,
            };
            // A binding whose marks the start could not read serves no key.
            let (package_id, marked_keys, problem) = match &binding.marks {
                Some((package_id, keys)) => (package_id.clone(), keys.clone(), None),
                None => (
                    String::new(),
                    BTreeSet::new(),
                    Some(actingcommand_contract::ReaderProblem::PackageUnreadable),
                ),
            };
            readers.push(FactRefreshReader {
                instance_id: instance.instance_id.clone(),
                package_id,
                marked_keys,
                problem,
                dispatched_today: window.map_or(0, |window| {
                    self.control.task_daily_count(
                        FACT_REFRESH_TASK_ID,
                        &instance.instance_id,
                        window.local_day,
                    )
                }),
                last_failure_code: self
                    .control
                    .latest_failure_code(FACT_REFRESH_TASK_ID, &instance.instance_id),
            });
        }
        Ok(FactRefreshInputs {
            settings: context.settings,
            readers,
            instance_facts_loaded_at_unix_ms: Some(context.instance_facts_loaded_at_unix_ms),
            instance_holds,
        })
    }

    /// Workflow #411 B15 R4 (model-b15 §9.2): the remedy plan at `time` on `facts` (an
    /// evaluation's, or an admission's for its check), and the decisions of the runs its journal
    /// holds, whose outcome keys are kept (`None` with remedies off, which keeps every key). With
    /// remedies off or an empty table the plan is empty, so the evaluation is unchanged.
    fn remedy_plan(
        &self,
        catalog: &CompiledCatalog,
        facts: &EvaluationFacts,
        time: EvaluationTime,
        remedy: &crate::remedy::RemedyContext,
    ) -> RuntimeHostResult<(RemedyPlan, Option<BTreeSet<String>>)> {
        if !remedy.enabled {
            return Ok((RemedyPlan::default(), None));
        }
        let Some(table) = self
            .remedies
            .table()
            .filter(|table| !table.table.is_empty())
        else {
            return Ok((RemedyPlan::default(), Some(BTreeSet::new())));
        };
        let journal = self.remedy_journal(table)?;
        let decisions = journal
            .iter()
            .flat_map(|pair| pair.runs.iter().map(|run| run.decision_id.clone()))
            .collect();
        let inputs = RemedyInputs {
            enabled: remedy.enabled,
            max_per_round: remedy.max_per_round,
            table: table.table.clone(),
            journal,
            stops: self.remedies.stops(),
            signals_seen: self.remedies.seen().clone(),
            configuration: remedy.configuration.clone(),
            persistent_gates: crate::remedy::persistent_gates(),
        };
        Ok((
            plan_remedies(catalog, facts, time, &inputs)?,
            Some(decisions),
        ))
    }

    /// Workflow #411 B15 R4 (model-b15 §7.8): the remedy journal. Per (task, instance) of every
    /// task in `table` (one that declares a remedy or provides a capability), its admitted runs in
    /// ledger order since the pair's latest root blocked run, at most
    /// [`REMEDY_JOURNAL_MAX_RUNS`] (the latest): each with its decision, its admission time, the
    /// end of the activity window it was admitted in, the `policy_remedy_requested` and
    /// `policy_remedy_retry` reasons of its chain, and how it settled (a success with the outcome
    /// key its terminal names, a failure, or not yet). The runs are this projection's, rebuilt in
    /// ledger order at start; the keys are the remedy state's.
    fn remedy_journal(
        &self,
        table: &crate::remedy::BuiltTable,
    ) -> RuntimeHostResult<Vec<RemedyJournal>> {
        let tasks = table.journal_tasks();
        let mut pairs = BTreeMap::<(&str, &str), Vec<(u64, &str, &SeenDispatch)>>::new();
        for (decision_id, dispatch) in &self.seen_dispatches {
            let Some(sequence) = dispatch.admitted_sequence else {
                continue;
            };
            if !tasks.contains(dispatch.data.task_id.as_str()) {
                continue;
            }
            pairs
                .entry((
                    dispatch.data.task_id.as_str(),
                    dispatch.data.instance_id.as_str(),
                ))
                .or_default()
                .push((sequence, decision_id.as_str(), dispatch));
        }
        let mut journals = Vec::with_capacity(pairs.len());
        for ((task_id, instance_id), mut runs) in pairs {
            runs.sort_by_key(|(sequence, _, _)| *sequence);
            let root = runs
                .iter()
                .rposition(|(_, decision_id, dispatch)| {
                    self.is_root_blocked_run(table, task_id, decision_id, dispatch)
                })
                .unwrap_or(0);
            let from = root.max(runs.len().saturating_sub(REMEDY_JOURNAL_MAX_RUNS));
            let runs = runs[from..]
                .iter()
                .map(|(_, decision_id, dispatch)| self.remedy_run(decision_id, dispatch))
                .collect::<RuntimeHostResult<Vec<_>>>()?;
            journals.push(RemedyJournal {
                task_id: task_id.to_owned(),
                instance_id: instance_id.to_owned(),
                runs,
            });
        }
        Ok(journals)
    }

    /// Workflow #411 B15 R4 (model-b15 §7.1): whether a run is a root blocked run: it succeeded on
    /// a key whose mapping declares a remedy, and its chain names no round.
    fn is_root_blocked_run(
        &self,
        table: &crate::remedy::BuiltTable,
        task_id: &str,
        decision_id: &str,
        dispatch: &SeenDispatch,
    ) -> bool {
        matches!(
            dispatch
                .execution
                .as_ref()
                .map(|execution| &execution.outcome),
            Some(PolicyExecutionOutcome::Succeeded { .. })
        ) && self
            .remedies
            .outcome_key(decision_id)
            .is_some_and(|outcome_key| table.remedy_key(task_id, outcome_key))
            && !dispatch
                .data
                .reasons
                .iter()
                .any(|reason| crate::remedy::is_round_reason(&reason.0))
    }

    /// Workflow #411 B15 R4 (model-b15 §7.8): one admitted run of the remedy journal.
    fn remedy_run(
        &self,
        decision_id: &str,
        dispatch: &SeenDispatch,
    ) -> RuntimeHostResult<RemedyRun> {
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchAdmissionMissing,
                HostLocation::EvaluatePolicyCycle,
            )
        })?;
        let admitted_at_unix_ms = admission.activity.admitted_at_unix_ms;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let profile = catalog
            .compiled
            .catalog()
            .activity
            .profiles
            .iter()
            .find(|profile| profile.id == admission.activity.profile_id)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyActivityProfileMissing,
                    HostLocation::ResolveActivityWindow,
                )
            })?;
        let window = activity_window_at(profile, admitted_at_unix_ms)
            .map_err(|_| {
                fatal(
                    HostCode::PolicyActivityDayOverflow,
                    HostLocation::ResolveActivityWindow,
                )
            })?
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyActivityWindowClosed,
                    HostLocation::ResolveActivityWindow,
                )
            })?;
        let settled = match dispatch
            .execution
            .as_ref()
            .map(|execution| &execution.outcome)
        {
            None => RemedySettlement::InFlight,
            Some(PolicyExecutionOutcome::Succeeded { .. }) => RemedySettlement::Succeeded {
                outcome_key: self.remedies.outcome_key(decision_id).map(str::to_owned),
            },
            Some(PolicyExecutionOutcome::Failed { .. }) => RemedySettlement::Failed,
        };
        Ok(RemedyRun {
            decision_id: decision_id.to_owned(),
            admitted_at_unix_ms,
            activity_window_end_unix_ms: window.until_unix_ms,
            remedy_reasons: dispatch
                .data
                .reasons
                .iter()
                .map(|reason| &reason.0)
                .filter(|outcome| crate::remedy::is_round_reason(outcome))
                .cloned()
                .collect(),
            settled,
            settled_at_unix_ms: dispatch
                .execution
                .as_ref()
                .map(|execution| execution.observed_at_unix_ms),
        })
    }

    /// Workflow #411 B15 R4 (model-b15 §7.9): an intent whose chain names a remedy round
    /// (`policy_remedy_requested`, `policy_remedy_retry`) is admitted only while every round and
    /// layer it names is still open and not yet served: the plan at the admission time, on the
    /// admission's facts, still gives the intent's pair a trigger override with a reason of the
    /// same code, blocked task, round and layer. Otherwise it is refused
    /// `policy_remedy_round_closed`. An intent that names no round is not checked.
    pub(crate) fn validate_remedy_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        facts: &EvaluationFacts,
        remedy: &crate::remedy::RemedyContext,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<()> {
        let named = reason_chain
            .reasons
            .iter()
            .map(|reason| &reason.outcome)
            .filter(|outcome| crate::remedy::is_round_reason(outcome))
            .collect::<Vec<_>>();
        if named.is_empty() {
            return Ok(());
        }
        let active = self.active.as_ref().ok_or_else(|| {
            request(
                SharedCode::PolicyCatalogUnavailable,
                HostLocation::ValidatePolicyDispatch,
            )
        })?;
        let (plan, _) = self.remedy_plan(
            &active.compiled,
            facts,
            EvaluationTime {
                unix_ms: now_unix_ms,
                monotonic_ms: now_unix_ms,
            },
            remedy,
        )?;
        let current = plan.trigger_override(&intent.task_id, &intent.instance_id);
        for outcome in named {
            let open = current.is_some_and(|current| {
                current
                    .reasons()
                    .iter()
                    .any(|reason| crate::remedy::same_layer(&reason.outcome, outcome))
            });
            if !open {
                return Err(crate::remedy::round_closed(outcome, &intent.instance_id));
            }
        }
        Ok(())
    }

    /// Workflow #411 B15 R4 (model-b15 §9.1, at start): builds the remedy table of the active
    /// catalog from `manifest`'s package locations.
    pub(crate) fn build_remedy_table(
        &mut self,
        manifest: Option<&ProcedureManifest>,
        instances: &[InstanceSnapshot],
        now_unix_ms: u64,
    ) {
        if let Some(active) = &self.active {
            let table =
                crate::remedy::build_table(&active.compiled, manifest, instances, now_unix_ms);
            self.remedies.set_table(table);
        }
    }

    /// Workflow #411 B15 R4: whether `task_id`'s package declares a remedy, so its settled run's
    /// terminal key is read (model-b15 §7.8).
    pub(crate) fn declares_remedies(&self, task_id: &str) -> bool {
        self.remedies.declares_remedies(task_id)
    }

    /// Workflow #411 B15 R4: whether any bound package declares a remedy.
    pub(crate) fn declares_any_remedy(&self) -> bool {
        self.remedies
            .table()
            .is_some_and(|table| !table.table.remedies.is_empty())
    }

    /// Workflow #411 B15 R4: whether `task_id`'s mapping of `outcome_key` declares a remedy.
    pub(crate) fn remedy_root_key(&self, task_id: &str, outcome_key: &str) -> bool {
        self.remedies
            .table()
            .is_some_and(|table| table.remedy_key(task_id, outcome_key))
    }

    /// Workflow #411 B15 R4: the outcome key the terminal of `decision_id`'s run names.
    pub(crate) fn note_remedy_key(&mut self, decision_id: &str, outcome_key: String) {
        self.remedies.note_key(decision_id, outcome_key);
    }

    /// Workflow #411 B15 R4: a recorded planning signal; a `signal:remedy:` one updates the
    /// remedy state (seen, a stop, a lift).
    pub(crate) fn observe_remedy_signal(
        &mut self,
        signal_id: &str,
        instance_id: &str,
        task_id: Option<&str>,
        outcome: &Outcome,
    ) {
        self.remedies
            .observe_signal(signal_id, instance_id, task_id, outcome);
    }

    /// Workflow #411 B15 R4: the remedy signals the last evaluation decided, for its recorder.
    pub(crate) fn take_remedy_signals(&mut self) -> Vec<PolicyPlanningSignalEventData> {
        self.remedies.take_pending()
    }

    /// Workflow #411 B15 R4 (model-b15 §7.8, at start): per pair of a task that declares a
    /// remedy, its admitted runs newest first, at most [`REMEDY_JOURNAL_MAX_RUNS`], each with
    /// whether its terminal is read (a completed success) and whether its chain names a round.
    pub(crate) fn remedy_key_reads(&self) -> Vec<Vec<crate::remedy::RemedyKeyRead>> {
        let Some(table) = self.remedies.table() else {
            return Vec::new();
        };
        let mut pairs = BTreeMap::<(&str, &str), Vec<(u64, &str, &SeenDispatch)>>::new();
        for (decision_id, dispatch) in &self.seen_dispatches {
            let Some(sequence) = dispatch.admitted_sequence else {
                continue;
            };
            if !table.declares_remedies(&dispatch.data.task_id) {
                continue;
            }
            pairs
                .entry((
                    dispatch.data.task_id.as_str(),
                    dispatch.data.instance_id.as_str(),
                ))
                .or_default()
                .push((sequence, decision_id.as_str(), dispatch));
        }
        pairs
            .into_values()
            .map(|mut runs| {
                runs.sort_by_key(|(sequence, _, _)| std::cmp::Reverse(*sequence));
                runs.into_iter()
                    .take(REMEDY_JOURNAL_MAX_RUNS)
                    .map(|(_, decision_id, dispatch)| crate::remedy::RemedyKeyRead {
                        decision_id: decision_id.to_owned(),
                        task_id: dispatch.data.task_id.clone(),
                        read: dispatch.completed_sequence.is_some()
                            && matches!(
                                dispatch
                                    .execution
                                    .as_ref()
                                    .map(|execution| &execution.outcome),
                                Some(PolicyExecutionOutcome::Succeeded { .. })
                            ),
                        round_run: dispatch
                            .data
                            .reasons
                            .iter()
                            .any(|reason| crate::remedy::is_round_reason(&reason.0)),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Workflow #411 B15 R4 (at start): the run identity of the completed dispatch `decision_id`,
    /// read from its `policy.dispatch_completed` record, by which its terminal is found.
    pub(crate) fn remedy_completed_run(
        &self,
        ledger: &GlobalLedger,
        decision_id: &str,
    ) -> RuntimeHostResult<CompletedPolicyRunIdentity> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchUnknown,
                HostLocation::RecoverPolicySchedulingOutcomes,
            )
        })?;
        let (Some(sequence), Some(execution)) =
            (dispatch.completed_sequence, dispatch.execution.as_ref())
        else {
            return Err(fatal(
                HostCode::PolicyDispatchCompletionIncomplete,
                HostLocation::RecoverPolicySchedulingOutcomes,
            ));
        };
        let events = ledger
            .query_page(
                EventQuery {
                    from_sequence: Some(sequence),
                    to_sequence: Some(sequence),
                    event_type: Some(EventType::PolicyDispatchCompleted),
                    ..EventQuery::default()
                },
                sequence.saturating_sub(1),
                sequence,
                2,
            )
            .map_err(|_| {
                fatal(
                    HostCode::PolicyRecoveryFailed,
                    HostLocation::RecoverPolicySchedulingOutcomes,
                )
            })?;
        let [event] = events.as_slice() else {
            return Err(fatal(
                HostCode::PolicyDispatchCompletionIncomplete,
                HostLocation::RecoverPolicySchedulingOutcomes,
            ));
        };
        completed_policy_run_identity(event, &dispatch.data, execution, BTreeSet::new())
    }

    /// Workflow #411 B15 R4, test only: the remedy table in effect.
    #[cfg(test)]
    pub(crate) fn remedy_table_for_test(&self) -> Option<crate::remedy::BuiltTable> {
        self.remedies.table().cloned()
    }

    pub(crate) fn validate_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        owner_epoch: OwnerEpoch,
        ledger_high_watermark: u64,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let active = self.active.as_ref().ok_or_else(|| {
            request(
                SharedCode::PolicyCatalogUnavailable,
                HostLocation::ValidatePolicyDispatch,
            )
        })?;
        if intent.catalog_hash != active.generation.catalog_hash
            || intent.catalog_version != active.generation.catalog_version
        {
            return Err(request(
                HostCode::PolicyCatalogMismatch,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        // Workflow #384 D2-S4: a fact refresh dispatch is checked against the Runtime-owned spec.
        let task = active
            .compiled
            .dispatch_task(&intent.task_id)
            .ok_or_else(|| {
                request(
                    HostCode::PolicyTaskMissing,
                    HostLocation::ValidatePolicyDispatch,
                )
            })?;
        let descriptor = &active.compiled.catalog().tasks.catalog;
        if task.entrypoint.operation_id != intent.operation_id
            || task.procedure_ref != intent.procedure_ref
            || task.expected_duration_ms != intent.expected_duration_ms
            || task.yield_points != intent.yield_points
            || task.load_profile != intent.load_profile
            || task.loop_budget.daily_limit != intent.prerequisites.daily_limit
            || task.loop_budget.window_iteration_limit
                != intent.prerequisites.window_iteration_limit
            || task.loop_budget.max_runtime_ms != intent.prerequisites.max_runtime_ms
            || intent.prerequisites.urgency_milli > 1_000
            || !active
                .compiled
                .catalog()
                .activity
                .profiles
                .iter()
                .any(|profile| profile.id == intent.prerequisites.activity_profile_id)
            || descriptor.approval_refs != intent.approval_refs
            || matches!(
                &task.scope,
                ScopeSelector::Instance { instance_id } if instance_id != &intent.instance_id
            )
        {
            return Err(request(
                HostCode::PolicyIntentCatalogMismatch,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        if intent.input_ledger_position != context.fact_ledger_position
            || intent.input_ledger_position > ledger_high_watermark
            || intent.fact_snapshot_id != context.fact_snapshot_id
        {
            return Err(request(
                HostCode::PolicyFactPositionMismatch,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        if reason_chain.id != intent.reason_chain_id
            || reason_chain.decision_id != intent.decision_id
            || reason_chain.reasons.is_empty()
        {
            return Err(request(
                HostCode::PolicyReasonChainMismatch,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        if reason_chain.reasons.iter().any(|reason| {
            reason
                .outcome
                .is(actingcommand_policy::PolicyCode::FailureRetryImmediate)
        }) {
            self.control
                .validate_immediate_retry(&active.compiled, intent, context.now_unix_ms)?;
        }
        if intent
            .approval_refs
            .iter()
            .any(|approval| !context.approval_fact_ids.contains(approval))
        {
            return Err(request(
                HostCode::PolicyApprovalFactMissing,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        if context.fencing_owner_epoch != owner_epoch || !intent.prerequisites.fencing_required {
            return Err(request(
                HostCode::PolicyFencingMismatch,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        if context.now_unix_ms < intent.prerequisites.evaluated_at_unix_ms
            || intent
                .prerequisites
                .facts_fresh_until_unix_ms
                .is_some_and(|expires| context.now_unix_ms > expires)
        {
            return Err(request(
                HostCode::PolicyFactsStale,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        if intent.prerequisites.daily_limit == 0
            || intent.prerequisites.window_iteration_limit == 0
            || intent.prerequisites.max_runtime_ms == 0
            || intent.expected_duration_ms > intent.prerequisites.max_runtime_ms
        {
            return Err(request(
                HostCode::PolicyBudgetExhausted,
                HostLocation::ValidatePolicyDispatch,
            ));
        }
        Ok(active.generation.clone())
    }

    pub(crate) fn replay_admission(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<Option<PolicyDispatchAdmission>> {
        let Some(seen) = self.seen_dispatches.get(&intent.decision_id) else {
            return Ok(None);
        };
        if seen.data != dispatch_event_data(intent, reason_chain)? {
            return Err(fatal(
                HostCode::PolicyDecisionIdentityConflict,
                HostLocation::ReplayPolicyDispatch,
            ));
        }
        let catalog = self
            .store
            .load_generation(&seen.data.catalog_hash)?
            .generation
            .clone();
        Ok(Some(PolicyDispatchAdmission::ReplaySuppressed {
            decision_id: intent.decision_id.clone(),
            catalog,
            original_intent_sequence: seen.intent_sequence,
        }))
    }

    pub(crate) fn preview_admission(
        &self,
        intent: &DispatchIntent,
        now_unix_ms: u64,
        suspension_lift: &SuspensionLiftView<'_>,
    ) -> RuntimeHostResult<PolicyAdmissionRecord> {
        let active = self.active.as_ref().ok_or_else(|| {
            request(
                SharedCode::PolicyCatalogUnavailable,
                HostLocation::ReservePolicyBudget,
            )
        })?;
        self.control.preview_admission_lifted(
            &active.compiled,
            intent,
            now_unix_ms,
            Some(suspension_lift),
        )
    }

    /// Workflow #336 L6: the failure identity (if it has one) and the dispatch of a pair's
    /// latest failed execution, while no success has followed it.
    /// Workflow #372 K5: under an active catalog, the failure its task's accounting group reads.
    pub(crate) fn latest_failure(
        &self,
        task_id: &str,
        instance_id: &str,
    ) -> Option<(Option<&str>, &str)> {
        match &self.active {
            Some(active) => {
                self.control
                    .group_latest_failure(&active.compiled, task_id, instance_id)
            }
            None => self.control.latest_failure(task_id, instance_id),
        }
    }

    /// Workflow #336 L6 (§12.6 point 4): the execution already recorded for a dispatch, if any.
    pub(crate) fn recorded_execution(
        &self,
        decision_id: &str,
    ) -> Option<&PolicyExecutionEventData> {
        self.seen_dispatches
            .get(decision_id)
            .and_then(|dispatch| dispatch.execution.as_ref())
    }

    pub(crate) fn commit_admission(
        &mut self,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
        reason_chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<()> {
        let active = self.active.as_ref().ok_or_else(|| {
            request(
                SharedCode::PolicyCatalogUnavailable,
                HostLocation::CommitPolicyBudget,
            )
        })?;
        self.control.commit_admission(
            &active.compiled,
            intent,
            admission,
            reason_chain.reasons.iter().any(|reason| {
                reason
                    .outcome
                    .is(actingcommand_policy::PolicyCode::FailureRetryImmediate)
            }),
        )?;
        // An admitted dispatch ends its pair's eligibility age (Workflow #308 slice 5b).
        self.eligibility
            .forget(&intent.task_id, &intent.instance_id);
        Ok(())
    }

    /// Workflow #191 U5-F1: applies one admission's intent and its admitted or rejected outcome
    /// right after both were appended, under the same `HostShared.policy` guard, with the
    /// checks `recover_dispatches` runs on them (R1-R6, R11). `admitted` is the receipt the
    /// admission already committed into `control`, so an admitted outcome is compared with it
    /// instead of committing it again; a rejected outcome leaves `control` unchanged. Nothing
    /// is written unless every check passes.
    pub(crate) fn apply_admission_events(
        &mut self,
        intent: &PersistedEvent,
        outcome: &PersistedEvent,
        admitted: Option<&PolicyAdmissionRecord>,
    ) -> RuntimeHostResult<()> {
        let EventPayload::Policy(PolicyPayload::DispatchIntent(intent_payload)) = intent.payload()
        else {
            return Err(fatal(
                HostCode::PolicyAdmissionEventInvalid,
                HostLocation::ApplyPolicyAdmissionEvents,
            ));
        };
        let (decision_id, mut record) = self.seen_intent(intent, intent_payload)?;
        let (outcome_payload, next) = match (outcome.payload(), admitted) {
            (EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload)), Some(_)) => {
                (payload, DispatchLifecycle::Admitted)
            }
            (EventPayload::Policy(PolicyPayload::DispatchRejected(payload)), None) => {
                (payload, DispatchLifecycle::Rejected)
            }
            _ => {
                return Err(fatal(
                    HostCode::PolicyAdmissionEventInvalid,
                    HostLocation::ApplyPolicyAdmissionEvents,
                ));
            }
        };
        if outcome_payload.decision_id() != decision_id {
            return Err(fatal(
                HostCode::PolicyDispatchIntentMissing,
                HostLocation::ApplyPolicyAdmissionEvents,
            ));
        }
        transition_record(
            &mut record,
            outcome_payload,
            DispatchLifecycle::Intent,
            next,
            outcome.sequence(),
        )?;
        let pinned = match admitted {
            Some(admitted) => {
                let admission = outcome_payload.admission().ok_or_else(|| {
                    fatal(
                        HostCode::PolicyDispatchAdmissionMissing,
                        HostLocation::ApplyPolicyAdmissionEvents,
                    )
                })?;
                if admission != admitted {
                    return Err(fatal(
                        HostCode::PolicyBudgetReceiptMismatch,
                        HostLocation::ApplyPolicyAdmissionEvents,
                    ));
                }
                record.admission = Some(admission.clone());
                Some(
                    self.store
                        .load_generation(&record.data.catalog_hash)?
                        .generation
                        .clone(),
                )
            }
            None => None,
        };
        check_seen_intent(
            &self.seen_dispatches,
            &self.dispatch_order,
            &decision_id,
            intent.sequence(),
        )?;
        if pinned.is_some() && self.pinned_dispatches.contains_key(&decision_id) {
            return Err(fatal(
                HostCode::PolicyAdmissionEventInvalid,
                HostLocation::ApplyPolicyAdmissionEvents,
            ));
        }
        insert_seen_intent(
            &mut self.seen_dispatches,
            &mut self.dispatch_order,
            decision_id.clone(),
            record,
            intent.sequence(),
        )?;
        if let Some(generation) = pinned {
            self.pinned_dispatches.insert(decision_id, generation);
        }
        Ok(())
    }

    /// Rebuilds the dispatch projection from the whole ledger. Since Workflow #191 U5-F1 its
    /// callers are startup reconciliation (`reconcile_policy_dispatches`), an admission whose
    /// intent or outcome append returned an error, and an execution or completion append that
    /// returned an error; each runs it under the guard it appended under.
    /// `PolicyHost::open` calls `recover_dispatches` directly.
    pub(crate) fn refresh_dispatches(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        self.recover_dispatches(ledger)
    }

    pub(crate) fn pinned_catalog(&self, decision_id: &str) -> Option<CatalogGeneration> {
        self.pinned_dispatches.get(decision_id).cloned()
    }

    pub(crate) fn validate_run_context(&self, context: &PolicyRunContext) -> RuntimeHostResult<()> {
        let dispatch = self
            .seen_dispatches
            .get(context.decision_id())
            .ok_or_else(|| {
                request(
                    HostCode::PolicyRunDispatchUnknown,
                    HostLocation::ValidatePolicyRunContext,
                )
            })?;
        let expected_data = dispatch_event_data(&context.intent, &context.reason_chain)?;
        let pinned = self.pinned_dispatches.get(context.decision_id());
        if dispatch.data != expected_data || dispatch.admission.as_ref() != Some(&context.admission)
        {
            return Err(fatal(
                HostCode::PolicyRunContextMismatch,
                HostLocation::ValidatePolicyRunContext,
            ));
        }
        if dispatch.lifecycle != DispatchLifecycle::Admitted || pinned != Some(&context.catalog) {
            return Err(request(
                HostCode::PolicyRunNotAdmitted,
                HostLocation::ValidatePolicyRunContext,
            ));
        }
        Ok(())
    }

    pub(crate) fn referenced_outcome_keys(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<BTreeSet<String>> {
        self.validate_run_context(context)?;
        let loaded = self.store.load_generation(context.catalog.catalog_hash())?;
        if loaded.generation() != &context.catalog {
            return Err(fatal(
                HostCode::PolicyRunCatalogIdentityMismatch,
                HostLocation::ReadPolicyOutcomeKeys,
            ));
        }
        Ok(loaded
            .compiled()
            .referenced_outcome_keys(context.catalog_task_id()))
    }

    pub(crate) fn referenced_outcome_keys_for_completed_run(
        &self,
        context: &PolicyRunContext,
    ) -> RuntimeHostResult<BTreeSet<String>> {
        let dispatch = self
            .seen_dispatches
            .get(context.decision_id())
            .ok_or_else(|| {
                request(
                    HostCode::PolicyRunDispatchUnknown,
                    HostLocation::ReadPolicyOutcomeKeys,
                )
            })?;
        let expected_data = dispatch_event_data(&context.intent, &context.reason_chain)?;
        if dispatch.data != expected_data
            || dispatch.admission.as_ref() != Some(&context.admission)
            || dispatch.lifecycle != DispatchLifecycle::Completed
            || self.pinned_dispatches.contains_key(context.decision_id())
        {
            return Err(fatal(
                HostCode::PolicyRunCompletedContextMismatch,
                HostLocation::ReadPolicyOutcomeKeys,
            ));
        }
        let loaded = self.store.load_generation(context.catalog.catalog_hash())?;
        if loaded.generation() != &context.catalog {
            return Err(fatal(
                HostCode::PolicyRunCatalogIdentityMismatch,
                HostLocation::ReadPolicyOutcomeKeys,
            ));
        }
        Ok(loaded
            .compiled()
            .referenced_outcome_keys(context.catalog_task_id()))
    }

    pub(crate) fn outcome_key_snapshot(&self) -> RuntimeHostResult<PolicyOutcomeKeySnapshot> {
        let Some(active) = &self.active else {
            return Ok(PolicyOutcomeKeySnapshot {
                generation: None,
                keys: BTreeMap::new(),
                completed_runs: BTreeMap::new(),
            });
        };
        let mut keys = BTreeMap::new();
        for task in &active.compiled.catalog().tasks.tasks {
            let referenced = active.compiled.referenced_outcome_keys(&task.id);
            if !referenced.is_empty() {
                keys.insert(task.id.clone(), referenced);
            }
        }
        // Workflow #399: a pair's latest completion alone decides its outcome. A completion
        // carries a run identity only when its own catalog referenced the task's outcome, so a
        // latest completion without one means the current catalog references an outcome the
        // pair has not produced yet: the pair has no usable outcome (the evaluator reads it as
        // `outcome_missing`) until the task completes under a referencing catalog.
        // Workflow #400: the same holds for a latest completion whose outcome key the current
        // catalog no longer lists for the task. That key is known only from the authoritative
        // outcome, so the run stays in `completed_runs` and the projection
        // (`project_authoritative_policy_inputs_with_gaps_under_gate`) leaves the pair out.
        // Workflow #372 K5: the completions of an id a task continues count as that task's, so
        // a pair's entry is keyed by the task and holds its accounting group's latest completed
        // run, whose own identity names the id it ran as.
        let accounting = accounting_tasks(&active.compiled);
        let mut latest = BTreeMap::<(&str, &str), (u64, &SeenDispatch)>::new();
        for (sequence, dispatch) in self.seen_dispatches.values().filter_map(completion_of) {
            let task_id = accounting
                .get(dispatch.data.task_id.as_str())
                .copied()
                .unwrap_or(dispatch.data.task_id.as_str());
            if !keys.contains_key(task_id) {
                continue;
            }
            let key = (task_id, dispatch.data.instance_id.as_str());
            if latest
                .get(&key)
                .is_none_or(|(current, _)| *current < sequence)
            {
                latest.insert(key, (sequence, dispatch));
            }
        }
        let completed_runs = latest
            .into_iter()
            .filter_map(|((task_id, instance_id), (_, dispatch))| {
                dispatch
                    .completed_run
                    .clone()
                    .map(|completed| ((task_id.to_owned(), instance_id.to_owned()), completed))
            })
            .collect();
        Ok(PolicyOutcomeKeySnapshot {
            generation: Some(active.generation.clone()),
            keys,
            completed_runs,
        })
    }

    pub(crate) fn validate_outcome_key_snapshot(
        &self,
        snapshot: &PolicyOutcomeKeySnapshot,
    ) -> RuntimeHostResult<()> {
        if &self.outcome_key_snapshot()? != snapshot {
            return Err(request(
                HostCode::PolicyOutcomeSnapshotChanged,
                HostLocation::ValidatePolicyOutcomeKeySnapshot,
            ));
        }
        Ok(())
    }

    /// Whether the admission request of `run` was confirmed through `through_sequence` since
    /// this host opened (Workflow #313 goal 5, #317 item F). The cache key is the run's
    /// `(decision_id, completed_sequence)`; a hit needs the pair's entry to hold that same run
    /// identity (which carries both) and terminal sequence, so any change re-validates.
    pub(crate) fn completed_run_admission_confirmed(
        &self,
        run: &CompletedPolicyRunIdentity,
        through_sequence: u64,
    ) -> bool {
        self.confirmed_run_admissions
            .get(&(run.catalog_task_id.clone(), run.instance_alias.clone()))
            .is_some_and(|(confirmed, confirmed_through)| {
                confirmed == run && *confirmed_through == through_sequence
            })
    }

    /// Records a confirmed admission request. It replaces the pair's previous entry: a pair
    /// projects only its latest completed run, so the cache holds one entry per pair.
    pub(crate) fn record_completed_run_admission_confirmed(
        &mut self,
        run: &CompletedPolicyRunIdentity,
        through_sequence: u64,
    ) {
        self.confirmed_run_admissions.insert(
            (run.catalog_task_id.clone(), run.instance_alias.clone()),
            (run.clone(), through_sequence),
        );
    }

    /// Workflow #191 A2 / #330 H1: each (catalog task, instance) pair's latest completed run.
    /// A pair's authoritative outcome is decided by its latest run alone, the same result as the
    /// online commit/clear sequence; the limit counts distinct pairs.
    ///
    /// Workflow #399: the latest completion is taken before its run identity is read. A pair
    /// whose latest completion has none (its catalog did not reference the task's outcome) has
    /// no authoritative outcome and is not counted, so an older run never stands for it.
    pub(crate) fn latest_completed_policy_runs(
        &self,
        limit: usize,
    ) -> RuntimeHostResult<Vec<CompletedPolicyRunIdentity>> {
        let mut latest = BTreeMap::<(&str, &str), (u64, &SeenDispatch)>::new();
        for (sequence, dispatch) in self.seen_dispatches.values().filter_map(completion_of) {
            let key = (
                dispatch.data.task_id.as_str(),
                dispatch.data.instance_id.as_str(),
            );
            match latest.get(&key) {
                // A completion sequence is the completion event's own; equal means corruption.
                Some((existing, _)) if *existing == sequence => {
                    return Err(fatal(
                        HostCode::PolicyOutcomeReplayOrderConflict,
                        HostLocation::RecoverPolicySchedulingOutcomes,
                    ));
                }
                Some((existing, _)) if *existing > sequence => {}
                _ => {
                    latest.insert(key, (sequence, dispatch));
                }
            }
        }
        let runs = latest
            .into_values()
            .filter_map(|(_, dispatch)| dispatch.completed_run.as_ref())
            .collect::<Vec<_>>();
        if runs.len() > limit {
            return Err(fatal(
                HostCode::PolicySchedulingOutcomeCapacityExceeded,
                HostLocation::RecoverPolicySchedulingOutcomes,
            ));
        }
        Ok(runs.into_iter().cloned().collect())
    }

    pub(crate) fn admitted_at(&self, decision_id: &str) -> RuntimeHostResult<u64> {
        self.seen_dispatches
            .get(decision_id)
            .and_then(|dispatch| dispatch.admission.as_ref())
            .map(|admission| admission.activity.admitted_at_unix_ms)
            .ok_or_else(|| {
                request(
                    HostCode::PolicyDispatchAdmissionMissing,
                    HostLocation::ReadPolicyAdmissionTime,
                )
            })
    }

    pub(crate) fn recovered_dispatch_clocks(&self) -> RuntimeHostResult<BTreeMap<String, u64>> {
        self.seen_dispatches
            .iter()
            .filter(|(_, dispatch)| dispatch.lifecycle == DispatchLifecycle::Admitted)
            .map(|(decision_id, dispatch)| {
                let admitted_at_unix_ms = dispatch
                    .admission
                    .as_ref()
                    .map(|admission| admission.activity.admitted_at_unix_ms)
                    .ok_or_else(|| {
                        fatal(
                            HostCode::PolicyDispatchAdmissionMissing,
                            HostLocation::RecoverPolicyDispatchClocks,
                        )
                    })?;
                Ok((decision_id.clone(), admitted_at_unix_ms))
            })
            .collect()
    }

    pub(crate) fn replay_execution(
        &self,
        decision_id: &str,
        input: &PolicyExecutionInput,
    ) -> RuntimeHostResult<Option<PolicyExecutionEventData>> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            request(
                HostCode::PolicyDispatchUnknown,
                HostLocation::ReplayPolicyExecutionOutcome,
            )
        })?;
        let Some(existing) = &dispatch.execution else {
            return Ok(None);
        };
        if !execution_input_matches(&existing.outcome, input) {
            return Err(fatal(
                HostCode::PolicyExecutionIdentityConflict,
                HostLocation::ReplayPolicyExecutionOutcome,
            ));
        }
        Ok(Some(existing.clone()))
    }

    pub(crate) fn execution_instance_id(&self, decision_id: &str) -> RuntimeHostResult<&str> {
        self.seen_dispatches
            .get(decision_id)
            .map(|dispatch| dispatch.data.instance_id.as_str())
            .ok_or_else(|| {
                request(
                    HostCode::PolicyDispatchUnknown,
                    HostLocation::ReadPolicyDispatchInstance,
                )
            })
    }

    pub(crate) fn active_performance_workloads(
        &self,
    ) -> RuntimeHostResult<Vec<PerformanceControlWorkload>> {
        let mut workloads = BTreeMap::new();
        for dispatch in self
            .seen_dispatches
            .iter()
            .filter(|(decision_id, dispatch)| {
                dispatch.lifecycle == DispatchLifecycle::Admitted
                    && !self.quarantined_dispatches.contains(*decision_id)
            })
            .map(|(_, dispatch)| dispatch)
        {
            let admission = dispatch.admission.as_ref().ok_or_else(|| {
                fatal(
                    HostCode::PolicyDispatchAdmissionMissing,
                    HostLocation::ReadActivePerformanceWorkloads,
                )
            })?;
            let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
            let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
            let workload = PerformanceControlWorkload {
                arbitration: self.arbitration.get(&intent.instance_id).copied(),
                instance_id: intent.instance_id.clone(),
                load_profile: intent.load_profile,
            };
            if workloads
                .insert(workload.instance_id.clone(), workload)
                .is_some()
            {
                return Err(fatal(
                    HostCode::PolicyInstanceDispatchConflict,
                    HostLocation::ReadActivePerformanceWorkloads,
                ));
            }
        }
        Ok(workloads.into_values().collect())
    }

    pub(crate) fn completion_data(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<(PolicyDispatchEventData, PolicyAdmissionRecord)> {
        if !self.pinned_dispatches.contains_key(decision_id) {
            return Err(request(
                HostCode::PolicyDispatchNotPinned,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchStateIncomplete,
                HostLocation::CompletePolicyDispatch,
            )
        })?;
        let admission = dispatch.admission.clone().ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchAdmissionMissing,
                HostLocation::CompletePolicyDispatch,
            )
        })?;
        if dispatch.execution.is_none() {
            return Err(request(
                HostCode::PolicyExecutionOutcomeMissing,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        Ok((dispatch.data.clone(), admission))
    }

    pub(crate) fn execution_data(
        &self,
        decision_id: &str,
    ) -> RuntimeHostResult<PolicyExecutionEventData> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchStateIncomplete,
                HostLocation::ReadPolicyExecutionOutcome,
            )
        })?;
        if dispatch.lifecycle != DispatchLifecycle::Admitted {
            return Err(request(
                HostCode::PolicyDispatchNotAdmitted,
                HostLocation::ReadPolicyExecutionOutcome,
            ));
        }
        dispatch.execution.clone().ok_or_else(|| {
            fatal(
                HostCode::PolicyExecutionOutcomeMissing,
                HostLocation::ReadPolicyExecutionOutcome,
            )
        })
    }

    pub(crate) fn complete_dispatch(
        &mut self,
        decision_id: &str,
        completion: &PersistedEvent,
    ) -> RuntimeHostResult<()> {
        let dispatch = self.seen_dispatches.get_mut(decision_id).ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchStateIncomplete,
                HostLocation::CompletePolicyDispatch,
            )
        })?;
        if dispatch.lifecycle != DispatchLifecycle::Admitted
            || dispatch.execution.is_none()
            || dispatch.completed_sequence.is_some()
        {
            return Err(fatal(
                HostCode::PolicyDispatchCompletionIncomplete,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        if !self.pinned_dispatches.contains_key(decision_id) {
            return Err(request(
                HostCode::PolicyDispatchNotPinned,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        // The completion gets the checks recovery runs on it (R5, R8; Workflow #191 U5-F1)
        // before anything is written.
        let EventPayload::Policy(PolicyPayload::DispatchCompleted(payload)) = completion.payload()
        else {
            return Err(fatal(
                HostCode::PolicyDispatchCompletionIncomplete,
                HostLocation::CompletePolicyDispatch,
            ));
        };
        if payload.decision_id() != decision_id {
            return Err(fatal(
                HostCode::PolicyDispatchCompletionIncomplete,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        if event_data(payload)? != dispatch.data
            || completion.sequence() <= dispatch.intent_sequence
        {
            return Err(fatal(
                HostCode::PolicyDispatchLifecycleInvalid,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        if payload.admission() != dispatch.admission.as_ref() {
            return Err(fatal(
                HostCode::PolicyDispatchCompletionIncomplete,
                HostLocation::CompletePolicyDispatch,
            ));
        }
        let execution = dispatch.execution.as_ref().ok_or_else(|| {
            fatal(
                HostCode::PolicyExecutionOutcomeMissing,
                HostLocation::CompletePolicyDispatch,
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let referenced_outcome_keys = catalog
            .compiled()
            .referenced_outcome_keys(&dispatch.data.task_id);
        let completed_run = if referenced_outcome_keys.is_empty() {
            None
        } else {
            Some(completed_policy_run_identity(
                completion,
                &dispatch.data,
                execution,
                referenced_outcome_keys,
            )?)
        };
        self.pinned_dispatches.remove(decision_id);
        dispatch.completed_run = completed_run;
        dispatch.completed_sequence = Some(completion.sequence());
        dispatch.lifecycle = DispatchLifecycle::Completed;
        Ok(())
    }

    pub(crate) fn dispatch_needs_completion(&self, decision_id: &str) -> RuntimeHostResult<bool> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            request(
                HostCode::PolicyDispatchUnknown,
                HostLocation::ReadPolicyDispatchLifecycle,
            )
        })?;
        match dispatch.lifecycle {
            DispatchLifecycle::Admitted => Ok(true),
            DispatchLifecycle::Completed => Ok(false),
            DispatchLifecycle::Intent | DispatchLifecycle::Rejected => Err(request(
                HostCode::PolicyDispatchNotAdmitted,
                HostLocation::ReadPolicyDispatchLifecycle,
            )),
        }
    }

    /// The record a dispatch's run settles to, or the one it already has. Workflow #386 SR-2a:
    /// `condition` is how a run that a condition ended settled; no live settlement decides one
    /// before SR-2b, so the host's callers pass none. #385 P-k (Tk-10): `budget_elapsed_ms` is
    /// the run's budget time when it had a factor segment above 1000, which the settlement
    /// compares in place of `runtime_ms`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_execution(
        &self,
        decision_id: &str,
        observed_at_unix_ms: u64,
        runtime_ms: u64,
        budget_elapsed_ms: Option<u64>,
        input: &PolicyExecutionInput,
        perf_context: &PerformanceContext,
        condition: Option<PolicyConditionSettlement>,
    ) -> RuntimeHostResult<PolicyExecutionPreparation> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            request(
                HostCode::PolicyDispatchUnknown,
                HostLocation::PreparePolicyExecutionOutcome,
            )
        })?;
        if let Some(existing) = &dispatch.execution {
            if !execution_input_matches(&existing.outcome, input) {
                return Err(fatal(
                    HostCode::PolicyExecutionIdentityConflict,
                    HostLocation::PreparePolicyExecutionOutcome,
                ));
            }
            return Ok(PolicyExecutionPreparation::Replay(existing.clone()));
        }
        if dispatch.lifecycle != DispatchLifecycle::Admitted {
            return Err(request(
                HostCode::PolicyDispatchNotAdmitted,
                HostLocation::PreparePolicyExecutionOutcome,
            ));
        }
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchAdmissionMissing,
                HostLocation::PreparePolicyExecutionOutcome,
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
        self.control
            .preview_paced_settlement(
                &catalog.compiled,
                &intent,
                admission,
                PolicyExecutionTiming {
                    observed_at_unix_ms,
                    runtime_ms,
                },
                budget_elapsed_ms,
                input,
                perf_context,
                condition,
            )
            .map(PolicyExecutionPreparation::New)
    }

    /// Commits the record `data` of a dispatch's run; `budget_elapsed_ms` is the budget time
    /// the record carries (#385 P-k, register P6), which its settlement compared.
    pub(crate) fn commit_execution(
        &mut self,
        data: &PolicyExecutionEventData,
        budget_elapsed_ms: Option<u64>,
    ) -> RuntimeHostResult<()> {
        let dispatch = self
            .seen_dispatches
            .get_mut(&data.decision_id)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyDispatchUnknown,
                    HostLocation::CommitPolicyExecutionOutcome,
                )
            })?;
        if dispatch.execution.is_some() {
            return Err(fatal(
                HostCode::PolicyExecutionDuplicate,
                HostLocation::CommitPolicyExecutionOutcome,
            ));
        }
        // Recovery accepts an execution only on an admitted dispatch (R9, Workflow #191 U5-F1).
        if dispatch.lifecycle != DispatchLifecycle::Admitted {
            return Err(fatal(
                HostCode::PolicyExecutionLifecycleInvalid,
                HostLocation::CommitPolicyExecutionOutcome,
            ));
        }
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchAdmissionMissing,
                HostLocation::CommitPolicyExecutionOutcome,
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
        self.control.commit_paced_execution(
            &catalog.compiled,
            &intent,
            admission,
            data,
            budget_elapsed_ms,
        )?;
        dispatch.execution = Some(data.clone());
        Ok(())
    }

    pub(crate) fn planning_signal(
        &self,
        signal_id: &str,
    ) -> RuntimeHostResult<Option<PolicyPlanningSignalEventData>> {
        self.store
            .load_planning_signal(signal_id)
            .map(|entry| entry.map(|(_, data)| data))
    }

    fn stage_planning_signal(
        &self,
        staged_quota: &mut DetectionQuotaState,
        data: &PolicyPlanningSignalEventData,
        prior_quota: Option<(u64, DetectionQuotaUsage)>,
        quota_already_projected: bool,
    ) -> RuntimeHostResult<()> {
        self.validate_planning_signal(data)?;
        if data.detection_budget.is_some()
            && !matches!(
                data.kind,
                PolicyPlanningSignalKind::DetectionReserved
                    | PolicyPlanningSignalKind::DetectionQuotaExhausted
            )
        {
            return Err(fatal(
                SharedCode::PolicyDetectionBudgetUnexpected,
                HostLocation::CommitPolicyDetectionBudget,
            ));
        }
        if let Some(budget) = &data.detection_budget
            && let Some((_, usage)) = prior_quota
        {
            staged_quota
                .cache_usage((data.instance_id.clone(), budget.window_id.clone()), usage)?;
        }
        if !quota_already_projected {
            staged_quota.commit_signal(data)?;
        }
        Ok(())
    }

    pub(crate) fn validate_planning_signal(
        &self,
        data: &PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        let Some(budget) = &data.detection_budget else {
            return Ok(());
        };
        let catalog = self.store.load_generation(&budget.catalog_hash)?;
        let profile = catalog
            .compiled
            .catalog()
            .activity
            .profiles
            .iter()
            .find(|profile| profile.id == budget.profile_id)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyDetectionBudgetProfileMismatch,
                    HostLocation::ValidatePolicyDetectionBudget,
                )
            })?;
        let (_, expected_window_id) = active_activity_window(profile, data.observed_at_unix_ms)
            .map_err(|_| {
                fatal(
                    HostCode::PolicyDetectionBudgetWindowMismatch,
                    HostLocation::ValidatePolicyDetectionBudget,
                )
            })?;
        let instance_scope_matches = match &profile.scope {
            ScopeSelector::Instance { instance_id } => instance_id == &data.instance_id,
            ScopeSelector::Server { .. } | ScopeSelector::Game { .. } => true,
        };
        if !instance_scope_matches
            || budget.window_id != expected_window_id
            || budget.dispatch_limit != profile.detection_budget.window_dispatch_limit
            || budget.runtime_limit_ms != profile.detection_budget.window_runtime_ms
            || budget.reservation_ms != profile.detection_budget.expected_duration_ms
        {
            return Err(fatal(
                HostCode::PolicyDetectionBudgetProfileMismatch,
                HostLocation::ValidatePolicyDetectionBudget,
            ));
        }
        Ok(())
    }

    /// One intent event as recovery records it (R1, R2); shared by `recover_dispatches` and
    /// `apply_admission_events` (Workflow #191 U5-F1).
    fn seen_intent(
        &self,
        event: &PersistedEvent,
        payload: &actingcommand_contract::PolicyDispatchPayload,
    ) -> RuntimeHostResult<(String, SeenDispatch)> {
        let data = event_data(payload)?;
        self.store.load_generation(payload.catalog_hash())?;
        Ok((
            payload.decision_id().to_owned(),
            SeenDispatch {
                data,
                admission: None,
                execution: None,
                completed_run: None,
                intent_sequence: event.sequence(),
                admitted_sequence: None,
                rejected_sequence: None,
                completed_sequence: None,
                lifecycle: DispatchLifecycle::Intent,
            },
        ))
    }

    fn recover_dispatches(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        // Workflow #381 R4a: each dispatch type through one position, in bounded pages.
        let events = ledger
            .latest_sequence()
            .and_then(|through| {
                read_event_types(
                    ledger,
                    &[
                        EventType::PolicyDispatchIntent,
                        EventType::PolicyDispatchAdmitted,
                        EventType::PolicyDispatchRejected,
                        EventType::PolicyDispatchCompleted,
                        EventType::PolicyExecutionRecorded,
                    ],
                    through,
                    RECOVERY_PAGE_EVENTS,
                )
            })
            .map_err(|_| {
                fatal(
                    HostCode::PolicyRecoveryFailed,
                    HostLocation::RecoverPolicyDispatches,
                )
            })?;
        let mut seen_dispatches = BTreeMap::new();
        let mut dispatch_order = BTreeMap::new();
        let mut control = PolicyControlState::default();
        for event in events {
            let EventPayload::Policy(payload) = event.payload() else {
                continue;
            };
            match payload {
                PolicyPayload::DispatchIntent(payload) => {
                    let (decision_id, record) = self.seen_intent(&event, payload)?;
                    insert_seen_intent(
                        &mut seen_dispatches,
                        &mut dispatch_order,
                        decision_id,
                        record,
                        event.sequence(),
                    )?;
                }
                PolicyPayload::DispatchAdmitted(payload) => {
                    let dispatch = transition_dispatch(
                        &mut seen_dispatches,
                        payload,
                        DispatchLifecycle::Intent,
                        DispatchLifecycle::Admitted,
                        event.sequence(),
                    )?;
                    let admission = payload.admission().cloned().ok_or_else(|| {
                        fatal(
                            HostCode::PolicyDispatchAdmissionMissing,
                            HostLocation::RecoverPolicyDispatches,
                        )
                    })?;
                    let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
                    let intent = control_intent(&dispatch.data, &catalog.compiled, &admission)?;
                    control.commit_admission(
                        &catalog.compiled,
                        &intent,
                        &admission,
                        dispatch.data.reasons.iter().any(|reason| {
                            reason
                                .0
                                .is(actingcommand_policy::PolicyCode::FailureRetryImmediate)
                        }),
                    )?;
                    dispatch.admission = Some(admission);
                }
                PolicyPayload::DispatchRejected(payload) => {
                    transition_dispatch(
                        &mut seen_dispatches,
                        payload,
                        DispatchLifecycle::Intent,
                        DispatchLifecycle::Rejected,
                        event.sequence(),
                    )?;
                }
                PolicyPayload::DispatchCompleted(payload) => {
                    let dispatch = transition_dispatch(
                        &mut seen_dispatches,
                        payload,
                        DispatchLifecycle::Admitted,
                        DispatchLifecycle::Completed,
                        event.sequence(),
                    )?;
                    if dispatch.execution.is_none()
                        || payload.admission() != dispatch.admission.as_ref()
                    {
                        return Err(fatal(
                            HostCode::PolicyDispatchCompletionIncomplete,
                            HostLocation::RecoverPolicyDispatches,
                        ));
                    }
                    let execution = dispatch.execution.as_ref().ok_or_else(|| {
                        fatal(
                            HostCode::PolicyExecutionOutcomeMissing,
                            HostLocation::RecoverPolicyDispatches,
                        )
                    })?;
                    let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
                    let referenced_outcome_keys = catalog
                        .compiled()
                        .referenced_outcome_keys(&dispatch.data.task_id);
                    dispatch.completed_run = if referenced_outcome_keys.is_empty() {
                        None
                    } else {
                        Some(completed_policy_run_identity(
                            &event,
                            &dispatch.data,
                            execution,
                            referenced_outcome_keys,
                        )?)
                    };
                }
                PolicyPayload::ExecutionRecorded(payload) => {
                    let data = execution_event_data(payload);
                    let dispatch =
                        seen_dispatches
                            .get_mut(payload.decision_id())
                            .ok_or_else(|| {
                                fatal(
                                    HostCode::PolicyDispatchIntentMissing,
                                    ContractLocation::RecoverPolicyExecution,
                                )
                            })?;
                    if dispatch.lifecycle != DispatchLifecycle::Admitted
                        || dispatch.execution.is_some()
                    {
                        return Err(fatal(
                            HostCode::PolicyExecutionLifecycleInvalid,
                            ContractLocation::RecoverPolicyExecution,
                        ));
                    }
                    let admission = dispatch.admission.as_ref().ok_or_else(|| {
                        fatal(
                            HostCode::PolicyDispatchAdmissionMissing,
                            ContractLocation::RecoverPolicyExecution,
                        )
                    })?;
                    let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
                    let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
                    // #385 P-k (register P6): a record settled on its budget time replays on it.
                    control.commit_paced_execution(
                        &catalog.compiled,
                        &intent,
                        admission,
                        &data,
                        payload.budget_elapsed_ms(),
                    )?;
                    dispatch.execution = Some(data);
                }
                // The query reads the dispatch and execution types only; the value records of
                // the decision memory and the fact-refresh transitions (register L7, L14, D1)
                // are never replayed here.
                PolicyPayload::PlanningSignalObserved(_)
                | PolicyPayload::ControlStateRecorded(_)
                | PolicyPayload::ActivityStateRecorded(_)
                | PolicyPayload::FactRefreshTransition(_) => {
                    return Err(fatal(
                        SharedCode::PolicyRecoveryQueryMismatch,
                        HostLocation::RecoverPolicyDispatches,
                    ));
                }
            }
        }
        let mut pinned_dispatches = BTreeMap::new();
        for (decision_id, dispatch) in &seen_dispatches {
            if matches!(
                dispatch.lifecycle,
                DispatchLifecycle::Intent | DispatchLifecycle::Admitted
            ) {
                let catalog = self
                    .store
                    .load_generation(&dispatch.data.catalog_hash)?
                    .generation
                    .clone();
                pinned_dispatches.insert(decision_id.clone(), catalog);
            }
        }
        self.seen_dispatches = seen_dispatches;
        self.dispatch_order = dispatch_order;
        self.pinned_dispatches = pinned_dispatches;
        self.control = control;
        Ok(())
    }

    fn recover_planning_signals(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        let latest = ledger
            .latest_sequence()
            .map_err(planning_transaction_error)?;
        let mut after_sequence = self.store.load_planning_checkpoint()?.unwrap_or(0);
        if after_sequence > latest {
            return Err(fatal(
                SharedCode::PolicyPlanningCheckpointAhead,
                HostLocation::RecoverPolicyPlanningSignals,
            ));
        }
        while after_sequence < latest {
            let page = ledger
                .planning_signal_recovery_page(after_sequence, latest)
                .map_err(planning_transaction_error)?;
            let prepared = self
                .store
                .state
                .prepare_planning_recovery(page)
                .map_err(planning_state_error)?;
            let mut staged_quota = self.detection_quota.clone();
            for step in prepared.steps() {
                self.stage_planning_signal(
                    &mut staged_quota,
                    step.data(),
                    step.prior_quota(),
                    step.quota_already_projected(),
                )?;
            }
            let through_sequence = prepared.through_sequence();
            prepared.commit().map_err(planning_state_error)?;
            self.detection_quota = staged_quota;
            after_sequence = through_sequence;
        }
        Ok(())
    }
}

impl DetectionQuotaState {
    fn ensure_loaded(
        &mut self,
        store: &CatalogStore,
        instance_id: &str,
        window_id: &str,
    ) -> RuntimeHostResult<()> {
        let key = (instance_id.to_owned(), window_id.to_owned());
        if self.windows.contains_key(&key) {
            self.touch(&key);
            return Ok(());
        }
        if let Some((_, usage)) = store.load_detection_quota(instance_id, window_id)? {
            self.cache_usage(key, usage)?;
        }
        Ok(())
    }

    fn preview(
        &mut self,
        profile: &ActivityProfile,
        catalog_hash: &str,
        instance_id: &str,
        now_unix_ms: u64,
    ) -> RuntimeHostResult<(PolicyPlanningSignalKind, PolicyDetectionBudgetRecord)> {
        let (_, window_id) = active_activity_window(profile, now_unix_ms)?;
        let key = (instance_id.to_owned(), window_id.clone());
        let usage = self.windows.get(&key).copied().unwrap_or_default();
        if self.windows.contains_key(&key) {
            self.touch(&key);
        }
        let next_dispatch = usage.dispatch_used.checked_add(1).ok_or_else(|| {
            fatal(
                HostCode::PolicyDetectionBudgetOverflow,
                HostLocation::ReservePolicyDetectionBudget,
            )
        })?;
        let next_runtime = usage
            .runtime_reserved_ms
            .checked_add(profile.detection_budget.expected_duration_ms)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyDetectionBudgetOverflow,
                    HostLocation::ReservePolicyDetectionBudget,
                )
            })?;
        let exhausted = next_dispatch > profile.detection_budget.window_dispatch_limit
            || next_runtime > profile.detection_budget.window_runtime_ms;
        Ok((
            if exhausted {
                PolicyPlanningSignalKind::DetectionQuotaExhausted
            } else {
                PolicyPlanningSignalKind::DetectionReserved
            },
            PolicyDetectionBudgetRecord {
                catalog_hash: catalog_hash.to_owned(),
                profile_id: profile.id.clone(),
                window_id,
                dispatch_used: if exhausted {
                    usage.dispatch_used
                } else {
                    next_dispatch
                },
                dispatch_limit: profile.detection_budget.window_dispatch_limit,
                runtime_reserved_ms: if exhausted {
                    usage.runtime_reserved_ms
                } else {
                    next_runtime
                },
                runtime_limit_ms: profile.detection_budget.window_runtime_ms,
                reservation_ms: profile.detection_budget.expected_duration_ms,
            },
        ))
    }

    fn commit_signal(&mut self, data: &PolicyPlanningSignalEventData) -> RuntimeHostResult<()> {
        let detection_kind = matches!(
            data.kind,
            PolicyPlanningSignalKind::DetectionReserved
                | PolicyPlanningSignalKind::DetectionQuotaExhausted
        );
        if !detection_kind {
            return if data.detection_budget.is_none() {
                Ok(())
            } else {
                Err(fatal(
                    SharedCode::PolicyDetectionBudgetUnexpected,
                    HostLocation::CommitPolicyDetectionBudget,
                ))
            };
        }
        let budget = data.detection_budget.as_ref().ok_or_else(|| {
            fatal(
                SharedCode::PolicyDetectionBudgetMissing,
                HostLocation::CommitPolicyDetectionBudget,
            )
        })?;
        let key = (data.instance_id.clone(), budget.window_id.clone());
        let current = self.windows.get(&key).copied().unwrap_or_default();
        if self.windows.contains_key(&key) {
            self.touch(&key);
        }
        match data.kind {
            PolicyPlanningSignalKind::DetectionReserved => {
                let expected_dispatch = current.dispatch_used.checked_add(1).ok_or_else(|| {
                    fatal(
                        HostCode::PolicyDetectionBudgetOverflow,
                        HostLocation::CommitPolicyDetectionBudget,
                    )
                })?;
                let expected_runtime = current
                    .runtime_reserved_ms
                    .checked_add(budget.reservation_ms)
                    .ok_or_else(|| {
                        fatal(
                            HostCode::PolicyDetectionBudgetOverflow,
                            HostLocation::CommitPolicyDetectionBudget,
                        )
                    })?;
                if budget.dispatch_used != expected_dispatch
                    || budget.runtime_reserved_ms != expected_runtime
                    || budget.dispatch_used > budget.dispatch_limit
                    || budget.runtime_reserved_ms > budget.runtime_limit_ms
                {
                    return Err(fatal(
                        HostCode::PolicyDetectionBudgetReceiptMismatch,
                        HostLocation::CommitPolicyDetectionBudget,
                    ));
                }
                self.cache_usage(
                    key,
                    DetectionQuotaUsage {
                        dispatch_used: budget.dispatch_used,
                        runtime_reserved_ms: budget.runtime_reserved_ms,
                    },
                )?;
            }
            PolicyPlanningSignalKind::DetectionQuotaExhausted => {
                if budget.dispatch_used != current.dispatch_used
                    || budget.runtime_reserved_ms != current.runtime_reserved_ms
                    || budget.dispatch_used < budget.dispatch_limit
                        && budget
                            .runtime_reserved_ms
                            .checked_add(budget.reservation_ms)
                            .is_some_and(|next| next <= budget.runtime_limit_ms)
                {
                    return Err(fatal(
                        HostCode::PolicyDetectionBudgetReceiptMismatch,
                        HostLocation::CommitPolicyDetectionBudget,
                    ));
                }
            }
            _ => unreachable!("detection kind checked above"),
        }
        Ok(())
    }

    fn cache_usage(
        &mut self,
        key: (String, String),
        usage: DetectionQuotaUsage,
    ) -> RuntimeHostResult<()> {
        self.windows.insert(key.clone(), usage);
        self.touch(&key);
        while self.windows.len() > MAX_DETECTION_QUOTA_CACHE_WINDOWS {
            let oldest = self.recency.pop_front().ok_or_else(|| {
                fatal(
                    HostCode::PolicyDetectionQuotaCacheInvariant,
                    HostLocation::CachePolicyDetectionQuota,
                )
            })?;
            if self.windows.remove(&oldest).is_none() {
                return Err(fatal(
                    HostCode::PolicyDetectionQuotaCacheInvariant,
                    HostLocation::CachePolicyDetectionQuota,
                ));
            }
        }
        Ok(())
    }

    fn touch(&mut self, key: &(String, String)) {
        if let Some(position) = self.recency.iter().position(|candidate| candidate == key) {
            self.recency.remove(position);
        }
        self.recency.push_back(key.clone());
    }
}

fn preview_detection_planning_signals(
    store: &CatalogStore,
    state: &DetectionQuotaState,
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    evaluation: &PolicyEvaluation,
    pending_dispatch_intents: &[DispatchIntent],
    now_unix_ms: u64,
) -> RuntimeHostResult<Vec<PolicyPlanningSignalEventData>> {
    if now_unix_ms == 0 {
        return Err(fatal(
            HostCode::PolicyDetectionTimeInvalid,
            HostLocation::PlanPolicyDetection,
        ));
    }
    let ordinary_instances = pending_dispatch_intents
        .iter()
        .map(|intent| intent.instance_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut staged = state.clone();
    let mut emitted_ids = BTreeSet::new();
    let mut signals = Vec::new();
    for decision in &evaluation.decisions {
        let Some(instance_id) = decision.instance_id.as_deref() else {
            continue;
        };
        if decision.detection_suggestions.is_empty() || ordinary_instances.contains(instance_id) {
            continue;
        }
        let instance = facts
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .ok_or_else(|| {
                fatal(
                    HostCode::PolicyDetectionInstanceMissing,
                    HostLocation::PlanPolicyDetection,
                )
            })?;
        let profile = matching_activity_profile(&catalog.catalog().activity.profiles, instance)
            .ok_or_else(|| {
                request(
                    HostCode::PolicyActivityProfileMissing,
                    HostLocation::PlanPolicyDetection,
                )
            })?;
        for suggestion in &decision.detection_suggestions {
            let reservation_id = detection_signal_id(
                "reserved",
                &[
                    &facts.fact_snapshot_id,
                    catalog.catalog_hash(),
                    &decision.task_id,
                    instance_id,
                    &scope_identity(&suggestion.scope),
                    &suggestion.fact_key,
                    &suggestion.reason,
                ],
            );
            if store.load_planning_signal(&reservation_id)?.is_some()
                || emitted_ids.contains(&reservation_id)
            {
                continue;
            }
            let (_, window_id) = match active_activity_window(profile, now_unix_ms) {
                Ok(window) => window,
                Err(error)
                    if !error.is_fatal()
                        && error.code() == HostCode::PolicyActivityWindowClosed.as_str() =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            staged.ensure_loaded(store, instance_id, &window_id)?;
            let (kind, budget) =
                match staged.preview(profile, catalog.catalog_hash(), instance_id, now_unix_ms) {
                    Ok(value) => value,
                    Err(error) if error.code() == HostCode::PolicyActivityWindowClosed.as_str() => {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
            let signal_id = if kind == PolicyPlanningSignalKind::DetectionQuotaExhausted {
                detection_signal_id(
                    "exhausted",
                    &[
                        instance_id,
                        &budget.catalog_hash,
                        &profile.id,
                        &budget.window_id,
                    ],
                )
            } else {
                reservation_id
            };
            let existing = store
                .load_planning_signal(&signal_id)?
                .map(|(_, data)| data);
            if kind == PolicyPlanningSignalKind::DetectionQuotaExhausted
                && let Some(existing) = existing.as_ref()
            {
                if emitted_ids.insert(signal_id) {
                    signals.push(existing.clone());
                }
                continue;
            }
            if existing.is_some() || !emitted_ids.insert(signal_id.clone()) {
                continue;
            }
            let signal = PolicyPlanningSignalEventData {
                signal_id,
                instance_id: instance_id.to_owned(),
                task_id: Some(decision.task_id.clone()),
                kind,
                outcome: detection_outcome(suggestion)?,
                observed_at_unix_ms: now_unix_ms,
                detection_budget: Some(budget),
            };
            staged.commit_signal(&signal)?;
            signals.push(signal);
        }
    }
    Ok(signals)
}

/// The outcome of a detection planning signal (Workflow #378 A2-P): `planning_fact_unknown`
/// with the suggestion's fact key and its reason as a token of the vocabulary `unknown_reason`.
/// A reason the vocabulary does not list is a broken invariant of the policy evaluator.
fn detection_outcome(suggestion: &DetectionSuggestion) -> RuntimeHostResult<Outcome> {
    let token = UNKNOWN_REASON_TOKENS
        .iter()
        .copied()
        .find(|token| *token == suggestion.reason)
        .ok_or_else(|| {
            fatal(
                HostCode::PolicyDetectionReasonUnknown,
                HostLocation::PlanPolicyDetection,
            )
        })?;
    Ok(Outcome::new(HostCode::PlanningFactUnknown)
        .fact_key(suggestion.fact_key.as_str())
        .unknown_reason(UnknownReasonToken(token)))
}

/// The tokens of the catalog vocabulary `unknown_reason`.
const UNKNOWN_REASON_TOKENS: [&str; 13] = [
    "fact_missing",
    "fact_expired",
    "fact_stale",
    "fact_low_confidence",
    "fact_not_scalar",
    // The vocabulary token shares the registered code's spelling.
    SharedCode::TypeMismatch.as_str(),
    "field_missing",
    "resource_observation_missing",
    "lookup_miss",
    "dependency_state_missing",
    "outcome_missing",
    "outcome_expired",
    "resource_outcome_missing",
];

/// A token of the catalog vocabulary `unknown_reason`.
#[derive(Clone, Copy)]
struct UnknownReasonToken(&'static str);

impl VocabularyToken for UnknownReasonToken {
    const VOCABULARY: &'static str = "unknown_reason";

    fn token(self) -> &'static str {
        self.0
    }
}

fn matching_activity_profile<'a>(
    profiles: &'a [ActivityProfile],
    instance: &InstanceSnapshot,
) -> Option<&'a ActivityProfile> {
    profiles
        .iter()
        .filter(|profile| match &profile.scope {
            ScopeSelector::Instance { instance_id } => instance_id == &instance.instance_id,
            ScopeSelector::Server { server_id } => server_id == &instance.server_id,
            ScopeSelector::Game { game_id } => game_id == &instance.game_id,
        })
        .max_by(|left, right| {
            scope_specificity(&left.scope)
                .cmp(&scope_specificity(&right.scope))
                .then_with(|| right.id.cmp(&left.id))
        })
}

const fn scope_specificity(scope: &ScopeSelector) -> u8 {
    match scope {
        ScopeSelector::Instance { .. } => 3,
        ScopeSelector::Server { .. } => 2,
        ScopeSelector::Game { .. } => 1,
    }
}

fn scope_identity(scope: &ScopeSelector) -> String {
    match scope {
        ScopeSelector::Instance { instance_id } => format!("instance:{instance_id}"),
        ScopeSelector::Server { server_id } => format!("server:{server_id}"),
        ScopeSelector::Game { game_id } => format!("game:{game_id}"),
    }
}

fn detection_signal_id(kind: &str, components: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    for component in components {
        digest.update([0]);
        digest.update(component.as_bytes());
    }
    format!("signal:detection:{kind}:{:x}", digest.finalize())
}

fn event_data(
    payload: &actingcommand_contract::PolicyDispatchPayload,
) -> RuntimeHostResult<PolicyDispatchEventData> {
    {
        let valid = payload
            .procedure_binding_digest()
            .strip_prefix("sha256:")
            .is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            });
        if !valid {
            return Err(fatal(
                HostCode::ProcedureBindingEventInvalid,
                HostLocation::RecoverPolicyDispatches,
            ));
        }
    }
    Ok(PolicyDispatchEventData {
        decision_id: payload.decision_id().to_owned(),
        task_id: payload.task_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        operation_id: payload.operation_id().to_owned(),
        procedure_binding_digest: payload.procedure_binding_digest().to_owned(),
        reason_chain_id: payload.reason_chain_id().to_owned(),
        reasons: payload.reasons().to_vec(),
        catalog_hash: payload.catalog_hash().to_owned(),
        catalog_version: payload.catalog_version(),
        input_ledger_position: payload.input_ledger_position(),
        fact_snapshot_id: payload.fact_snapshot_id().to_owned(),
        approval_fact_ids: payload.approval_fact_ids().to_vec(),
        urgency_milli: payload.urgency_milli(),
    })
}

fn dispatch_event_data(
    intent: &DispatchIntent,
    reason_chain: &DecisionReasonChain,
) -> RuntimeHostResult<PolicyDispatchEventData> {
    let procedure_binding_digest = intent.procedure_binding_digest.clone().ok_or_else(|| {
        fatal(
            HostCode::ProcedureBindingDigestMissing,
            HostLocation::BuildPolicyDispatchEvent,
        )
    })?;
    Ok(PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
        procedure_binding_digest,
        reason_chain_id: reason_chain.id.clone(),
        reasons: reason_chain
            .reasons
            .iter()
            .map(|reason| PolicyReasonRecord(reason.outcome.clone()))
            .collect(),
        catalog_hash: intent.catalog_hash.clone(),
        catalog_version: intent.catalog_version,
        input_ledger_position: intent.input_ledger_position,
        fact_snapshot_id: intent.fact_snapshot_id.clone(),
        approval_fact_ids: intent.approval_refs.clone(),
        urgency_milli: intent.prerequisites.urgency_milli,
    })
}

fn transition_dispatch<'a>(
    seen: &'a mut BTreeMap<String, SeenDispatch>,
    payload: &actingcommand_contract::PolicyDispatchPayload,
    expected: DispatchLifecycle,
    next: DispatchLifecycle,
    sequence: u64,
) -> RuntimeHostResult<&'a mut SeenDispatch> {
    let Some(intent) = seen.get_mut(payload.decision_id()) else {
        return Err(fatal(
            HostCode::PolicyDispatchIntentMissing,
            HostLocation::RecoverPolicyDispatches,
        ));
    };
    transition_record(intent, payload, expected, next, sequence)?;
    Ok(intent)
}

/// Moves one recorded dispatch from `expected` to `next` at `sequence` (R5); shared by
/// `recover_dispatches` and `apply_admission_events` (Workflow #191 U5-F1).
fn transition_record(
    intent: &mut SeenDispatch,
    payload: &actingcommand_contract::PolicyDispatchPayload,
    expected: DispatchLifecycle,
    next: DispatchLifecycle,
    sequence: u64,
) -> RuntimeHostResult<()> {
    if intent.data != event_data(payload)?
        || intent.lifecycle != expected
        || sequence <= intent.intent_sequence
    {
        return Err(fatal(
            HostCode::PolicyDispatchLifecycleInvalid,
            HostLocation::RecoverPolicyDispatches,
        ));
    }
    match next {
        DispatchLifecycle::Admitted => intent.admitted_sequence = Some(sequence),
        DispatchLifecycle::Rejected => intent.rejected_sequence = Some(sequence),
        DispatchLifecycle::Completed => intent.completed_sequence = Some(sequence),
        DispatchLifecycle::Intent => {
            return Err(fatal(
                HostCode::PolicyDispatchLifecycleInvalid,
                HostLocation::RecoverPolicyDispatches,
            ));
        }
    }
    intent.lifecycle = next;
    Ok(())
}

/// A new intent must name a decision and a sequence recovery has not seen (R3, R4).
fn check_seen_intent(
    seen: &BTreeMap<String, SeenDispatch>,
    order: &BTreeMap<u64, String>,
    decision_id: &str,
    sequence: u64,
) -> RuntimeHostResult<()> {
    if seen.contains_key(decision_id) {
        return Err(fatal(
            HostCode::PolicyDecisionIdentityConflict,
            HostLocation::RecoverPolicyDispatches,
        ));
    }
    if order.contains_key(&sequence) {
        return Err(fatal(
            HostCode::PolicyDispatchSequenceConflict,
            HostLocation::RecoverPolicyDispatches,
        ));
    }
    Ok(())
}

/// Records a new intent once R3 and R4 pass; shared by `recover_dispatches` and
/// `apply_admission_events` (Workflow #191 U5-F1).
fn insert_seen_intent(
    seen: &mut BTreeMap<String, SeenDispatch>,
    order: &mut BTreeMap<u64, String>,
    decision_id: String,
    record: SeenDispatch,
    sequence: u64,
) -> RuntimeHostResult<()> {
    check_seen_intent(seen, order, &decision_id, sequence)?;
    order.insert(sequence, decision_id.clone());
    seen.insert(decision_id, record);
    Ok(())
}

/// A completed dispatch with its completion event's sequence (Workflow #399).
/// `complete_dispatch` and `transition_record` set the lifecycle and the sequence together.
fn completion_of(dispatch: &SeenDispatch) -> Option<(u64, &SeenDispatch)> {
    match (dispatch.lifecycle, dispatch.completed_sequence) {
        (DispatchLifecycle::Completed, Some(sequence)) => Some((sequence, dispatch)),
        _ => None,
    }
}

fn completed_policy_run_identity(
    event: &PersistedEvent,
    data: &PolicyDispatchEventData,
    execution: &PolicyExecutionEventData,
    referenced_outcome_keys: BTreeSet<String>,
) -> RuntimeHostResult<CompletedPolicyRunIdentity> {
    let EventPayload::Policy(PolicyPayload::DispatchCompleted(payload)) = event.payload() else {
        return Err(fatal(
            HostCode::PolicyDispatchCompletionIncomplete,
            HostLocation::RecoverPolicyDispatches,
        ));
    };
    let admission = payload.admission().ok_or_else(|| {
        fatal(
            HostCode::PolicyDispatchAdmissionMissing,
            HostLocation::RecoverPolicyDispatches,
        )
    })?;
    let links = event.links();
    let (
        Some(instance_id),
        Some(admission_request_id),
        Some(correlation_id),
        Some(task_id),
        Some(run_id),
        Some(lease_id),
    ) = (
        links.instance_id(),
        links.request_id(),
        links.correlation_id(),
        links.task_id(),
        links.run_id(),
        links.lease_id(),
    )
    else {
        return Err(fatal(
            HostCode::PolicyRunIdentityMissing,
            HostLocation::RecoverPolicyDispatches,
        ));
    };
    Ok(CompletedPolicyRunIdentity {
        decision_id: data.decision_id.clone(),
        catalog_task_id: data.task_id.clone(),
        instance_alias: data.instance_id.clone(),
        instance_id: *instance_id,
        admission_request_id: *admission_request_id,
        correlation_id: *correlation_id,
        task_id: *task_id,
        run_id: *run_id,
        lease_id: *lease_id,
        execution_outcome: execution.outcome.clone(),
        completion_sequence: event.sequence(),
        activity_window_id: admission.activity.window_id.clone(),
        referenced_outcome_keys,
    })
}

/// Workflow #372 K5: each id a task of `catalog` declares it continues, with that task's id.
fn accounting_tasks(catalog: &CompiledCatalog) -> BTreeMap<&str, &str> {
    catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .flat_map(|task| {
            task.continues_from
                .iter()
                .flatten()
                .map(|continued| (continued.as_str(), task.id.as_str()))
        })
        .collect()
}

/// Folds the executed runs among `runs`, in admission order per (task, instance) pair, into
/// each pair's latest settlement (Workflow #308 slice 5b).
fn fold_settlements<'a>(
    runs: impl Iterator<Item = &'a SeenDispatch>,
) -> RuntimeHostResult<BTreeMap<(String, String), PolicySettlement>> {
    fold_settlements_by(runs, &BTreeMap::new())
}

/// `fold_settlements` with each run of an id in `accounting` (a continued id, with the task
/// that continues it) folded as a run of that task (Workflow #372 K5).
fn fold_settlements_by<'a>(
    runs: impl Iterator<Item = &'a SeenDispatch>,
    accounting: &BTreeMap<&str, &str>,
) -> RuntimeHostResult<BTreeMap<(String, String), PolicySettlement>> {
    let mut executed = runs
        .filter_map(|dispatch| {
            dispatch
                .execution
                .as_ref()
                .map(|execution| (dispatch, execution))
        })
        .collect::<Vec<_>>();
    executed.sort_by_key(|(dispatch, _)| (dispatch.admitted_sequence, dispatch.intent_sequence));
    let mut latest = BTreeMap::<(String, String), PolicySettlement>::new();
    for (dispatch, execution) in executed {
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                HostCode::PolicyDispatchAdmissionMissing,
                HostLocation::ProjectPolicySettlement,
            )
        })?;
        let (succeeded, duration_ms) = match &execution.outcome {
            PolicyExecutionOutcome::Succeeded { runtime_ms } => (true, *runtime_ms),
            // The failure's duration is its settlement time minus its admission time.
            PolicyExecutionOutcome::Failed { .. } => (
                false,
                execution
                    .observed_at_unix_ms
                    .saturating_sub(admission.activity.admitted_at_unix_ms),
            ),
        };
        let task_id = accounting
            .get(dispatch.data.task_id.as_str())
            .copied()
            .unwrap_or(dispatch.data.task_id.as_str());
        let key = (task_id.to_owned(), dispatch.data.instance_id.clone());
        let failure_streak = if succeeded {
            0
        } else {
            latest
                .get(&key)
                .map_or(0, |previous| previous.failure_streak)
                .saturating_add(1)
        };
        latest.insert(
            key,
            PolicySettlement {
                catalog_task_id: task_id.to_owned(),
                instance_alias: dispatch.data.instance_id.clone(),
                duration_ms,
                succeeded,
                failure_streak,
                completed_at_unix_ms: execution.observed_at_unix_ms,
            },
        );
    }
    Ok(latest)
}

fn execution_event_data(
    payload: &actingcommand_contract::PolicyExecutionPayload,
) -> PolicyExecutionEventData {
    PolicyExecutionEventData {
        decision_id: payload.decision_id().to_owned(),
        task_id: payload.task_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        observed_at_unix_ms: payload.observed_at_unix_ms(),
        outcome: payload.policy_result().clone(),
    }
}

fn control_intent(
    data: &PolicyDispatchEventData,
    catalog: &CompiledCatalog,
    admission: &PolicyAdmissionRecord,
) -> RuntimeHostResult<DispatchIntent> {
    // Workflow #384 D2-S4: a fact refresh dispatch runs under the Runtime-owned spec.
    let task = catalog.dispatch_task(&data.task_id).ok_or_else(|| {
        fatal(
            HostCode::PolicyTaskMissing,
            HostLocation::RecoverPolicyControl,
        )
    })?;
    Ok(DispatchIntent {
        decision_id: data.decision_id.clone(),
        task_id: data.task_id.clone(),
        instance_id: data.instance_id.clone(),
        operation_id: data.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        procedure_binding_digest: Some(data.procedure_binding_digest.clone()),
        catalog_hash: data.catalog_hash.clone(),
        catalog_version: data.catalog_version,
        input_ledger_position: data.input_ledger_position,
        fact_snapshot_id: data.fact_snapshot_id.clone(),
        approval_refs: data.approval_fact_ids.clone(),
        reason_chain_id: data.reason_chain_id.clone(),
        expected_duration_ms: task.expected_duration_ms,
        yield_points: task.yield_points.clone(),
        load_profile: task.load_profile.clone(),
        prerequisites: DispatchPrerequisites {
            fencing_required: true,
            evaluated_at_unix_ms: admission.activity.admitted_at_unix_ms,
            facts_fresh_until_unix_ms: None,
            activity_profile_id: admission.activity.profile_id.clone(),
            daily_limit: task.loop_budget.daily_limit,
            window_iteration_limit: task.loop_budget.window_iteration_limit,
            max_runtime_ms: task.loop_budget.max_runtime_ms,
            urgency_milli: data.urgency_milli,
        },
    })
}

fn execution_input_matches(outcome: &PolicyExecutionOutcome, input: &PolicyExecutionInput) -> bool {
    match (outcome, input) {
        (PolicyExecutionOutcome::Succeeded { .. }, PolicyExecutionInput::Succeeded) => true,
        (PolicyExecutionOutcome::Failed { failure }, PolicyExecutionInput::Succeeded) => {
            failure.reported_success
        }
        (PolicyExecutionOutcome::Failed { failure }, PolicyExecutionInput::Failed { .. }) => {
            !failure.reported_success && input.names_failure(failure)
        }
        _ => false,
    }
}

struct CatalogStore {
    root: PathBuf,
    generations: PathBuf,
    legacy_active_pointer: PathBuf,
    state: Arc<RuntimeStateStore>,
    memo: Mutex<CatalogMemo>,
}

/// Bounded least-recently-used memo of verified, compiled generations keyed by
/// catalog hash. A generation directory is immutable once published, so the
/// hash alone identifies its verified content.
#[derive(Default)]
struct CatalogMemo {
    // Least recently used first, most recently used last.
    entries: Vec<(String, Arc<LoadedCatalog>)>,
}

impl CatalogMemo {
    fn get(&mut self, hash: &str) -> Option<Arc<LoadedCatalog>> {
        let position = self.entries.iter().position(|(key, _)| key == hash)?;
        let entry = self.entries.remove(position);
        let loaded = Arc::clone(&entry.1);
        self.entries.push(entry);
        Some(loaded)
    }

    fn insert(&mut self, hash: String, loaded: Arc<LoadedCatalog>) {
        if let Some(position) = self.entries.iter().position(|(key, _)| *key == hash) {
            self.entries.remove(position);
        }
        while self.entries.len() >= MAX_CATALOG_MEMO_GENERATIONS {
            self.entries.remove(0);
        }
        self.entries.push((hash, loaded));
    }
}

impl CatalogStore {
    fn open(state_root: &Path, state: Arc<RuntimeStateStore>) -> RuntimeHostResult<Self> {
        let root = state_root.join("policy").join("catalogs");
        let generations = root.join(GENERATIONS_DIR);
        fs::create_dir_all(&generations).map_err(|_| {
            fatal(
                HostCode::CatalogStateCreateFailed,
                HostLocation::OpenCatalogStore,
            )
        })?;
        let store = Self {
            legacy_active_pointer: root.join(ACTIVE_POINTER_FILE),
            root,
            generations,
            state,
            memo: Mutex::new(CatalogMemo::default()),
        };
        Ok(store)
    }

    /// Workflow #361 C3: the same store over an existing state root, read only: no directory
    /// is created and nothing is staged or migrated.
    fn open_read_only(state_root: &Path, state: Arc<RuntimeStateStore>) -> Self {
        let root = state_root.join("policy").join("catalogs");
        Self {
            legacy_active_pointer: root.join(ACTIVE_POINTER_FILE),
            generations: root.join(GENERATIONS_DIR),
            root,
            state,
            memo: Mutex::new(CatalogMemo::default()),
        }
    }

    fn stage(&self, sources: &CatalogSources) -> RuntimeHostResult<LoadedCatalog> {
        let compiled = compile_catalog(sources).map_err(|_| {
            request(
                HostCode::CatalogCompileFailed,
                HostLocation::StageCatalogGeneration,
            )
        })?;
        let generation = generation_from(&compiled, sources);
        let path = self.generation_path(&generation.catalog_hash)?;
        if path.exists() {
            return self
                .load_generation(&generation.catalog_hash)
                .map(|loaded| LoadedCatalog::clone(&loaded));
        }
        let temporary = self.generations.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&temporary).map_err(|_| {
            fatal(
                HostCode::CatalogGenerationCreateFailed,
                HostLocation::StageCatalogGeneration,
            )
        })?;
        let result = (|| {
            write_new_file(&temporary.join("tasks.json"), &sources.tasks.bytes)?;
            write_new_file(&temporary.join("pools.json"), &sources.pools.bytes)?;
            write_new_file(&temporary.join("activity.json"), &sources.activity.bytes)?;
            write_new_file(&temporary.join("timeline.json"), &sources.timeline.bytes)?;
            // The optional fifth document (Workflow #308 slice 4a-2).
            if let Some(selection) = &sources.selection {
                write_new_file(&temporary.join("selection.json"), &selection.bytes)?;
            }
            let manifest = serde_json::to_vec_pretty(&generation).map_err(|_| {
                fatal(
                    HostCode::CatalogManifestEncodeFailed,
                    HostLocation::StageCatalogGeneration,
                )
            })?;
            write_new_file(&temporary.join("manifest.json"), &manifest)?;
            fs::rename(&temporary, &path).map_err(|_| {
                fatal(
                    HostCode::CatalogGenerationPublishFailed,
                    HostLocation::StageCatalogGeneration,
                )
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            remove_directory_if_exists(&temporary)?;
            return Err(error);
        }
        self.load_generation(&generation.catalog_hash)
            .map(|loaded| LoadedCatalog::clone(&loaded))
    }

    fn load_active(&self) -> RuntimeHostResult<Option<LoadedCatalog>> {
        let Some(document) = self
            .state
            .read_json_document(ACTIVE_POINTER_STATE_KEY)
            .map_err(|error| RuntimeHostError::state(&error))?
        else {
            return Ok(None);
        };
        if document.schema_version() != CATALOG_STATE_SCHEMA {
            return Err(fatal(
                HostCode::CatalogPointerVersionUnsupported,
                HostLocation::LoadActiveCatalog,
            ));
        }
        let pointer: CatalogPointer = serde_json::from_slice(document.payload()).map_err(|_| {
            fatal(
                HostCode::CatalogPointerInvalid,
                HostLocation::LoadActiveCatalog,
            )
        })?;
        if pointer.schema_version != CATALOG_STATE_SCHEMA {
            return Err(fatal(
                HostCode::CatalogPointerVersionUnsupported,
                HostLocation::LoadActiveCatalog,
            ));
        }
        let loaded = self.load_generation(&pointer.generation.catalog_hash)?;
        if loaded.generation != pointer.generation {
            return Err(fatal(
                HostCode::CatalogPointerGenerationMismatch,
                HostLocation::LoadActiveCatalog,
            ));
        }
        Ok(Some(LoadedCatalog::clone(&loaded)))
    }

    fn load_generation(&self, hash: &str) -> RuntimeHostResult<Arc<LoadedCatalog>> {
        if let Some(loaded) = self.memoised_generation(hash)? {
            return Ok(loaded);
        }
        let loaded = Arc::new(self.read_generation(hash)?);
        self.memoise_generation(hash, Arc::clone(&loaded))?;
        Ok(loaded)
    }

    fn memoised_generation(&self, hash: &str) -> RuntimeHostResult<Option<Arc<LoadedCatalog>>> {
        let loaded = self
            .memo
            .lock()
            .map_err(|_| {
                fatal(
                    HostCode::RuntimeStatePoisoned,
                    HostLocation::LoadCatalogGeneration,
                )
            })?
            .get(hash);
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        if loaded.generation.schema_version != CATALOG_STATE_SCHEMA
            || loaded.generation.catalog_hash != hash
        {
            return Err(fatal(
                HostCode::CatalogGenerationIdentityMismatch,
                HostLocation::LoadCatalogGeneration,
            ));
        }
        Ok(Some(loaded))
    }

    fn memoise_generation(&self, hash: &str, loaded: Arc<LoadedCatalog>) -> RuntimeHostResult<()> {
        self.memo
            .lock()
            .map_err(|_| {
                fatal(
                    HostCode::RuntimeStatePoisoned,
                    HostLocation::LoadCatalogGeneration,
                )
            })?
            .insert(hash.to_owned(), loaded);
        Ok(())
    }

    fn read_generation(&self, hash: &str) -> RuntimeHostResult<LoadedCatalog> {
        let path = self.generation_path(hash)?;
        let manifest = read_bounded(&path.join("manifest.json"), MAX_MANIFEST_BYTES)?;
        let generation: CatalogGeneration = serde_json::from_slice(&manifest).map_err(|_| {
            fatal(
                HostCode::CatalogManifestInvalid,
                HostLocation::LoadCatalogGeneration,
            )
        })?;
        if generation.schema_version != CATALOG_STATE_SCHEMA || generation.catalog_hash != hash {
            return Err(fatal(
                HostCode::CatalogGenerationIdentityMismatch,
                HostLocation::LoadCatalogGeneration,
            ));
        }
        // A generation records its selection document only when it has one; its absence is
        // the four-document catalog, never a missing file.
        let selection = if generation
            .sources
            .iter()
            .any(|record| record.kind == "selection")
        {
            Some(self.load_source(&path, &generation, "selection")?)
        } else {
            None
        };
        let sources = CatalogSources {
            tasks: self.load_source(&path, &generation, "tasks")?,
            pools: self.load_source(&path, &generation, "pools")?,
            activity: self.load_source(&path, &generation, "activity")?,
            timeline: self.load_source(&path, &generation, "timeline")?,
            selection,
        };
        let compiled = compile_catalog(&sources).map_err(|_| {
            fatal(
                HostCode::CatalogGenerationInvalid,
                HostLocation::LoadCatalogGeneration,
            )
        })?;
        if compiled.catalog_hash() != generation.catalog_hash
            || compiled.summary().catalog_id != generation.catalog_id
            || compiled.summary().catalog_version != generation.catalog_version
        {
            return Err(fatal(
                HostCode::CatalogGenerationContentMismatch,
                HostLocation::LoadCatalogGeneration,
            ));
        }
        Ok(LoadedCatalog {
            generation,
            compiled,
            sources,
        })
    }

    fn load_planning_signal(
        &self,
        signal_id: &str,
    ) -> RuntimeHostResult<Option<(u64, PolicyPlanningSignalEventData)>> {
        self.state
            .load_planning_signal(signal_id)
            .map_err(planning_state_error)
    }

    fn load_detection_quota(
        &self,
        instance_id: &str,
        window_id: &str,
    ) -> RuntimeHostResult<Option<(u64, DetectionQuotaUsage)>> {
        self.state
            .load_detection_quota(instance_id, window_id)
            .map_err(planning_state_error)
    }

    fn load_planning_checkpoint(&self) -> RuntimeHostResult<Option<u64>> {
        self.state
            .load_planning_checkpoint()
            .map_err(planning_state_error)
    }

    fn load_source(
        &self,
        path: &Path,
        generation: &CatalogGeneration,
        kind: &str,
    ) -> RuntimeHostResult<CatalogDocumentSource> {
        let record = generation
            .sources
            .iter()
            .find(|record| record.kind == kind)
            .ok_or_else(|| {
                fatal(
                    HostCode::CatalogSourceMissing,
                    HostLocation::LoadCatalogGeneration,
                )
            })?;
        let bytes = read_bounded(
            &path.join(format!("{kind}.json")),
            actingcommand_policy::MAX_DOCUMENT_BYTES,
        )?;
        if sha256(&bytes) != record.sha256 {
            return Err(fatal(
                HostCode::CatalogSourceHashMismatch,
                HostLocation::LoadCatalogGeneration,
            ));
        }
        Ok(CatalogDocumentSource::new(record.source_uri.clone(), bytes))
    }

    fn generation_path(&self, hash: &str) -> RuntimeHostResult<PathBuf> {
        let digest = hash.strip_prefix("sha256:").ok_or_else(|| {
            request(
                HostCode::CatalogHashInvalid,
                HostLocation::ResolveCatalogGeneration,
            )
        })?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(request(
                HostCode::CatalogHashInvalid,
                HostLocation::ResolveCatalogGeneration,
            ));
        }
        Ok(self.generations.join(digest))
    }
}

fn generation_from(compiled: &CompiledCatalog, sources: &CatalogSources) -> CatalogGeneration {
    let summary = compiled.summary();
    let mut records = vec![
        source_record("tasks", &sources.tasks),
        source_record("pools", &sources.pools),
        source_record("activity", &sources.activity),
        source_record("timeline", &sources.timeline),
    ];
    if let Some(selection) = &sources.selection {
        records.push(source_record("selection", selection));
    }
    CatalogGeneration {
        schema_version: CATALOG_STATE_SCHEMA.to_owned(),
        catalog_id: summary.catalog_id.clone(),
        catalog_version: summary.catalog_version,
        catalog_hash: summary.catalog_hash.clone(),
        sources: records,
    }
}

fn source_record(kind: &str, source: &CatalogDocumentSource) -> CatalogSourceRecord {
    CatalogSourceRecord {
        kind: kind.to_owned(),
        source_uri: source.source_uri.clone(),
        sha256: sha256(&source.bytes),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> RuntimeHostResult<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| {
            fatal(
                HostCode::CatalogFileCreateFailed,
                HostLocation::WriteCatalogState,
            )
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| {
            fatal(
                HostCode::CatalogFileWriteFailed,
                HostLocation::WriteCatalogState,
            )
        })
}

fn read_bounded(path: &Path, maximum_bytes: usize) -> RuntimeHostResult<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|_| {
        fatal(
            HostCode::CatalogFileUnavailable,
            HostLocation::ReadCatalogState,
        )
    })?;
    if metadata.len() > maximum_bytes as u64 {
        return Err(fatal(
            HostCode::CatalogFileTooLarge,
            HostLocation::ReadCatalogState,
        ));
    }
    fs::read(path).map_err(|_| {
        fatal(
            HostCode::CatalogFileReadFailed,
            HostLocation::ReadCatalogState,
        )
    })
}

fn remove_directory_if_exists(path: &Path) -> RuntimeHostResult<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(fatal(
            HostCode::CatalogTemporaryCleanupFailed,
            HostLocation::StageCatalogGeneration,
        )),
    }
}

fn policy_evaluation_cost(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    resources: &EvaluationResources,
) -> RuntimeHostResult<PolicyEvaluationCost> {
    let counts = catalog.summary().counts;
    let catalog_tasks = count_u64(counts.tasks)?;
    let catalog_records = [
        counts.tasks,
        counts.pools,
        counts.activity_profiles,
        counts.timeline_events,
    ]
    .into_iter()
    .try_fold(0_u64, |total, count| {
        total
            .checked_add(count_u64(count)?)
            .ok_or_else(policy_evaluation_measurement_overflow)
    })?;
    let fact_records = count_u64(facts.facts.len())?;
    let outcome_records = count_u64(facts.outcomes.len())?;
    let task_state_records = count_u64(facts.tasks.len())?;
    let instance_records = count_u64(facts.instances.len())?;
    let resource_records = count_u64(resources.pools.len())?
        .checked_add(count_u64(resources.hosts.len())?)
        .ok_or_else(policy_evaluation_measurement_overflow)?;
    let task_instance_pairs = catalog_tasks
        .checked_mul(instance_records)
        .ok_or_else(policy_evaluation_measurement_overflow)?;
    let work_units = [
        catalog_records,
        fact_records,
        outcome_records,
        task_state_records,
        instance_records,
        resource_records,
        task_instance_pairs,
    ]
    .into_iter()
    .try_fold(0_u64, |total, count| {
        total
            .checked_add(count)
            .ok_or_else(policy_evaluation_measurement_overflow)
    })?;
    Ok(PolicyEvaluationCost {
        catalog_records,
        catalog_tasks,
        fact_records,
        outcome_records,
        task_state_records,
        instance_records,
        resource_records,
        task_instance_pairs,
        work_units,
    })
}

fn count_u64(value: usize) -> RuntimeHostResult<u64> {
    u64::try_from(value).map_err(|_| policy_evaluation_measurement_overflow())
}

fn policy_evaluation_measurement_overflow() -> RuntimeHostError {
    fatal(
        HostCode::PolicyEvaluationMeasurementOverflow,
        HostLocation::MeasurePolicyEvaluation,
    )
}

#[cfg(unix)]
fn sync_directory(path: &Path, operation: impl Into<Location>) -> RuntimeHostResult<()> {
    let operation: Location = operation.into();
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| fatal(HostCode::CatalogDirectorySyncFailed, operation))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path, _operation: impl Into<Location>) -> RuntimeHostResult<()> {
    // Rust's standard library cannot open Windows directories for fsync without unsafe flags.
    Ok(())
}

fn fatal(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeHalted)
}

fn request(code: impl Into<Code>, operation: impl Into<Location>) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

#[cfg(test)]
mod detection_quota_cache_tests {
    use super::*;

    #[test]
    fn detection_quota_cache_evicts_lru_windows_at_the_evaluation_bound() {
        const OVERFLOW: usize = 16;

        let mut state = DetectionQuotaState::default();
        for index in 0..MAX_DETECTION_QUOTA_CACHE_WINDOWS + OVERFLOW {
            state
                .cache_usage(
                    quota_key(index),
                    DetectionQuotaUsage {
                        dispatch_used: 1,
                        runtime_reserved_ms: 1,
                    },
                )
                .expect("cache quota usage");
        }

        assert_eq!(state.windows.len(), MAX_DETECTION_QUOTA_CACHE_WINDOWS);
        assert_eq!(state.recency.len(), MAX_DETECTION_QUOTA_CACHE_WINDOWS);
        for index in 0..OVERFLOW {
            assert!(!state.windows.contains_key(&quota_key(index)));
        }

        let touched = quota_key(OVERFLOW);
        state.touch(&touched);
        state
            .cache_usage(
                quota_key(MAX_DETECTION_QUOTA_CACHE_WINDOWS + OVERFLOW),
                DetectionQuotaUsage {
                    dispatch_used: 2,
                    runtime_reserved_ms: 2,
                },
            )
            .expect("cache next quota usage");

        assert!(state.windows.contains_key(&touched));
        assert!(!state.windows.contains_key(&quota_key(OVERFLOW + 1)));
        assert_eq!(
            state.recency.iter().cloned().collect::<BTreeSet<_>>(),
            state.windows.keys().cloned().collect::<BTreeSet<_>>()
        );
    }

    fn quota_key(index: usize) -> (String, String) {
        (format!("instance-{index}"), format!("window-{index}"))
    }
}
