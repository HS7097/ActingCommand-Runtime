// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned catalog generations and replayable policy admission state.

use crate::policy_control::{
    PolicyControlState, PolicyExecutionInput, PolicyExecutionTiming, active_activity_window,
};
use crate::{PerformanceControlWorkload, ProcedureManifest, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    CatalogPayload, EventPayload, EventQuery, EventType, LeaseToken, OwnerEpoch,
    PerformanceContext, PolicyAdmissionRecord, PolicyDetectionBudgetRecord,
    PolicyDispatchEventData, PolicyExecutionEventData, PolicyExecutionOutcome, PolicyPayload,
    PolicyPlanningSignalEventData, PolicyPlanningSignalKind, PolicyReasonRecord,
    ProjectDecisionPageRequest, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;
use actingcommand_policy::{
    ActivityProfile, CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReasonChain,
    DispatchIntent, DispatchPrerequisites, EvaluationFacts, EvaluationResources, EvaluationTime,
    InstanceSnapshot, MAX_EVALUATION_INSTANCES, PolicyEvaluation, ScopeSelector, compile_catalog,
    evaluate,
};
use actingcommand_runtime_state::RuntimeStateStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const CATALOG_STATE_SCHEMA: &str = "actingcommand.catalog-state.v1";
const LEGACY_CATALOG_POINTER_SCHEMA: &str = "actingcommand.catalog-pointer-file.v1";
const ACTIVE_POINTER_FILE: &str = "active.json";
const ACTIVE_POINTER_STATE_KEY: &str = "policy.catalog.active";
const GENERATIONS_DIR: &str = "generations";
const MAX_POINTER_BYTES: usize = 16 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const DEFAULT_DEBOUNCE_MS: u64 = 250;
const DEFAULT_COOLDOWN_MS: u64 = 1_000;
const DEFAULT_RECONCILIATION_INTERVAL_MS: u64 = 60_000;
const DEFAULT_CLOCK_JUMP_THRESHOLD_MS: u64 = 5_000;
const PLANNING_SIGNAL_PROJECTION_NAMESPACE: &str = "policy.planning-signal.v1";
const DETECTION_QUOTA_PROJECTION_NAMESPACE: &str = "policy.detection-quota.v1";
const PLANNING_SIGNAL_CHECKPOINT_KEY: &str = "checkpoint";
const PLANNING_SIGNAL_PROJECTION_SCHEMA: &str = "actingcommand.policy-planning-signal.v1";
const DETECTION_QUOTA_PROJECTION_SCHEMA: &str = "actingcommand.policy-detection-quota.v1";
const PLANNING_SIGNAL_RECOVERY_PAGE_EVENTS: usize = 256;
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
            return Err(fatal("invalid_policy_cadence", "initialize_policy_runtime"));
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
    pub measurement: Option<PolicyEvaluationMeasurement>,
}

pub(crate) struct PolicyEvaluationContext<'a> {
    pub(crate) procedure_manifest: &'a ProcedureManifest,
    pub(crate) time: EvaluationTime,
    pub(crate) seed: u64,
    pub(crate) trigger: PolicyTrigger,
    pub(crate) sampled_at_monotonic_ms: u64,
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
        decision_id: String,
        catalog: CatalogGeneration,
        token: LeaseToken,
        admission: Box<PolicyAdmissionRecord>,
    },
    ReplaySuppressed {
        decision_id: String,
        catalog: CatalogGeneration,
        original_intent_sequence: u64,
    },
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
    intent_sequence: u64,
    admitted_sequence: Option<u64>,
    rejected_sequence: Option<u64>,
    completed_sequence: Option<u64>,
    lifecycle: DispatchLifecycle,
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
            return Err(request("invalid_policy_time", "evaluate_policy_cycle"));
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
                .ok_or_else(|| fatal("policy_time_overflow", "evaluate_reconciliation_trigger"))?;
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
            .ok_or_else(|| fatal("policy_time_overflow", "evaluate_policy_event"))?;
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
            .ok_or_else(|| fatal("policy_time_overflow", "evaluate_full_policy_cycle"))?;
        Ok(PolicyRecomputeDirective {
            kind: PolicyRecomputeKind::Full,
            reason,
            eligible_at_unix_ms: now_unix_ms,
        })
    }
}

