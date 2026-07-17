// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned catalog generations and replayable policy admission state.

use crate::policy_control::{PolicyControlState, PolicyExecutionInput};
use crate::{PerformanceControlWorkload, RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    EventPayload, LeaseToken, OwnerEpoch, PerformanceContext, PolicyAdmissionRecord,
    PolicyDispatchEventData, PolicyExecutionEventData, PolicyExecutionOutcome, PolicyPayload,
    PolicyPlanningSignalEventData, PolicyReasonRecord, RuntimeErrorCode,
};
use actingcommand_ledger::GlobalLedger;
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, DecisionReasonChain, DispatchIntent,
    DispatchPrerequisites, EvaluationFacts, EvaluationResources, EvaluationTime, PolicyEvaluation,
    ScopeSelector, compile_catalog, evaluate,
};
use actingcommand_runtime_state::RuntimeStateStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const CATALOG_STATE_SCHEMA: &str = "actingcommand.catalog-state.v1";
const ACTIVE_POINTER_FILE: &str = "active.json";
const ACTIVE_POINTER_STATE_KEY: &str = "policy.catalog.active";
const GENERATIONS_DIR: &str = "generations";
const MAX_POINTER_BYTES: usize = 16 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const DEFAULT_DEBOUNCE_MS: u64 = 250;
const DEFAULT_COOLDOWN_MS: u64 = 1_000;
const DEFAULT_RECONCILIATION_INTERVAL_MS: u64 = 60_000;
const DEFAULT_CLOCK_JUMP_THRESHOLD_MS: u64 = 5_000;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyCycle {
    pub directive: PolicyRecomputeDirective,
    pub evaluation: Option<PolicyEvaluation>,
    pub pending_dispatch_intents: Vec<DispatchIntent>,
}

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchLifecycle {
    Intent,
    Admitted,
    Rejected,
    Completed,
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
    pinned_dispatches: BTreeMap<String, CatalogGeneration>,
    planning_signals: BTreeMap<String, PolicyPlanningSignalEventData>,
    control: PolicyControlState,
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
            pinned_dispatches: BTreeMap::new(),
            planning_signals: BTreeMap::new(),
            control: PolicyControlState::default(),
        };
        host.recover_dispatches(ledger)?;
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

    pub(crate) fn project_dispatches(&self) -> Vec<PolicyDispatchProjection> {
        self.seen_dispatches
            .values()
            .map(|dispatch| PolicyDispatchProjection {
                data: dispatch.data.clone(),
                state: match dispatch.lifecycle {
                    DispatchLifecycle::Intent => PolicyDispatchProjectionState::Intent,
                    DispatchLifecycle::Admitted => PolicyDispatchProjectionState::Admitted,
                    DispatchLifecycle::Rejected => PolicyDispatchProjectionState::Rejected,
                    DispatchLifecycle::Completed => PolicyDispatchProjectionState::Completed,
                },
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
        time: EvaluationTime,
        seed: u64,
        trigger: PolicyTrigger,
    ) -> RuntimeHostResult<PolicyCycle> {
        let directive = self.cadence.observe(trigger, time.unix_ms)?;
        if directive.kind == PolicyRecomputeKind::Deferred {
            return Ok(PolicyCycle {
                directive,
                evaluation: None,
                pending_dispatch_intents: Vec::new(),
            });
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| request("policy_catalog_unavailable", "evaluate_policy_cycle"))?;
        let evaluation = evaluate(&active.compiled, facts, resources, time, seed)
            .map_err(|_| request("policy_evaluation_rejected", "evaluate_policy_cycle"))?;
        let pending_dispatch_intents = evaluation
            .dispatch_intents
            .iter()
            .filter(|intent| !self.seen_dispatches.contains_key(&intent.decision_id))
            .cloned()
            .collect();
        Ok(PolicyCycle {
            directive,
            evaluation: Some(evaluation),
            pending_dispatch_intents,
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
        if seen.data != dispatch_event_data(intent, reason_chain) {
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
            if existing.observed_at_unix_ms != observed_at_unix_ms
                || !execution_input_matches(&existing.outcome, input)
            {
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
                observed_at_unix_ms,
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
    ) -> Option<&PolicyPlanningSignalEventData> {
        self.planning_signals.get(signal_id)
    }

    pub(crate) fn commit_planning_signal(
        &mut self,
        data: PolicyPlanningSignalEventData,
    ) -> RuntimeHostResult<()> {
        if let Some(existing) = self.planning_signals.get(&data.signal_id) {
            return if existing == &data {
                Ok(())
            } else {
                Err(fatal(
                    "policy_planning_signal_identity_conflict",
                    "commit_policy_planning_signal",
                ))
            };
        }
        self.planning_signals.insert(data.signal_id.clone(), data);
        Ok(())
    }

    fn recover_dispatches(&mut self, ledger: &GlobalLedger) -> RuntimeHostResult<()> {
        let events = ledger
            .query(Default::default())
            .map_err(|_| fatal("policy_recovery_failed", "recover_policy_dispatches"))?;
        let mut seen_dispatches = BTreeMap::new();
        let mut planning_signals = BTreeMap::new();
        let mut control = PolicyControlState::default();
        for event in events {
            let EventPayload::Policy(payload) = event.payload() else {
                continue;
            };
            match payload {
                PolicyPayload::DispatchIntent(payload) => {
                    let data = event_data(payload);
                    self.store.load_generation(payload.catalog_hash())?;
                    let record = SeenDispatch {
                        data,
                        admission: None,
                        execution: None,
                        intent_sequence: event.sequence(),
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
                }
                PolicyPayload::DispatchAdmitted(payload) => {
                    let dispatch = transition_dispatch(
                        &mut seen_dispatches,
                        payload,
                        DispatchLifecycle::Intent,
                        DispatchLifecycle::Admitted,
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
                    )?;
                }
                PolicyPayload::DispatchCompleted(payload) => {
                    let dispatch = transition_dispatch(
                        &mut seen_dispatches,
                        payload,
                        DispatchLifecycle::Admitted,
                        DispatchLifecycle::Completed,
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
                PolicyPayload::PlanningSignalObserved(payload) => {
                    let data = planning_signal_event_data(payload);
                    if planning_signals
                        .insert(data.signal_id.clone(), data)
                        .is_some()
                    {
                        return Err(fatal(
                            "policy_planning_signal_identity_conflict",
                            "recover_policy_planning_signals",
                        ));
                    }
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
        self.pinned_dispatches = pinned_dispatches;
        self.planning_signals = planning_signals;
        self.control = control;
        Ok(())
    }
}

fn event_data(payload: &actingcommand_contract::PolicyDispatchPayload) -> PolicyDispatchEventData {
    PolicyDispatchEventData {
        decision_id: payload.decision_id().to_owned(),
        task_id: payload.task_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        operation_id: payload.operation_id().to_owned(),
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
    }
}

fn dispatch_event_data(
    intent: &DispatchIntent,
    reason_chain: &DecisionReasonChain,
) -> PolicyDispatchEventData {
    PolicyDispatchEventData {
        decision_id: intent.decision_id.clone(),
        task_id: intent.task_id.clone(),
        instance_id: intent.instance_id.clone(),
        operation_id: intent.operation_id.clone(),
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
    }
}

fn transition_dispatch<'a>(
    seen: &'a mut BTreeMap<String, SeenDispatch>,
    payload: &actingcommand_contract::PolicyDispatchPayload,
    expected: DispatchLifecycle,
    next: DispatchLifecycle,
) -> RuntimeHostResult<&'a mut SeenDispatch> {
    let Some(intent) = seen.get_mut(payload.decision_id()) else {
        return Err(fatal(
            "policy_dispatch_intent_missing",
            "recover_policy_dispatches",
        ));
    };
    if intent.data != event_data(payload) || intent.lifecycle != expected {
        return Err(fatal(
            "policy_dispatch_lifecycle_invalid",
            "recover_policy_dispatches",
        ));
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
        catalog_hash: data.catalog_hash.clone(),
        catalog_version: data.catalog_version,
        input_ledger_position: data.input_ledger_position,
        fact_snapshot_id: data.fact_snapshot_id.clone(),
        approval_refs: data.approval_fact_ids.clone(),
        reason_chain_id: data.reason_chain_id.clone(),
        expected_duration_ms: task.expected_duration_ms,
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
                CATALOG_STATE_SCHEMA,
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
