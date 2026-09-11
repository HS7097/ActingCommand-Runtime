// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    EventActor, EventPayload, EventSource, EventType, OriginModule, PolicyDetectionBudgetRecord,
    PolicyPayload, PolicyPlanningSignalEventData, PolicyPlanningSignalKind,
};
use actingcommand_ledger::{PersistedEvent, PlanningSignalRecoveryPage};
use actingcommand_runtime_database::RuntimeTransaction;
use serde::{Deserialize, Serialize};

pub const PLANNING_SIGNAL_PROJECTION_NAMESPACE: &str = "policy.planning-signal.v1";
pub const DETECTION_QUOTA_PROJECTION_NAMESPACE: &str = "policy.detection-quota.v1";
const CHECKPOINT_KEY: &str = "checkpoint";
const SIGNAL_SCHEMA: &str = "actingcommand.policy-planning-signal.v1";
const QUOTA_SCHEMA: &str = "actingcommand.policy-detection-quota.v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanningQuotaUsage {
    pub dispatch_used: u32,
    pub runtime_reserved_ms: u64,
}

#[derive(Serialize, Deserialize)]
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
            schema_version: SIGNAL_SCHEMA.to_owned(),
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDetectionQuota {
    schema_version: String,
    instance_id: String,
    window_id: String,
    dispatch_used: u32,
    runtime_reserved_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningSignalCheckpoint {
    schema_version: String,
    through_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanningStateObservation {
    Applied,
    Unchanged,
    Unknown,
}

type PlanningKey = (&'static str, String);
type PlanningEntries = BTreeMap<PlanningKey, Option<ProjectionEntry>>;

struct PlanningRows {
    store: Arc<RuntimeStateStore>,
    baseline: PlanningEntries,
    staged: PlanningEntries,
}

impl PlanningRows {
    fn new(store: &Arc<RuntimeStateStore>) -> RuntimeStateResult<Self> {
        let mut rows = Self {
            store: Arc::clone(store),
            baseline: BTreeMap::new(),
            staged: BTreeMap::new(),
        };
        rows.load(checkpoint_key())?;
        checkpoint(rows.entry(&checkpoint_key())?)?;
        Ok(rows)
    }

    fn load(&mut self, key: PlanningKey) -> RuntimeStateResult<()> {
        if let std::collections::btree_map::Entry::Vacant(slot) = self.baseline.entry(key.clone()) {
            let entry = self.store.read_projection_entry(key.0, &key.1)?;
            slot.insert(entry.clone());
            self.staged.insert(key, entry);
        }
        Ok(())
    }

    fn entry(&self, key: &PlanningKey) -> RuntimeStateResult<Option<&ProjectionEntry>> {
        prepared_entry(&self.staged, key)
    }

    fn prepare_signal(&mut self, signal: &PlanningSignalRows) -> RuntimeStateResult<()> {
        self.load(signal.signal_key.clone())?;
        if let Some((key, _)) = &signal.quota {
            self.load(key.clone())?;
        }
        Ok(())
    }

    fn current(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<PlanningEntries> {
        if !transaction.belongs_to(&self.store.database) {
            return Err(planning_error("state_transaction_owner_mismatch"));
        }
        self.baseline
            .keys()
            .map(|key| {
                let entry = query_projection_entry(transaction.sql(), key.0, &key.1)?
                    .map(|row| {
                        self.store
                            .validate_projection_row(row, "apply_planning_projection")
                    })
                    .transpose()?;
                Ok((key.clone(), entry))
            })
            .collect()
    }

    fn write(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        entries: &PlanningEntries,
    ) -> RuntimeStateResult<()> {
        for entry in entries.values().flatten() {
            let written = self.store.write_projection_in_transaction(
                transaction.sql(),
                entry.namespace(),
                entry.entry_key(),
                entry.ledger_sequence(),
                entry.payload(),
                entry.payload_sha256(),
            )?;
            if written != *entry {
                return Err(planning_error(
                    "policy_planning_projection_position_mismatch",
                ));
            }
        }
        Ok(())
    }
}

struct PlanningSignalRows {
    data: PolicyPlanningSignalEventData,
    signal_key: PlanningKey,
    signal_payload: Vec<u8>,
    quota: Option<(PlanningKey, Vec<u8>)>,
}

impl PlanningSignalRows {
    fn prepare(data: &PolicyPlanningSignalEventData) -> RuntimeStateResult<Self> {
        let detection = matches!(
            data.kind,
            PolicyPlanningSignalKind::DetectionReserved
                | PolicyPlanningSignalKind::DetectionQuotaExhausted
        );
        if detection != data.detection_budget.is_some() {
            return Err(planning_error(if detection {
                "policy_detection_budget_missing"
            } else {
                "policy_detection_budget_unexpected"
            }));
        }
        let signal_key = signal_key(&data.signal_id);
        let signal_payload = serde_json::to_vec(&StoredPlanningSignal::from_data(data))
            .map_err(|_| planning_error("policy_planning_signal_projection_encode_failed"))?;
        validate_projection_input(signal_key.0, &signal_key.1, 1, &signal_payload)?;
        let quota = data
            .detection_budget
            .as_ref()
            .map(|budget| {
                let key = quota_key(&data.instance_id, &budget.window_id);
                let payload = serde_json::to_vec(&StoredDetectionQuota {
                    schema_version: QUOTA_SCHEMA.to_owned(),
                    instance_id: data.instance_id.clone(),
                    window_id: budget.window_id.clone(),
                    dispatch_used: budget.dispatch_used,
                    runtime_reserved_ms: budget.runtime_reserved_ms,
                })
                .map_err(|_| planning_error("policy_detection_quota_projection_encode_failed"))?;
                validate_projection_input(key.0, &key.1, 1, &payload)?;
                Ok::<_, RuntimeStateError>((key, payload))
            })
            .transpose()?;
        Ok(Self {
            data: data.clone(),
            signal_key,
            signal_payload,
            quota,
        })
    }

    fn prior_quota(
        &self,
        entries: &PlanningEntries,
    ) -> RuntimeStateResult<Option<(u64, PlanningQuotaUsage)>> {
        match (&self.quota, &self.data.detection_budget) {
            (Some((key, _)), Some(budget)) => decode_quota(
                prepared_entry(entries, key)?,
                &self.data.instance_id,
                &budget.window_id,
            ),
            _ => Ok(None),
        }
    }

    fn stage(
        &self,
        entries: &mut PlanningEntries,
        sequence: u64,
    ) -> RuntimeStateResult<PlanningSignalRecoveryStep> {
        let prior = decode_signal(
            prepared_entry(entries, &self.signal_key)?,
            &self.data.signal_id,
        )?;
        if prior
            .as_ref()
            .is_some_and(|(position, data)| *position != sequence || data != &self.data)
        {
            return Err(planning_error("policy_planning_signal_identity_conflict"));
        }
        let prior_quota = self.prior_quota(entries)?;
        let mut quota_already_projected = false;
        if let Some((position, usage)) = prior_quota {
            let budget = self
                .data
                .detection_budget
                .as_ref()
                .ok_or_else(|| planning_error("policy_detection_budget_missing"))?;
            if position > sequence
                || position == sequence
                    && (prior.is_none()
                        || usage.dispatch_used != budget.dispatch_used
                        || usage.runtime_reserved_ms != budget.runtime_reserved_ms)
            {
                return Err(planning_error("policy_detection_quota_projection_conflict"));
            }
            quota_already_projected = position == sequence;
        }
        if prior.is_none() {
            stage_entry(
                entries,
                &self.signal_key,
                sequence,
                self.signal_payload.clone(),
            )?;
        }
        if !quota_already_projected && let Some((key, payload)) = &self.quota {
            stage_entry(entries, key, sequence, payload.clone())?;
        }
        Ok(PlanningSignalRecoveryStep {
            data: self.data.clone(),
            prior_quota,
            quota_already_projected,
        })
    }
}

pub struct PlanningSignalRecoveryStep {
    data: PolicyPlanningSignalEventData,
    prior_quota: Option<(u64, PlanningQuotaUsage)>,
    quota_already_projected: bool,
}

impl PlanningSignalRecoveryStep {
    pub fn data(&self) -> &PolicyPlanningSignalEventData {
        &self.data
    }
    pub fn prior_quota(&self) -> Option<(u64, PlanningQuotaUsage)> {
        self.prior_quota
    }
    pub fn quota_already_projected(&self) -> bool {
        self.quota_already_projected
    }
}

pub struct PreparedPlanningProjection {
    rows: PlanningRows,
    signal: PlanningSignalRows,
    applied_sequence: AtomicU64,
}

impl PreparedPlanningProjection {
    /// The writer-assigned projection attempt position; this is not a commit outcome.
    pub fn attempted_sequence(&self) -> Option<u64> {
        let sequence = self.applied_sequence.load(Ordering::Acquire);
        (sequence != 0).then_some(sequence)
    }

    pub fn prior_quota(&self) -> RuntimeStateResult<Option<(u64, PlanningQuotaUsage)>> {
        self.signal.prior_quota(&self.rows.baseline)
    }

    fn expected(&self, sequence: u64) -> RuntimeStateResult<PlanningEntries> {
        let mut entries = self.rows.baseline.clone();
        self.signal.stage(&mut entries, sequence)?;
        stage_checkpoint(&mut entries, sequence)?;
        Ok(entries)
    }

    pub fn apply(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
        event: &PersistedEvent,
    ) -> RuntimeStateResult<()> {
        if self.rows.current(transaction)? != self.rows.baseline {
            return Err(planning_error("policy_planning_projection_changed"));
        }
        if planning_event_data(event)? != self.signal.data {
            return Err(planning_error("policy_planning_projection_fact_mismatch"));
        }
        actingcommand_ledger::verify_transaction_event(
            &self.rows.store.database,
            transaction,
            event,
        )
        .map_err(planning_ledger_error)?;
        let entries = self.expected(event.sequence())?;
        self.applied_sequence
            .store(event.sequence(), Ordering::Release);
        self.rows.write(transaction, &entries)
    }

    pub fn observe(
        &self,
        transaction: &RuntimeTransaction<'_, '_>,
    ) -> RuntimeStateResult<PlanningStateObservation> {
        let current = self.rows.current(transaction)?;
        let sequence = self.applied_sequence.load(Ordering::Acquire);
        Ok(if sequence != 0 && current == self.expected(sequence)? {
            PlanningStateObservation::Applied
        } else if current == self.rows.baseline {
            PlanningStateObservation::Unchanged
        } else {
            PlanningStateObservation::Unknown
        })
    }
}

pub struct PreparedPlanningRecovery {
    rows: PlanningRows,
    page: PlanningSignalRecoveryPage,
    steps: Vec<PlanningSignalRecoveryStep>,
}

impl PreparedPlanningRecovery {
    pub fn steps(&self) -> &[PlanningSignalRecoveryStep] {
        &self.steps
    }
    pub fn through_sequence(&self) -> u64 {
        self.page.through_sequence()
    }

    /// Commits the prepared historical interval without appending any new fact.
    pub fn commit(self) -> RuntimeStateResult<()> {
        let mut connection = self.rows.store.connection("recover_planning_projections")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| planning_sql_error("begin_planning_recovery", &error))?;
        let work = (|| {
            let scope = self.rows.store.database.borrow_transaction(&transaction);
            let current = self.rows.current(&scope)?;
            if current != self.rows.baseline {
                return Err(planning_error("policy_planning_projection_changed"));
            }
            actingcommand_ledger::verify_transaction_planning_page(
                &self.rows.store.database,
                &scope,
                &self.page,
                checkpoint(prepared_entry(&current, &checkpoint_key())?)?.unwrap_or(0),
            )
            .map_err(planning_ledger_error)?;
            self.rows.write(&scope, &self.rows.staged)
        })();
        if let Err(original) = work {
            return match transaction.rollback() {
                Ok(()) => Err(original),
                Err(rollback) => Err(planning_error("policy_planning_projection_rollback_failed")
                    .with_detail(format!("primary={original}; rollback={rollback}"))),
            };
        }
        transaction.commit().map_err(|error| {
            planning_sql_error("commit_planning_recovery", &error).with_detail(format!(
                "commit_outcome=unknown; through_sequence={}; primary={error}",
                self.page.through_sequence()
            ))
        })?;
        Ok(())
    }
}

impl RuntimeStateStore {
    pub fn load_planning_signal(
        &self,
        signal_id: &str,
    ) -> RuntimeStateResult<Option<(u64, PolicyPlanningSignalEventData)>> {
        let key = signal_key(signal_id);
        decode_signal(
            self.read_projection_entry(key.0, &key.1)?.as_ref(),
            signal_id,
        )
    }

    pub fn load_detection_quota(
        &self,
        instance_id: &str,
        window_id: &str,
    ) -> RuntimeStateResult<Option<(u64, PlanningQuotaUsage)>> {
        let key = quota_key(instance_id, window_id);
        decode_quota(
            self.read_projection_entry(key.0, &key.1)?.as_ref(),
            instance_id,
            window_id,
        )
    }

    pub fn load_planning_checkpoint(&self) -> RuntimeStateResult<Option<u64>> {
        checkpoint(
            self.read_projection_entry(PLANNING_SIGNAL_PROJECTION_NAMESPACE, CHECKPOINT_KEY)?
                .as_ref(),
        )
    }

    pub fn prepare_planning_projection(
        self: &Arc<Self>,
        data: &PolicyPlanningSignalEventData,
    ) -> RuntimeStateResult<PreparedPlanningProjection> {
        let signal = PlanningSignalRows::prepare(data)?;
        let mut rows = PlanningRows::new(self)?;
        rows.prepare_signal(&signal)?;
        Ok(PreparedPlanningProjection {
            rows,
            signal,
            applied_sequence: AtomicU64::new(0),
        })
    }

    pub fn prepare_planning_recovery(
        self: &Arc<Self>,
        page: PlanningSignalRecoveryPage,
    ) -> RuntimeStateResult<PreparedPlanningRecovery> {
        let mut rows = PlanningRows::new(self)?;
        if checkpoint(rows.entry(&checkpoint_key())?)?.unwrap_or(0) != page.after_sequence() {
            return Err(planning_error("policy_planning_projection_changed"));
        }
        let mut steps = Vec::new();
        for event in page.planning_events() {
            let signal = PlanningSignalRows::prepare(&planning_event_data(event)?)?;
            rows.prepare_signal(&signal)?;
            steps.push(signal.stage(&mut rows.staged, event.sequence())?);
        }
        stage_checkpoint(&mut rows.staged, page.through_sequence())?;
        Ok(PreparedPlanningRecovery { rows, page, steps })
    }
}

fn prepared_entry<'a>(
    entries: &'a PlanningEntries,
    key: &PlanningKey,
) -> RuntimeStateResult<Option<&'a ProjectionEntry>> {
    entries
        .get(key)
        .map(Option::as_ref)
        .ok_or_else(|| planning_error("policy_planning_projection_unprepared"))
}

fn stage_entry(
    entries: &mut PlanningEntries,
    key: &PlanningKey,
    sequence: u64,
    payload: Vec<u8>,
) -> RuntimeStateResult<()> {
    validate_projection_input(key.0, &key.1, sequence, &payload)?;
    sqlite_integer(
        sequence,
        "projection_sequence_overflow",
        "prepare_planning_projection",
    )?;
    prepared_entry(entries, key)?;
    entries.insert(
        key.clone(),
        Some(ProjectionEntry {
            namespace: key.0.to_owned(),
            entry_key: key.1.clone(),
            ledger_sequence: sequence,
            payload_sha256: sha256(&payload),
            payload,
        }),
    );
    Ok(())
}

fn stage_checkpoint(entries: &mut PlanningEntries, sequence: u64) -> RuntimeStateResult<()> {
    let key = checkpoint_key();
    if let Some(prior) = checkpoint(prepared_entry(entries, &key)?)? {
        if prior > sequence {
            return Err(planning_error("policy_planning_checkpoint_ahead"));
        }
        if prior == sequence {
            return Ok(());
        }
    }
    let payload = serde_json::to_vec(&PlanningSignalCheckpoint {
        schema_version: SIGNAL_SCHEMA.to_owned(),
        through_sequence: sequence,
    })
    .map_err(|_| planning_error("policy_planning_checkpoint_encode_failed"))?;
    stage_entry(entries, &key, sequence, payload)
}

fn decode_signal(
    entry: Option<&ProjectionEntry>,
    signal_id: &str,
) -> RuntimeStateResult<Option<(u64, PolicyPlanningSignalEventData)>> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let stored: StoredPlanningSignal = serde_json::from_slice(entry.payload())
        .map_err(|_| planning_error("policy_planning_signal_projection_invalid"))?;
    if stored.schema_version != SIGNAL_SCHEMA || stored.signal_id != signal_id {
        return Err(planning_error("policy_planning_signal_projection_invalid"));
    }
    Ok(Some((entry.ledger_sequence(), stored.into_data())))
}