pub(crate) struct PolicyHost {
    store: CatalogStore,
    active: Option<LoadedCatalog>,
    cadence: PolicyCadenceState,
    seen_dispatches: BTreeMap<String, SeenDispatch>,
    dispatch_order: BTreeMap<u64, String>,
    pinned_dispatches: BTreeMap<String, CatalogGeneration>,
    detection_quota: DetectionQuotaState,
    control: PolicyControlState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DetectionQuotaState {
    windows: BTreeMap<(String, String), DetectionQuotaUsage>,
    recency: VecDeque<(String, String)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DetectionQuotaUsage {
    dispatch_used: u32,
    runtime_reserved_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPlanningSignal {
    schema_version: String,
    signal_id: String,
    instance_id: String,
    task_id: Option<String>,
    kind: PolicyPlanningSignalKind,
    fact_code: String,
    observed_at_unix_ms: u64,
    detection_budget: Option<PolicyDetectionBudgetRecord>,
}

impl StoredPlanningSignal {
    fn from_data(data: &PolicyPlanningSignalEventData) -> Self {
        Self {
            schema_version: PLANNING_SIGNAL_PROJECTION_SCHEMA.to_owned(),
            signal_id: data.signal_id.clone(),
            instance_id: data.instance_id.clone(),
            task_id: data.task_id.clone(),
            kind: data.kind,
            fact_code: data.fact_code.clone(),
            observed_at_unix_ms: data.observed_at_unix_ms,
            detection_budget: data.detection_budget.clone(),
        }
    }

    fn into_data(self) -> PolicyPlanningSignalEventData {
        PolicyPlanningSignalEventData {
            signal_id: self.signal_id,
            instance_id: self.instance_id,
            task_id: self.task_id,
            kind: self.kind,
            fact_code: self.fact_code,
            observed_at_unix_ms: self.observed_at_unix_ms,
            detection_budget: self.detection_budget,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDetectionQuota {
    schema_version: String,
    instance_id: String,
    window_id: String,
    dispatch_used: u32,
    runtime_reserved_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningSignalCheckpoint {
    schema_version: String,
    through_sequence: u64,
}

impl PolicyHost {
    pub(crate) fn open(
        state_root: &Path,
        state: Arc<RuntimeStateStore>,
        ledger: &GlobalLedger,
        cadence: PolicyCadence,
    ) -> RuntimeHostResult<Self> {
        let store = CatalogStore::open(state_root, state)?;
        let active = store.load_active()?;
        let mut host = Self {
            store,
            active,
            cadence: PolicyCadenceState::new(cadence)?,
            seen_dispatches: BTreeMap::new(),
            dispatch_order: BTreeMap::new(),
            pinned_dispatches: BTreeMap::new(),
            detection_quota: DetectionQuotaState::default(),
            control: PolicyControlState::default(),
        };
        host.recover_dispatches(ledger)?;
        host.recover_planning_signals(ledger)?;
        Ok(host)
    }

    pub(crate) fn stage(&self, sources: &CatalogSources) -> RuntimeHostResult<LoadedCatalog> {
        self.store.stage(sources)
    }

    pub(crate) fn load_generation(&self, hash: &str) -> RuntimeHostResult<LoadedCatalog> {
        self.store.load_generation(hash)
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
        let latest = ledger
            .latest_sequence()
            .map_err(|_| fatal("catalog_projection_query_failed", "project_policy_catalog"))?;
        if ledger_position == 0 || ledger_position > latest {
            return Err(request(
                "catalog_projection_position_invalid",
                "project_policy_catalog",
            ));
        }
        let events = ledger
            .query(EventQuery {
                to_sequence: Some(ledger_position),
                ..EventQuery::default()
            })
            .map_err(|_| fatal("catalog_projection_query_failed", "project_policy_catalog"))?;
        let mut projected = None;
        for event in events {
            let transition = match event.payload() {
                EventPayload::Catalog(CatalogPayload::Activated(payload))
                | EventPayload::Catalog(CatalogPayload::RolledBack(payload)) => payload,
                _ => continue,
            };
            projected = Some((
                transition.catalog_id().to_owned(),
                transition.catalog_version(),
                transition.catalog_hash().to_owned(),
            ));
        }
        let Some((catalog_id, catalog_version, catalog_hash)) = projected else {
            return Ok(None);
        };
        let loaded = self.store.load_generation(&catalog_hash)?;
        let generation = loaded.generation();
        if generation.catalog_id() != catalog_id
            || generation.catalog_version() != catalog_version
            || generation.catalog_hash() != catalog_hash
        {
            return Err(fatal(
                "catalog_projection_identity_mismatch",
                "project_policy_catalog",
            ));
        }
        Ok(Some(loaded))
    }

    pub(crate) fn project_dispatches(
        &self,
        current_ledger_position: u64,
        page_request: &ProjectDecisionPageRequest,
    ) -> RuntimeHostResult<PolicyDispatchPage> {
        page_request
            .validate()
            .map_err(|_| request("project_decision_page_invalid", "project_policy_dispatches"))?;
        let snapshot_ledger_position = page_request
            .cursor()
            .map_or(current_ledger_position, |cursor| {
                cursor.snapshot_ledger_position()
            });
        if snapshot_ledger_position == 0 || snapshot_ledger_position > current_ledger_position {
            return Err(request(
                "project_decision_cursor_invalid",
                "project_policy_dispatches",
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
                    "project_decision_cursor_invalid",
                    "project_policy_dispatches",
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
        let dispatch = self
            .seen_dispatches
            .get(decision_id)
            .ok_or_else(|| fatal("policy_dispatch_order_corrupt", "project_policy_dispatches"))?;
        if dispatch.intent_sequence > snapshot_ledger_position {
            return Err(fatal(
                "policy_dispatch_snapshot_invalid",
                "project_policy_dispatches",
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

    pub(crate) fn switch_active(
        &mut self,
        catalog: LoadedCatalog,
        expected_active_hash: Option<&str>,
    ) -> RuntimeHostResult<()> {
        let active_hash = self
            .active
            .as_ref()
            .map(|active| active.generation.catalog_hash.as_str());
        if active_hash != expected_active_hash {
            return Err(request(
                "catalog_active_generation_changed",
                "switch_active_catalog",
            ));
        }
        self.store.write_active_pointer(&catalog.generation)?;
        self.active = Some(catalog);
        self.cadence.catalog_changed();
        Ok(())
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
        } = context;
        let directive = self.cadence.observe(trigger, time.unix_ms)?;
        if directive.kind == PolicyRecomputeKind::Deferred {
            return Ok(PolicyCycle {
                directive,
                evaluation: None,
                pending_dispatch_intents: Vec::new(),
                detection_planning_signals: Vec::new(),
                measurement: None,
            });
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| request("policy_catalog_unavailable", "evaluate_policy_cycle"))?;
        let cost = policy_evaluation_cost(&active.compiled, facts, resources)?;
        let started = Instant::now();
        let mut evaluation = evaluate(&active.compiled, facts, resources, time, seed)
            .map_err(|_| request("policy_evaluation_rejected", "evaluate_policy_cycle"))?;
        procedure_manifest.bind_evaluation(&mut evaluation)?;
        let elapsed_micros = u64::try_from(started.elapsed().as_micros()).map_err(|_| {
            fatal(
                "policy_evaluation_measurement_overflow",
                "measure_policy_evaluation",
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
        let requested_recompute = directive.kind;
        Ok(PolicyCycle {
            directive,
            evaluation: Some(evaluation),
            pending_dispatch_intents,
            detection_planning_signals,
            measurement: Some(PolicyEvaluationMeasurement {
                sampled_at_monotonic_ms,
                elapsed_micros,
                requested_recompute,
                execution: PolicyEvaluationExecution::FullCatalogScan,
                cost,
            }),
        })
    }

    pub(crate) fn validate_dispatch(
        &self,
        intent: &DispatchIntent,
        reason_chain: &DecisionReasonChain,
        context: &PolicyAdmissionContext,
        owner_epoch: OwnerEpoch,
        ledger_high_watermark: u64,
    ) -> RuntimeHostResult<CatalogGeneration> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| request("policy_catalog_unavailable", "validate_policy_dispatch"))?;
        if intent.catalog_hash != active.generation.catalog_hash
            || intent.catalog_version != active.generation.catalog_version
        {
            return Err(request(
                "policy_catalog_mismatch",
                "validate_policy_dispatch",
            ));
        }
        let task = active
            .compiled
            .catalog()
            .tasks
            .tasks
            .iter()
            .find(|task| task.id == intent.task_id)
            .ok_or_else(|| request("policy_task_missing", "validate_policy_dispatch"))?;
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
                "policy_intent_catalog_mismatch",
                "validate_policy_dispatch",
            ));
        }
        if intent.input_ledger_position != context.fact_ledger_position
            || intent.input_ledger_position > ledger_high_watermark
            || intent.fact_snapshot_id != context.fact_snapshot_id
        {
            return Err(request(
                "policy_fact_position_mismatch",
                "validate_policy_dispatch",
            ));
        }
        if reason_chain.id != intent.reason_chain_id
            || reason_chain.decision_id != intent.decision_id
            || reason_chain.reasons.is_empty()
        {
            return Err(request(
                "policy_reason_chain_mismatch",
                "validate_policy_dispatch",
            ));
        }
        if intent
            .approval_refs
            .iter()
            .any(|approval| !context.approval_fact_ids.contains(approval))
        {
            return Err(request(
                "policy_approval_fact_missing",
                "validate_policy_dispatch",
            ));
        }
        if context.fencing_owner_epoch != owner_epoch || !intent.prerequisites.fencing_required {
            return Err(request(
                "policy_fencing_mismatch",
                "validate_policy_dispatch",
            ));
        }
        if context.now_unix_ms < intent.prerequisites.evaluated_at_unix_ms
            || intent
                .prerequisites
                .facts_fresh_until_unix_ms
                .is_some_and(|expires| context.now_unix_ms > expires)
        {
            return Err(request("policy_facts_stale", "validate_policy_dispatch"));
        }
        if intent.prerequisites.daily_limit == 0
            || intent.prerequisites.window_iteration_limit == 0
            || intent.prerequisites.max_runtime_ms == 0
            || intent.expected_duration_ms > intent.prerequisites.max_runtime_ms
        {
            return Err(request(
                "policy_budget_exhausted",
                "validate_policy_dispatch",
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
                "policy_decision_identity_conflict",
                "replay_policy_dispatch",
            ));
        }
        let catalog = self
            .store
            .load_generation(&seen.data.catalog_hash)?
            .generation;
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
    ) -> RuntimeHostResult<PolicyAdmissionRecord> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| request("policy_catalog_unavailable", "reserve_policy_budget"))?;
        self.control
            .preview_admission(&active.compiled, intent, now_unix_ms)
    }

    pub(crate) fn commit_admission(
        &mut self,
        intent: &DispatchIntent,
        admission: &PolicyAdmissionRecord,
    ) -> RuntimeHostResult<()> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| request("policy_catalog_unavailable", "commit_policy_budget"))?;
        self.control
            .commit_admission(&active.compiled, intent, admission)
    }

    pub(crate) fn refresh_dispatches(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        self.recover_dispatches(ledger)
    }

    pub(crate) fn pinned_catalog(&self, decision_id: &str) -> Option<CatalogGeneration> {
        self.pinned_dispatches.get(decision_id).cloned()
    }

    pub(crate) fn admitted_at(&self, decision_id: &str) -> RuntimeHostResult<u64> {
        self.seen_dispatches
            .get(decision_id)
            .and_then(|dispatch| dispatch.admission.as_ref())
            .map(|admission| admission.activity.admitted_at_unix_ms)
            .ok_or_else(|| {
                request(
                    "policy_dispatch_admission_missing",
                    "read_policy_admission_time",
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
                            "policy_dispatch_admission_missing",
                            "recover_policy_dispatch_clocks",
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
        let dispatch = self
            .seen_dispatches
            .get(decision_id)
            .ok_or_else(|| request("policy_dispatch_unknown", "replay_policy_execution_outcome"))?;
        let Some(existing) = &dispatch.execution else {
            return Ok(None);
        };
        if !execution_input_matches(&existing.outcome, input) {
            return Err(fatal(
                "policy_execution_identity_conflict",
                "replay_policy_execution_outcome",
            ));
        }
        Ok(Some(existing.clone()))
    }

    pub(crate) fn execution_instance_id(&self, decision_id: &str) -> RuntimeHostResult<&str> {
        self.seen_dispatches
            .get(decision_id)
            .map(|dispatch| dispatch.data.instance_id.as_str())
            .ok_or_else(|| request("policy_dispatch_unknown", "read_policy_dispatch_instance"))
    }

    pub(crate) fn active_performance_workloads(
        &self,
    ) -> RuntimeHostResult<Vec<PerformanceControlWorkload>> {
        let mut workloads = BTreeMap::new();
        for dispatch in self
            .seen_dispatches
            .values()
            .filter(|dispatch| dispatch.lifecycle == DispatchLifecycle::Admitted)
        {
            let admission = dispatch.admission.as_ref().ok_or_else(|| {
                fatal(
                    "policy_dispatch_admission_missing",
                    "read_active_performance_workloads",
                )
            })?;
            let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
            let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
            let workload = PerformanceControlWorkload {
                instance_id: intent.instance_id.clone(),
                load_profile: intent.load_profile,
            };
            if workloads
                .insert(workload.instance_id.clone(), workload)
                .is_some()
            {
                return Err(fatal(
                    "policy_instance_dispatch_conflict",
                    "read_active_performance_workloads",
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
                "policy_dispatch_not_pinned",
                "complete_policy_dispatch",
            ));
        }
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            fatal(
                "policy_dispatch_state_incomplete",
                "complete_policy_dispatch",
            )
        })?;
        let admission = dispatch.admission.clone().ok_or_else(|| {
            fatal(
                "policy_dispatch_admission_missing",
                "complete_policy_dispatch",
            )
        })?;
        if dispatch.execution.is_none() {
            return Err(request(
                "policy_execution_outcome_missing",
                "complete_policy_dispatch",
            ));
        }
        Ok((dispatch.data.clone(), admission))
    }

    pub(crate) fn complete_dispatch(&mut self, decision_id: &str) -> RuntimeHostResult<()> {
        if self.pinned_dispatches.remove(decision_id).is_none() {
            return Err(request(
                "policy_dispatch_not_pinned",
                "complete_policy_dispatch",
            ));
        }
        Ok(())
    }

    pub(crate) fn dispatch_needs_completion(&self, decision_id: &str) -> RuntimeHostResult<bool> {
        let dispatch = self
            .seen_dispatches
            .get(decision_id)
            .ok_or_else(|| request("policy_dispatch_unknown", "read_policy_dispatch_lifecycle"))?;
        match dispatch.lifecycle {
            DispatchLifecycle::Admitted => Ok(true),
            DispatchLifecycle::Completed => Ok(false),
            DispatchLifecycle::Intent | DispatchLifecycle::Rejected => Err(request(
                "policy_dispatch_not_admitted",
                "read_policy_dispatch_lifecycle",
            )),
        }
    }

    pub(crate) fn prepare_execution(
        &self,
        decision_id: &str,
        observed_at_unix_ms: u64,
        runtime_ms: u64,
        input: &PolicyExecutionInput,
        perf_context: &PerformanceContext,
    ) -> RuntimeHostResult<PolicyExecutionPreparation> {
        let dispatch = self.seen_dispatches.get(decision_id).ok_or_else(|| {
            request(
                "policy_dispatch_unknown",
                "prepare_policy_execution_outcome",
            )
        })?;
        if let Some(existing) = &dispatch.execution {
            if !execution_input_matches(&existing.outcome, input) {
                return Err(fatal(
                    "policy_execution_identity_conflict",
                    "prepare_policy_execution_outcome",
                ));
            }
            return Ok(PolicyExecutionPreparation::Replay(existing.clone()));
        }
        if dispatch.lifecycle != DispatchLifecycle::Admitted {
            return Err(request(
                "policy_dispatch_not_admitted",
                "prepare_policy_execution_outcome",
            ));
        }
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                "policy_dispatch_admission_missing",
                "prepare_policy_execution_outcome",
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
        self.control
            .preview_execution(
                &catalog.compiled,
                &intent,
                admission,
                PolicyExecutionTiming {
                    observed_at_unix_ms,
                    runtime_ms,
                },
                input,
                perf_context,
            )
            .map(PolicyExecutionPreparation::New)
    }

    pub(crate) fn commit_execution(
        &mut self,
        data: &PolicyExecutionEventData,
    ) -> RuntimeHostResult<()> {
        let dispatch = self
            .seen_dispatches
            .get_mut(&data.decision_id)
            .ok_or_else(|| fatal("policy_dispatch_unknown", "commit_policy_execution_outcome"))?;
        if dispatch.execution.is_some() {
            return Err(fatal(
                "policy_execution_duplicate",
                "commit_policy_execution_outcome",
            ));
        }
        let admission = dispatch.admission.as_ref().ok_or_else(|| {
            fatal(
                "policy_dispatch_admission_missing",
                "commit_policy_execution_outcome",
            )
        })?;
        let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
        let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
        self.control
            .commit_execution(&catalog.compiled, &intent, admission, data)?;
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

    pub(crate) fn commit_planning_signal(
        &mut self,
        sequence: u64,
        data: PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        self.validate_planning_signal(&data)?;
        if data.detection_budget.is_some()
            && !matches!(
                data.kind,
                PolicyPlanningSignalKind::DetectionReserved
                    | PolicyPlanningSignalKind::DetectionQuotaExhausted
            )
        {
            return Err(fatal(
                "policy_detection_budget_unexpected",
                "commit_policy_detection_budget",
            ));
        }
        let signal_already_projected = if let Some((existing_sequence, existing)) =
            self.store.load_planning_signal(&data.signal_id)?
        {
            if existing_sequence != sequence || existing != data {
                return Err(fatal(
                    "policy_planning_signal_identity_conflict",
                    "commit_policy_planning_signal",
                ));
            }
            true
        } else {
            false
        };
        let mut staged_quota = self.detection_quota.clone();
        let mut quota_already_projected = false;
        if let Some(budget) = &data.detection_budget
            && let Some((quota_sequence, usage)) = self
                .store
                .load_detection_quota(&data.instance_id, &budget.window_id)?
        {
            if quota_sequence > sequence
                || quota_sequence == sequence
                    && (!signal_already_projected
                        || usage.dispatch_used != budget.dispatch_used
                        || usage.runtime_reserved_ms != budget.runtime_reserved_ms)
            {
                return Err(fatal(
                    "policy_detection_quota_projection_conflict",
                    "commit_policy_planning_signal",
                ));
            }
            staged_quota
                .cache_usage((data.instance_id.clone(), budget.window_id.clone()), usage)?;
            quota_already_projected = quota_sequence == sequence;
        }
        if !quota_already_projected {
            staged_quota.commit_signal(&data)?;
        }
        self.store.persist_planning_signal(sequence, &data)?;
        if let Some(budget) = &data.detection_budget {
            let usage = staged_quota
                .windows
                .get(&(data.instance_id.clone(), budget.window_id.clone()))
                .copied()
                .unwrap_or_default();
            self.store.persist_detection_quota(
                sequence,
                &data.instance_id,
                &budget.window_id,
                usage,
            )?;
        }
        self.store.persist_planning_checkpoint(sequence)?;
        self.detection_quota = staged_quota;
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
                    "policy_detection_budget_profile_mismatch",
                    "validate_policy_detection_budget",
                )
            })?;
        let (_, expected_window_id) = active_activity_window(profile, data.observed_at_unix_ms)
            .map_err(|_| {
                fatal(
                    "policy_detection_budget_window_mismatch",
                    "validate_policy_detection_budget",
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
                "policy_detection_budget_profile_mismatch",
                "validate_policy_detection_budget",
            ));
        }
        Ok(())
    }

    fn recover_dispatches(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        let mut events = Vec::new();
        for event_type in [
            EventType::PolicyDispatchIntent,
            EventType::PolicyDispatchAdmitted,
            EventType::PolicyDispatchRejected,
            EventType::PolicyDispatchCompleted,
            EventType::PolicyExecutionRecorded,
        ] {
            events.extend(
                ledger
                    .query(EventQuery {
                        event_type: Some(event_type),
                        ..EventQuery::default()
                    })
                    .map_err(|_| fatal("policy_recovery_failed", "recover_policy_dispatches"))?,
            );
        }
        events.sort_by_key(|event| event.sequence());
        let mut seen_dispatches = BTreeMap::new();
        let mut dispatch_order = BTreeMap::new();
        let mut control = PolicyControlState::default();
        for event in events {
            let EventPayload::Policy(payload) = event.payload() else {
                continue;
            };
            match payload {
                PolicyPayload::DispatchIntent(payload) => {
                    let data = event_data(payload)?;
                    self.store.load_generation(payload.catalog_hash())?;
                    let record = SeenDispatch {
                        data,
                        admission: None,
                        execution: None,
                        intent_sequence: event.sequence(),
                        admitted_sequence: None,
                        rejected_sequence: None,
                        completed_sequence: None,
                        lifecycle: DispatchLifecycle::Intent,
                    };
                    if seen_dispatches
                        .insert(payload.decision_id().to_owned(), record)
                        .is_some()
                    {
                        return Err(fatal(
                            "policy_decision_identity_conflict",
                            "recover_policy_dispatches",
                        ));
                    }
                    if dispatch_order
                        .insert(event.sequence(), payload.decision_id().to_owned())
                        .is_some()
                    {
                        return Err(fatal(
                            "policy_dispatch_sequence_conflict",
                            "recover_policy_dispatches",
                        ));
                    }
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
                            "policy_dispatch_admission_missing",
                            "recover_policy_dispatches",
                        )
                    })?;
                    let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
                    let intent = control_intent(&dispatch.data, &catalog.compiled, &admission)?;
                    control.commit_admission(&catalog.compiled, &intent, &admission)?;
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
                            "policy_dispatch_completion_incomplete",
                            "recover_policy_dispatches",
                        ));
                    }
                }
                PolicyPayload::ExecutionRecorded(payload) => {
                    let data = execution_event_data(payload);
                    let dispatch =
                        seen_dispatches
                            .get_mut(payload.decision_id())
                            .ok_or_else(|| {
                                fatal("policy_dispatch_intent_missing", "recover_policy_execution")
                            })?;
                    if dispatch.lifecycle != DispatchLifecycle::Admitted
                        || dispatch.execution.is_some()
                    {
                        return Err(fatal(
                            "policy_execution_lifecycle_invalid",
                            "recover_policy_execution",
                        ));
                    }
                    let admission = dispatch.admission.as_ref().ok_or_else(|| {
                        fatal(
                            "policy_dispatch_admission_missing",
                            "recover_policy_execution",
                        )
                    })?;
                    let catalog = self.store.load_generation(&dispatch.data.catalog_hash)?;
                    let intent = control_intent(&dispatch.data, &catalog.compiled, admission)?;
                    control.commit_execution(&catalog.compiled, &intent, admission, &data)?;
                    dispatch.execution = Some(data);
                }
                PolicyPayload::PlanningSignalObserved(_) => {
                    return Err(fatal(
                        "policy_recovery_query_mismatch",
                        "recover_policy_dispatches",
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
                    .generation;
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
            .map_err(|_| fatal("policy_recovery_failed", "recover_policy_planning_signals"))?;
        let mut after_sequence = self.store.load_planning_checkpoint()?.unwrap_or(0);
        if after_sequence > latest {
            return Err(fatal(
                "policy_planning_checkpoint_ahead",
                "recover_policy_planning_signals",
            ));
        }
        while after_sequence < latest {
            let events = ledger
                .query_page(
                    EventQuery {
                        event_type: Some(EventType::PolicyPlanningSignalObserved),
                        ..EventQuery::default()
                    },
                    after_sequence,
                    latest,
                    PLANNING_SIGNAL_RECOVERY_PAGE_EVENTS,
                )
                .map_err(|_| fatal("policy_recovery_failed", "recover_policy_planning_signals"))?;
            if events.is_empty() {
                break;
            }
            for event in events {
                let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload)) =
                    event.payload()
                else {
                    return Err(fatal(
                        "policy_recovery_query_mismatch",
                        "recover_policy_planning_signals",
                    ));
                };
                let sequence = event.sequence();
                self.commit_planning_signal(sequence, planning_signal_event_data(payload))?;
                after_sequence = sequence;
            }
        }
        if latest > 0 && after_sequence < latest {
            self.store.persist_planning_checkpoint(latest)?;
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
                "policy_detection_budget_overflow",
                "reserve_policy_detection_budget",
            )
        })?;
        let next_runtime = usage
            .runtime_reserved_ms
            .checked_add(profile.detection_budget.expected_duration_ms)
            .ok_or_else(|| {
                fatal(
                    "policy_detection_budget_overflow",
                    "reserve_policy_detection_budget",
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
                    "policy_detection_budget_unexpected",
                    "commit_policy_detection_budget",
                ))
            };
        }
        let budget = data.detection_budget.as_ref().ok_or_else(|| {
            fatal(
                "policy_detection_budget_missing",
                "commit_policy_detection_budget",
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
                        "policy_detection_budget_overflow",
                        "commit_policy_detection_budget",
                    )
                })?;
                let expected_runtime = current
                    .runtime_reserved_ms
                    .checked_add(budget.reservation_ms)
                    .ok_or_else(|| {
                        fatal(
                            "policy_detection_budget_overflow",
                            "commit_policy_detection_budget",
                        )
                    })?;
                if budget.dispatch_used != expected_dispatch
                    || budget.runtime_reserved_ms != expected_runtime
                    || budget.dispatch_used > budget.dispatch_limit
                    || budget.runtime_reserved_ms > budget.runtime_limit_ms
                {
                    return Err(fatal(
                        "policy_detection_budget_receipt_mismatch",
                        "commit_policy_detection_budget",
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
                        "policy_detection_budget_receipt_mismatch",
                        "commit_policy_detection_budget",
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
                    "policy_detection_quota_cache_invariant",
                    "cache_policy_detection_quota",
                )
            })?;
            if self.windows.remove(&oldest).is_none() {
                return Err(fatal(
                    "policy_detection_quota_cache_invariant",
                    "cache_policy_detection_quota",
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
            "policy_detection_time_invalid",
            "plan_policy_detection",
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
            .ok_or_else(|| fatal("policy_detection_instance_missing", "plan_policy_detection"))?;
        let profile = matching_activity_profile(&catalog.catalog().activity.profiles, instance)
            .ok_or_else(|| request("policy_activity_profile_missing", "plan_policy_detection"))?;
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
            let (_, window_id) = active_activity_window(profile, now_unix_ms)?;
            staged.ensure_loaded(store, instance_id, &window_id)?;
            let (kind, budget) =
                match staged.preview(profile, catalog.catalog_hash(), instance_id, now_unix_ms) {
                    Ok(value) => value,
                    Err(error) if error.code() == "policy_activity_window_closed" => continue,
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
                fact_code: format!("{}.{}", suggestion.fact_key, suggestion.reason),
                observed_at_unix_ms: now_unix_ms,
                detection_budget: Some(budget),
            };
            staged.commit_signal(&signal)?;
            signals.push(signal);
        }
    }
    Ok(signals)
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
    for digest in [payload.package_digest(), payload.procedure_binding_digest()] {
        let valid = digest.strip_prefix("sha256:").is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
        if !valid {
            return Err(fatal(
                "procedure_binding_event_invalid",
                "recover_policy_dispatches",
            ));
        }
    }
    Ok(PolicyDispatchEventData {
        decision_id: payload.decision_id().to_owned(),
        task_id: payload.task_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        operation_id: payload.operation_id().to_owned(),
        package_digest: payload.package_digest().to_owned(),
        procedure_binding_digest: payload.procedure_binding_digest().to_owned(),
        reason_chain_id: payload.reason_chain_id().to_owned(),
        reasons: payload
            .reasons()
            .iter()
            .map(|reason| PolicyReasonRecord {
                code: reason.code.clone(),
                detail: reason.detail.clone(),
            })
            .collect(),
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
    let package_digest = intent.package_digest.clone().ok_or_else(|| {
        fatal(
            "procedure_package_digest_missing",
            "build_policy_dispatch_event",
        )
    })?;
    let procedure_binding_digest = intent.procedure_binding_digest.clone().ok_or_else(|| {
        fatal(
            "procedure_binding_digest_missing",
            "build_policy_dispatch_event",
        )
    })?;
    Ok(PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
        package_digest,
        procedure_binding_digest,
        reason_chain_id: reason_chain.id.clone(),
        reasons: reason_chain
            .reasons
            .iter()
            .map(|reason| PolicyReasonRecord {
                code: reason.code.clone(),
                detail: reason.detail.clone(),
            })
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
            "policy_dispatch_intent_missing",
            "recover_policy_dispatches",
        ));
    };
    if intent.data != event_data(payload)?
        || intent.lifecycle != expected
        || sequence <= intent.intent_sequence
    {
        return Err(fatal(
            "policy_dispatch_lifecycle_invalid",
            "recover_policy_dispatches",
        ));
    }
    match next {
        DispatchLifecycle::Admitted => intent.admitted_sequence = Some(sequence),
        DispatchLifecycle::Rejected => intent.rejected_sequence = Some(sequence),
        DispatchLifecycle::Completed => intent.completed_sequence = Some(sequence),
        DispatchLifecycle::Intent => {
            return Err(fatal(
                "policy_dispatch_lifecycle_invalid",
                "recover_policy_dispatches",
            ));
        }
    }
    intent.lifecycle = next;
    Ok(intent)
}

fn execution_event_data(
    payload: &actingcommand_contract::PolicyExecutionPayload,
) -> PolicyExecutionEventData {
    PolicyExecutionEventData {
        decision_id: payload.decision_id().to_owned(),
        task_id: payload.task_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        observed_at_unix_ms: payload.observed_at_unix_ms(),
        outcome: payload.outcome().clone(),
    }
}

fn planning_signal_event_data(
    payload: &actingcommand_contract::PolicyPlanningSignalPayload,
) -> PolicyPlanningSignalEventData {
    PolicyPlanningSignalEventData {
        signal_id: payload.signal_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        task_id: payload.task_id().map(str::to_owned),
        kind: payload.kind(),
        fact_code: payload.fact_code().to_owned(),
        observed_at_unix_ms: payload.observed_at_unix_ms(),
        detection_budget: payload.detection_budget().cloned(),
    }
}

fn control_intent(
    data: &PolicyDispatchEventData,
    catalog: &CompiledCatalog,
    admission: &PolicyAdmissionRecord,
) -> RuntimeHostResult<DispatchIntent> {
    let task = catalog
        .catalog()
        .tasks
        .tasks
        .iter()
        .find(|task| task.id == data.task_id)
        .ok_or_else(|| fatal("policy_task_missing", "recover_policy_control"))?;
    Ok(DispatchIntent {
        decision_id: data.decision_id.clone(),
        task_id: data.task_id.clone(),
        instance_id: data.instance_id.clone(),
        operation_id: data.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        package_digest: Some(data.package_digest.clone()),
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
        (
            PolicyExecutionOutcome::Failed { failure },
            PolicyExecutionInput::Failed { error_code, class },
        ) => {
            !failure.reported_success
                && failure.error_code == *error_code
                && failure.original_class == *class
        }
        _ => false,
    }
}

struct CatalogStore {
    root: PathBuf,
    generations: PathBuf,
    legacy_active_pointer: PathBuf,
    state: Arc<RuntimeStateStore>,
}

impl CatalogStore {
    fn open(state_root: &Path, state: Arc<RuntimeStateStore>) -> RuntimeHostResult<Self> {
        let root = state_root.join("policy").join("catalogs");
        let generations = root.join(GENERATIONS_DIR);
        fs::create_dir_all(&generations)
            .map_err(|_| fatal("catalog_state_create_failed", "open_catalog_store"))?;
        let store = Self {
            legacy_active_pointer: root.join(ACTIVE_POINTER_FILE),
            root,
            generations,
            state,
        };
        store.migrate_legacy_active_pointer()?;
        Ok(store)
    }

    fn stage(&self, sources: &CatalogSources) -> RuntimeHostResult<LoadedCatalog> {
        let compiled = compile_catalog(sources)
            .map_err(|_| request("catalog_compile_failed", "stage_catalog_generation"))?;
        let generation = generation_from(&compiled, sources);
        let path = self.generation_path(&generation.catalog_hash)?;
        if path.exists() {
            return self.load_generation(&generation.catalog_hash);
        }
        let temporary = self.generations.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&temporary).map_err(|_| {
            fatal(
                "catalog_generation_create_failed",
                "stage_catalog_generation",
            )
        })?;
        let result = (|| {
            write_new_file(&temporary.join("tasks.json"), &sources.tasks.bytes)?;
            write_new_file(&temporary.join("pools.json"), &sources.pools.bytes)?;
            write_new_file(&temporary.join("activity.json"), &sources.activity.bytes)?;
            write_new_file(&temporary.join("timeline.json"), &sources.timeline.bytes)?;
            let manifest = serde_json::to_vec_pretty(&generation)
                .map_err(|_| fatal("catalog_manifest_encode_failed", "stage_catalog_generation"))?;
            write_new_file(&temporary.join("manifest.json"), &manifest)?;
            fs::rename(&temporary, &path).map_err(|_| {
                fatal(
                    "catalog_generation_publish_failed",
                    "stage_catalog_generation",
                )
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            remove_directory_if_exists(&temporary)?;
            return Err(error);
        }
        self.load_generation(&generation.catalog_hash)
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
                "catalog_pointer_version_unsupported",
                "load_active_catalog",
            ));
        }
        let pointer: CatalogPointer = serde_json::from_slice(document.payload())
            .map_err(|_| fatal("catalog_pointer_invalid", "load_active_catalog"))?;
        if pointer.schema_version != CATALOG_STATE_SCHEMA {
            return Err(fatal(
                "catalog_pointer_version_unsupported",
                "load_active_catalog",
            ));
        }
        let loaded = self.load_generation(&pointer.generation.catalog_hash)?;
        if loaded.generation != pointer.generation {
            return Err(fatal(
                "catalog_pointer_generation_mismatch",
                "load_active_catalog",
            ));
        }
        Ok(Some(loaded))
    }

    fn load_generation(&self, hash: &str) -> RuntimeHostResult<LoadedCatalog> {
        let path = self.generation_path(hash)?;
        let manifest = read_bounded(&path.join("manifest.json"), MAX_MANIFEST_BYTES)?;
        let generation: CatalogGeneration = serde_json::from_slice(&manifest)
            .map_err(|_| fatal("catalog_manifest_invalid", "load_catalog_generation"))?;
        if generation.schema_version != CATALOG_STATE_SCHEMA || generation.catalog_hash != hash {
            return Err(fatal(
                "catalog_generation_identity_mismatch",
                "load_catalog_generation",
            ));
        }
        let sources = CatalogSources {
            tasks: self.load_source(&path, &generation, "tasks")?,
            pools: self.load_source(&path, &generation, "pools")?,
            activity: self.load_source(&path, &generation, "activity")?,
            timeline: self.load_source(&path, &generation, "timeline")?,
        };
        let compiled = compile_catalog(&sources)
            .map_err(|_| fatal("catalog_generation_invalid", "load_catalog_generation"))?;
        if compiled.catalog_hash() != generation.catalog_hash
            || compiled.summary().catalog_id != generation.catalog_id
            || compiled.summary().catalog_version != generation.catalog_version
        {
            return Err(fatal(
                "catalog_generation_content_mismatch",
                "load_catalog_generation",
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
        let Some(entry) = self
            .state
            .read_projection_entry(
                PLANNING_SIGNAL_PROJECTION_NAMESPACE,
                &planning_signal_projection_key(signal_id),
            )
            .map_err(|error| RuntimeHostError::state(&error))?
        else {
            return Ok(None);
        };
        let stored: StoredPlanningSignal =
            serde_json::from_slice(entry.payload()).map_err(|_| {
                fatal(
                    "policy_planning_signal_projection_invalid",
                    "load_policy_planning_signal",
                )
            })?;
        if stored.schema_version != PLANNING_SIGNAL_PROJECTION_SCHEMA
            || stored.signal_id != signal_id
        {
            return Err(fatal(
                "policy_planning_signal_projection_invalid",
                "load_policy_planning_signal",
            ));
        }
        Ok(Some((entry.ledger_sequence(), stored.into_data())))
    }

    fn persist_planning_signal(
        &self,
        sequence: u64,
        data: &PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        if let Some((existing_sequence, existing)) = self.load_planning_signal(&data.signal_id)? {
            if existing_sequence != sequence || existing != *data {
                return Err(fatal(
                    "policy_planning_signal_identity_conflict",
                    "persist_policy_planning_signal",
                ));
            }
            return Ok(());
        }
        let payload = serde_json::to_vec(&StoredPlanningSignal::from_data(data)).map_err(|_| {
            fatal(
                "policy_planning_signal_projection_encode_failed",
                "persist_policy_planning_signal",
            )
        })?;
        self.state
            .write_projection_entry(
                PLANNING_SIGNAL_PROJECTION_NAMESPACE,
                &planning_signal_projection_key(&data.signal_id),
                sequence,
                &payload,
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        Ok(())
    }

    fn load_detection_quota(
        &self,
        instance_id: &str,
        window_id: &str,
    ) -> RuntimeHostResult<Option<(u64, DetectionQuotaUsage)>> {
        let Some(entry) = self
            .state
            .read_projection_entry(
                DETECTION_QUOTA_PROJECTION_NAMESPACE,
                &detection_quota_projection_key(instance_id, window_id),
            )
            .map_err(|error| RuntimeHostError::state(&error))?
        else {
            return Ok(None);
        };
        let stored: StoredDetectionQuota =
            serde_json::from_slice(entry.payload()).map_err(|_| {
                fatal(
                    "policy_detection_quota_projection_invalid",
                    "load_policy_detection_quota",
                )
            })?;
        if stored.schema_version != DETECTION_QUOTA_PROJECTION_SCHEMA
            || stored.instance_id != instance_id
            || stored.window_id != window_id
        {
            return Err(fatal(
                "policy_detection_quota_projection_invalid",
                "load_policy_detection_quota",
            ));
        }
        Ok(Some((
            entry.ledger_sequence(),
            DetectionQuotaUsage {
                dispatch_used: stored.dispatch_used,
                runtime_reserved_ms: stored.runtime_reserved_ms,
            },
        )))
    }

    fn persist_detection_quota(
        &self,
        sequence: u64,
        instance_id: &str,
        window_id: &str,
        usage: DetectionQuotaUsage,
    ) -> RuntimeHostResult<()> {
        if let Some((existing_sequence, existing)) =
            self.load_detection_quota(instance_id, window_id)?
        {
            if existing_sequence > sequence || existing_sequence == sequence && existing != usage {
                return Err(fatal(
                    "policy_detection_quota_projection_conflict",
                    "persist_policy_detection_quota",
                ));
            }
            if existing_sequence == sequence {
                return Ok(());
            }
        }
        let payload = serde_json::to_vec(&StoredDetectionQuota {
            schema_version: DETECTION_QUOTA_PROJECTION_SCHEMA.to_owned(),
            instance_id: instance_id.to_owned(),
            window_id: window_id.to_owned(),
            dispatch_used: usage.dispatch_used,
            runtime_reserved_ms: usage.runtime_reserved_ms,
        })
        .map_err(|_| {
            fatal(
                "policy_detection_quota_projection_encode_failed",
                "persist_policy_detection_quota",
            )
        })?;
        self.state
            .write_projection_entry(
                DETECTION_QUOTA_PROJECTION_NAMESPACE,
                &detection_quota_projection_key(instance_id, window_id),
                sequence,
                &payload,
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        Ok(())
    }

    fn load_planning_checkpoint(&self) -> RuntimeHostResult<Option<u64>> {
        let Some(entry) = self
            .state
            .read_projection_entry(
                PLANNING_SIGNAL_PROJECTION_NAMESPACE,
                PLANNING_SIGNAL_CHECKPOINT_KEY,
            )
            .map_err(|error| RuntimeHostError::state(&error))?
        else {
            return Ok(None);
        };
        let checkpoint: PlanningSignalCheckpoint = serde_json::from_slice(entry.payload())
            .map_err(|_| {
                fatal(
                    "policy_planning_checkpoint_invalid",
                    "load_policy_planning_checkpoint",
                )
            })?;
        if checkpoint.schema_version != PLANNING_SIGNAL_PROJECTION_SCHEMA
            || checkpoint.through_sequence != entry.ledger_sequence()
        {
            return Err(fatal(
                "policy_planning_checkpoint_invalid",
                "load_policy_planning_checkpoint",
            ));
        }
        Ok(Some(checkpoint.through_sequence))
    }

    fn persist_planning_checkpoint(&self, through_sequence: u64) -> RuntimeHostResult<()> {
        if let Some(existing) = self.load_planning_checkpoint()? {
            if existing > through_sequence {
                return Err(fatal(
                    "policy_planning_checkpoint_ahead",
                    "persist_policy_planning_checkpoint",
                ));
            }
            if existing == through_sequence {
                return Ok(());
            }
        }
        let payload = serde_json::to_vec(&PlanningSignalCheckpoint {
            schema_version: PLANNING_SIGNAL_PROJECTION_SCHEMA.to_owned(),
            through_sequence,
        })
        .map_err(|_| {
            fatal(
                "policy_planning_checkpoint_encode_failed",
                "persist_policy_planning_checkpoint",
            )
        })?;
        self.state
            .write_projection_entry(
                PLANNING_SIGNAL_PROJECTION_NAMESPACE,
                PLANNING_SIGNAL_CHECKPOINT_KEY,
                through_sequence,
                &payload,
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        Ok(())
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
            .ok_or_else(|| fatal("catalog_source_missing", "load_catalog_generation"))?;
        let bytes = read_bounded(
            &path.join(format!("{kind}.json")),
            actingcommand_policy::MAX_DOCUMENT_BYTES,
        )?;
        if sha256(&bytes) != record.sha256 {
            return Err(fatal(
                "catalog_source_hash_mismatch",
                "load_catalog_generation",
            ));
        }
        Ok(CatalogDocumentSource::new(record.source_uri.clone(), bytes))
    }

    fn write_active_pointer(&self, generation: &CatalogGeneration) -> RuntimeHostResult<()> {
        let pointer = CatalogPointer {
            schema_version: CATALOG_STATE_SCHEMA.to_owned(),
            generation: generation.clone(),
        };
        let bytes = serde_json::to_vec(&pointer)
            .map_err(|_| fatal("catalog_pointer_encode_failed", "switch_active_catalog"))?;
        let current = self
            .state
            .read_json_document(ACTIVE_POINTER_STATE_KEY)
            .map_err(|error| RuntimeHostError::state(&error))?;
        self.state
            .write_json_document(
                ACTIVE_POINTER_STATE_KEY,
                CATALOG_STATE_SCHEMA,
                &bytes,
                current.as_ref().map(|document| document.payload_sha256()),
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        Ok(())
    }

    fn migrate_legacy_active_pointer(&self) -> RuntimeHostResult<()> {
        if !self.legacy_active_pointer.exists() {
            return Ok(());
        }
        let bytes = read_bounded(&self.legacy_active_pointer, MAX_POINTER_BYTES)?;
        let pointer: CatalogPointer = serde_json::from_slice(&bytes)
            .map_err(|_| fatal("catalog_pointer_invalid", "migrate_active_catalog"))?;
        if pointer.schema_version != CATALOG_STATE_SCHEMA {
            return Err(fatal(
                "catalog_pointer_version_unsupported",
                "migrate_active_catalog",
            ));
        }
        let loaded = self.load_generation(&pointer.generation.catalog_hash)?;
        if loaded.generation != pointer.generation {
            return Err(fatal(
                "catalog_pointer_generation_mismatch",
                "migrate_active_catalog",
            ));
        }
        let canonical = serde_json::to_vec(&pointer)
            .map_err(|_| fatal("catalog_pointer_encode_failed", "migrate_active_catalog"))?;
        self.state
            .migrate_legacy_json_document(
                ACTIVE_POINTER_STATE_KEY,
                LEGACY_CATALOG_POINTER_SCHEMA,
                CATALOG_STATE_SCHEMA,
                &canonical,
            )
            .map_err(|error| RuntimeHostError::state(&error))?;
        fs::remove_file(&self.legacy_active_pointer)
            .map_err(|_| fatal("catalog_pointer_cleanup_failed", "migrate_active_catalog"))?;
        sync_directory(&self.root, "migrate_active_catalog")
    }

    fn generation_path(&self, hash: &str) -> RuntimeHostResult<PathBuf> {
        let digest = hash
            .strip_prefix("sha256:")
            .ok_or_else(|| request("catalog_hash_invalid", "resolve_catalog_generation"))?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(request(
                "catalog_hash_invalid",
                "resolve_catalog_generation",
            ));
        }
        Ok(self.generations.join(digest))
    }
}

fn generation_from(compiled: &CompiledCatalog, sources: &CatalogSources) -> CatalogGeneration {
    let summary = compiled.summary();
    CatalogGeneration {
        schema_version: CATALOG_STATE_SCHEMA.to_owned(),
        catalog_id: summary.catalog_id.clone(),
        catalog_version: summary.catalog_version,
        catalog_hash: summary.catalog_hash.clone(),
        sources: vec![
            source_record("tasks", &sources.tasks),
            source_record("pools", &sources.pools),
            source_record("activity", &sources.activity),
            source_record("timeline", &sources.timeline),
        ],
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

fn planning_signal_projection_key(signal_id: &str) -> String {
    format!("{:x}", Sha256::digest(signal_id.as_bytes()))
}

fn detection_quota_projection_key(instance_id: &str, window_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(instance_id.as_bytes());
    digest.update([0]);
    digest.update(window_id.as_bytes());
    format!("{:x}", digest.finalize())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> RuntimeHostResult<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| fatal("catalog_file_create_failed", "write_catalog_state"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| fatal("catalog_file_write_failed", "write_catalog_state"))
}

fn read_bounded(path: &Path, maximum_bytes: usize) -> RuntimeHostResult<Vec<u8>> {
    let metadata =
        fs::metadata(path).map_err(|_| fatal("catalog_file_unavailable", "read_catalog_state"))?;
    if metadata.len() > maximum_bytes as u64 {
        return Err(fatal("catalog_file_too_large", "read_catalog_state"));
    }
    fs::read(path).map_err(|_| fatal("catalog_file_read_failed", "read_catalog_state"))
}

fn remove_directory_if_exists(path: &Path) -> RuntimeHostResult<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(fatal(
            "catalog_temporary_cleanup_failed",
            "stage_catalog_generation",
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
        "policy_evaluation_measurement_overflow",
        "measure_policy_evaluation",
    )
}

#[cfg(unix)]
fn sync_directory(path: &Path, operation: &'static str) -> RuntimeHostResult<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| fatal("catalog_directory_sync_failed", operation))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path, _operation: &'static str) -> RuntimeHostResult<()> {
    // Rust's standard library cannot open Windows directories for fsync without unsafe flags.
    Ok(())
}

fn fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}

fn request(code: &'static str, operation: &'static str) -> RuntimeHostError {
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