fn decode_quota(
    entry: Option<&ProjectionEntry>,
    instance_id: &str,
    window_id: &str,
) -> RuntimeStateResult<Option<(u64, PlanningQuotaUsage)>> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let stored: StoredDetectionQuota = serde_json::from_slice(entry.payload())
        .map_err(|_| planning_error("policy_detection_quota_projection_invalid"))?;
    if stored.schema_version != QUOTA_SCHEMA
        || stored.instance_id != instance_id
        || stored.window_id != window_id
    {
        return Err(planning_error("policy_detection_quota_projection_invalid"));
    }
    Ok(Some((
        entry.ledger_sequence(),
        PlanningQuotaUsage {
            dispatch_used: stored.dispatch_used,
            runtime_reserved_ms: stored.runtime_reserved_ms,
        },
    )))
}

fn checkpoint(entry: Option<&ProjectionEntry>) -> RuntimeStateResult<Option<u64>> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let stored: PlanningSignalCheckpoint = serde_json::from_slice(entry.payload())
        .map_err(|_| planning_error("policy_planning_checkpoint_invalid"))?;
    if stored.schema_version != SIGNAL_SCHEMA || stored.through_sequence != entry.ledger_sequence()
    {
        return Err(planning_error("policy_planning_checkpoint_invalid"));
    }
    Ok(Some(stored.through_sequence))
}

fn planning_event_data(
    event: &PersistedEvent,
) -> RuntimeStateResult<PolicyPlanningSignalEventData> {
    if event.event_type() != EventType::PolicyPlanningSignalObserved
        || event.origin().source() != EventSource::Scheduler
        || event.origin().actor() != EventActor::Scheduler
        || event.origin().module() != OriginModule::Policy
    {
        return Err(planning_error("policy_planning_projection_origin_invalid"));
    }
    let EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload)) = event.payload()
    else {
        return Err(planning_error("policy_recovery_query_mismatch"));
    };
    Ok(PolicyPlanningSignalEventData {
        signal_id: payload.signal_id().to_owned(),
        instance_id: payload.instance_id().to_owned(),
        task_id: payload.task_id().map(str::to_owned),
        kind: payload.kind(),
        fact_code: payload.fact_code().to_owned(),
        observed_at_unix_ms: payload.observed_at_unix_ms(),
        detection_budget: payload.detection_budget().cloned(),
    })
}

fn signal_key(signal_id: &str) -> PlanningKey {
    (
        PLANNING_SIGNAL_PROJECTION_NAMESPACE,
        format!("{:x}", Sha256::digest(signal_id.as_bytes())),
    )
}

fn quota_key(instance_id: &str, window_id: &str) -> PlanningKey {
    (
        DETECTION_QUOTA_PROJECTION_NAMESPACE,
        format!(
            "{:x}",
            Sha256::digest(format!("{instance_id}\0{window_id}").as_bytes())
        ),
    )
}

fn checkpoint_key() -> PlanningKey {
    (
        PLANNING_SIGNAL_PROJECTION_NAMESPACE,
        CHECKPOINT_KEY.to_owned(),
    )
}

fn planning_error(code: &'static str) -> RuntimeStateError {
    fatal(code, "persist_planning_projection")
}
fn planning_ledger_error(error: actingcommand_ledger::GlobalLedgerError) -> RuntimeStateError {
    planning_error(error.code()).with_detail(format!("{error}; detail={:?}", error.detail()))
}
fn planning_sql_error(operation: &'static str, error: &rusqlite::Error) -> RuntimeStateError {
    fatal("policy_planning_projection_transaction_failed", operation).with_detail(error.to_string())
}
