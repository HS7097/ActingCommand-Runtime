// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-1, the v0.12.0 shape slice (register `contracts/v012-register.md` §1.1 and
// §1.2): the ledger observer's persisted shapes and the value records of the decision memory,
// the activity state, the wake obligations and the fact-refresh transitions. Types, readers and
// validation only; the slices the register names in its "First writer" column write them.

use super::{PayloadDetail, validate_policy_admission, validate_policy_token};
use crate::codes::SanitizationCode;
use crate::outcome::{Category, CodeStr, Outcome};
use crate::{
    AgentWakeData, DiagnosticCode, EffectDisposition, EventAction, EventId, OwnerEpoch,
    PolicyAdmissionRecord, PolicyFailureClass, PolicyFailureDisposition, RuntimeStateStoreRevision,
    SanitizationError, SanitizedAudit,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// The most day entries a decision-memory record holds (register L7).
pub const MAX_POLICY_CONTROL_STATE_DAYS: usize = 32;

/// A revisioned source whose changes the ledger records (ledger observer §8.3; vocabulary
/// `ledger_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerSource {
    InstanceFacts,
    RuntimeFacts,
    DecisionMemory,
    FactRefreshTransitions,
}

/// One change of a revisioned source: the source, its lineage (a fresh identity per owner
/// epoch) and its revision, +1 per accepted change and contiguous within a lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRevision {
    pub source: LedgerSource,
    pub lineage: OwnerEpoch,
    pub revision: u64,
}

impl SourceRevision {
    /// A change carries a revision from 1 on.
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.revision == 0 {
            return Err(invalid_ledger_record("source"));
        }
        Ok(())
    }
}

/// The capture of a stored record (register L4): the optional envelope member every stored
/// record may carry from AO-2 on. `timestamp_unix_ms` of the record is the capture time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordCapture {
    pub capture_seq: u64,
    pub captured_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRevision>,
}

impl RecordCapture {
    /// The capture of a record whose timestamp is `timestamp_unix_ms`: the record's time is
    /// its capture time, and a revisioned source names a change.
    pub fn validate(&self, timestamp_unix_ms: u64) -> Result<(), SanitizationError> {
        if self.captured_at_unix_ms == 0 || self.captured_at_unix_ms != timestamp_unix_ms {
            return Err(invalid_ledger_record("capture"));
        }
        if let Some(source) = &self.source {
            source.validate()?;
        }
        Ok(())
    }
}

/// What a gap record covers (ledger observer §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerGapKind {
    /// Import: no trailer, the last line truncated (`ledger_dump_incomplete`, warning).
    DumpIncomplete,
    /// Import: a middle line's hash differs (`ledger_dump_invalid`, error).
    DumpInvalid,
    /// Start: the previous epoch has neither a close record nor a complete dump
    /// (`ledger_tail_unconfirmed`, warning).
    TailUnconfirmed,
    /// Flush: one record failed assembly or sanitizing (`ledger_record_rejected`, error).
    RecordRejected,
}

impl LedgerGapKind {
    /// The category of the gap's code (ledger observer §10, §16).
    pub const fn category(self) -> Category {
        match self {
            Self::DumpIncomplete | Self::TailUnconfirmed => Category::Warning,
            Self::DumpInvalid | Self::RecordRejected => Category::Error,
        }
    }
}

/// `ledger.gap_recorded` (register L1): records the ledger knows it does not hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerGapPayload {
    action: EventAction,
    gap_kind: LedgerGapKind,
    gap_start_unix_ms: u64,
    gap_end_unix_ms: u64,
    /// Absent: the count is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event_id: Option<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<SourceRevision>,
    outcome: Outcome,
    audit: SanitizedAudit,
}

impl LedgerGapPayload {
    pub const fn gap_kind(&self) -> LedgerGapKind {
        self.gap_kind
    }

    pub const fn gap_start_unix_ms(&self) -> u64 {
        self.gap_start_unix_ms
    }

    pub const fn gap_end_unix_ms(&self) -> u64 {
        self.gap_end_unix_ms
    }

    pub const fn record_count(&self) -> Option<u64> {
        self.record_count
    }

    pub const fn event_id(&self) -> Option<&EventId> {
        self.event_id.as_ref()
    }

    pub const fn source(&self) -> Option<&SourceRevision> {
        self.source.as_ref()
    }

    /// Workflow #383 AO-1: the gap of a damaged dump (`ledger-observer.md` §9.4): `range` is
    /// `(gap_start_unix_ms, gap_end_unix_ms)`, the count unknown and no record named.
    pub(super) fn dump(
        gap_kind: LedgerGapKind,
        range: (u64, u64),
        outcome: Outcome,
        audit: SanitizedAudit,
    ) -> Result<Self, SanitizationError> {
        let payload = Self {
            action: EventAction::LedgerRecovery,
            gap_kind,
            gap_start_unix_ms: range.0,
            gap_end_unix_ms: range.1,
            record_count: None,
            event_id: None,
            source: None,
            outcome,
            audit,
        };
        payload.validate()?;
        Ok(payload)
    }

    /// Workflow #383 AO-2 (`ledger-observer.md` §10): the gap of one record the flush thread
    /// refused: its capture time, its event id and, for a revisioned source, the source.
    pub(super) fn rejected(
        captured_at_unix_ms: u64,
        event_id: EventId,
        source: Option<SourceRevision>,
        outcome: Outcome,
        audit: SanitizedAudit,
    ) -> Result<Self, SanitizationError> {
        let payload = Self {
            action: EventAction::LedgerRecovery,
            gap_kind: LedgerGapKind::RecordRejected,
            gap_start_unix_ms: captured_at_unix_ms,
            gap_end_unix_ms: captured_at_unix_ms,
            record_count: Some(1),
            event_id: Some(event_id),
            source,
            outcome,
            audit,
        };
        payload.validate()?;
        Ok(payload)
    }

    /// A gap's time range is ordered; a rejected record is one record at one capture time,
    /// with its event id; every other gap's count is unknown and names no record.
    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::LedgerRecovery
            || self.gap_start_unix_ms == 0
            || self.gap_start_unix_ms > self.gap_end_unix_ms
            || self.outcome.category() != self.gap_kind.category()
        {
            return Err(invalid_ledger_record("ledger_gap"));
        }
        let shaped = match self.gap_kind {
            LedgerGapKind::RecordRejected => {
                self.record_count == Some(1)
                    && self.event_id.is_some()
                    && self.gap_start_unix_ms == self.gap_end_unix_ms
            }
            LedgerGapKind::DumpIncomplete
            | LedgerGapKind::DumpInvalid
            | LedgerGapKind::TailUnconfirmed => {
                self.record_count.is_none() && self.event_id.is_none() && self.source.is_none()
            }
        };
        if !shaped {
            return Err(invalid_ledger_record("ledger_gap"));
        }
        if let Some(source) = &self.source {
            source.validate()?;
        }
        Ok(())
    }
}

/// The edge of a reported stretch of a condition (vocabulary `segment_edge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerSegmentEdge {
    Started,
    Ended,
}

/// `ledger.health_observed` (register L2): one probe finding; the finding's values travel in
/// `outcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerHealthPayload {
    action: EventAction,
    outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    segment_edge: Option<LedgerSegmentEdge>,
    audit: SanitizedAudit,
}

impl LedgerHealthPayload {
    /// Workflow #383 AO-2 (`ledger-observer.md` §14.1): one finding of the ledger itself.
    pub(super) fn new(
        outcome: Outcome,
        segment_edge: Option<LedgerSegmentEdge>,
        audit: SanitizedAudit,
    ) -> Result<Self, SanitizationError> {
        let payload = Self {
            action: EventAction::LedgerProbe,
            outcome,
            segment_edge,
            audit,
        };
        payload.validate()?;
        Ok(payload)
    }

    pub const fn segment_edge(&self) -> Option<LedgerSegmentEdge> {
        self.segment_edge
    }

    /// A finding is a warning or an error (ledger observer §14.1); a probe without one writes
    /// nothing.
    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::LedgerProbe
            || !matches!(self.outcome.category(), Category::Warning | Category::Error)
        {
            return Err(invalid_ledger_record("ledger_health"));
        }
        Ok(())
    }
}

/// `ledger.dump_imported` (register L3): a dump was imported (`ledger_dump_imported`, info).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerDumpImportedPayload {
    action: EventAction,
    outcome: Outcome,
    audit: SanitizedAudit,
}

impl LedgerDumpImportedPayload {
    /// Workflow #383 AO-1: the ledger's own record of a dump it imported (`ledger-observer.md`
    /// §9.4 step 5).
    pub(super) fn new(outcome: Outcome, audit: SanitizedAudit) -> Result<Self, SanitizationError> {
        let payload = Self {
            action: EventAction::LedgerRecovery,
            outcome,
            audit,
        };
        payload.validate()?;
        Ok(payload)
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::LedgerRecovery || self.outcome.category() != Category::Info {
            return Err(invalid_ledger_record("dump_imported"));
        }
        Ok(())
    }
}

/// One store of `runtime.lifecycle_observed` phase `tagged_state_loaded` (register L5): the
/// base record of the store's new lineage. Both previous members are absent on a new state
/// root; `previous_revision` is the highest committed revision of that source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaggedStateStore {
    pub source: LedgerSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_lineage: Option<OwnerEpoch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_revision: Option<u64>,
    pub lineage: OwnerEpoch,
    pub loaded_count: u64,
}

/// The stores of one `tagged_state_loaded` record: one entry per store, each with its previous
/// lineage and revision together or neither, and a lineage that is new.
pub(super) fn validate_tagged_state_stores(
    stores: &[TaggedStateStore],
) -> Result<(), SanitizationError> {
    let mut sources = BTreeSet::new();
    if stores.is_empty() {
        return Err(invalid_ledger_record("tagged_state_loaded"));
    }
    for store in stores {
        if !sources.insert(store.source)
            || store.previous_lineage.is_some() != store.previous_revision.is_some()
            || store.previous_lineage == Some(store.lineage)
        {
            return Err(invalid_ledger_record("tagged_state_loaded"));
        }
    }
    Ok(())
}

/// The report a `tagged_state_loaded` or `tagged_state_enabled` record may carry (register
/// X4): the load (`ledger_tagged_state_loaded`) or the seeding (`ledger_tagged_state_seeded`),
/// both info, as every code ledger observer §16 reports for these phases.
pub(super) fn validate_tagged_state_outcome(
    outcome: Option<&Outcome>,
) -> Result<(), SanitizationError> {
    if outcome.is_some_and(|outcome| outcome.category() != Category::Info) {
        return Err(invalid_ledger_record("tagged_state_outcome"));
    }
    Ok(())
}

/// `input_fact_revisions` of `policy.dispatch_intent` and `task.selection_evaluated`
/// (register L9): both stores' (lineage, revision) the decision read, each the same pair a
/// status read's source names (`RuntimeStateStoreRevision`, register W1). A reader accepts it or
/// `input_ledger_position`, which stays non-zero when present (order-v012 §3.5 item 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputFactRevisions {
    pub instance_facts: RuntimeStateStoreRevision,
    pub runtime_facts: RuntimeStateStoreRevision,
}

/// `input_ledger_position` as a reader takes it (register L9): absent reads as 0, and a
/// present position is never 0.
pub(super) fn deserialize_input_ledger_position<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let position = u64::deserialize(deserializer)?;
    if position == 0 {
        return Err(serde::de::Error::custom(
            "a present input_ledger_position is not 0",
        ));
    }
    Ok(position)
}

/// `input_ledger_position` is written only when the decision has one (register L9).
pub(super) const fn input_ledger_position_absent(position: &u64) -> bool {
    *position == 0
}

/// One day's count of a dispatch pair (register L7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDailyCount {
    pub local_day: i64,
    pub count: u32,
}

/// A dispatch pair's failure streak (register L7), split as the execution record's failure is
/// (§1.15 Q1): the failure code, the #336 failure identity and the class make the same error.
/// `paused_on` is rebuilt from `failure_identity` and never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFailureStreakRecord {
    pub failure_code: CodeStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_identity: Option<String>,
    pub class: PolicyFailureClass,
    pub count: u16,
    pub escalation_streak: u16,
    pub disposition: PolicyFailureDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at_unix_ms: Option<u64>,
    pub decision_id: String,
}

/// The normal admission that opened a dispatch pair's retry round (register L7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRetryRoundRecord {
    pub admission: PolicyAdmissionRecord,
    pub until_unix_ms: u64,
    pub expected_duration_ms: u64,
}

/// A dispatch pair's dispatch clock (register L7): when its last dispatch was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDispatchClockRecord {
    pub admitted_at_unix_ms: u64,
}

/// The rerun a dispatch pair is owed after condition terminations (register L7; Workflow #386
/// §4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConditionReruns {
    pub failure_code: CodeStr,
    pub count: u32,
    pub expires_at_unix_ms: u64,
}

/// `policy.control_state_recorded` (register L7): one dispatch pair's complete decision memory.
/// `task_id` is a catalog task or the reserved identity of a fact-refresh recognition package.
/// `active_window` is the `window_id` of the task's activity profile that `window_counts` and
/// `window_runtime_ms` count in; `window_runtime_ms` is the runtime the task has reserved or used
/// in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyControlStateRecord {
    pub instance_id: String,
    pub task_id: String,
    pub daily_counts: Vec<PolicyDailyCount>,
    pub window_counts: u32,
    pub window_runtime_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_streak: Option<PolicyFailureStreakRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_round: Option<PolicyRetryRoundRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_clock: Option<PolicyDispatchClockRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_reruns: Option<PolicyConditionReruns>,
}

impl PolicyControlStateRecord {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        crate::validate_instance_alias(&self.instance_id)
            .map_err(|_| invalid_policy_record("instance_id"))?;
        validate_policy_token(&self.task_id, "task_id")?;
        let mut days = BTreeSet::new();
        if self.daily_counts.len() > MAX_POLICY_CONTROL_STATE_DAYS
            || self
                .daily_counts
                .iter()
                .any(|day| !days.insert(day.local_day))
        {
            return Err(invalid_policy_record("daily_counts"));
        }
        match &self.active_window {
            Some(window) => validate_policy_token(window, "active_window")?,
            None if self.window_counts != 0 || self.window_runtime_ms != 0 => {
                return Err(invalid_policy_record("active_window"));
            }
            None => {}
        }
        if let Some(streak) = &self.failure_streak {
            validate_policy_token(streak.failure_code.as_str(), "failure_code")?;
            if let Some(identity) = &streak.failure_identity {
                validate_policy_token(identity, "failure_identity")?;
            }
            validate_policy_token(&streak.decision_id, "decision_id")?;
            if streak.count == 0 || streak.retry_at_unix_ms == Some(0) {
                return Err(invalid_policy_record("failure_streak"));
            }
        }
        if let Some(round) = &self.retry_round {
            validate_policy_admission(&round.admission)?;
            if round.until_unix_ms == 0 {
                return Err(invalid_policy_record("retry_round"));
            }
        }
        if self
            .dispatch_clock
            .is_some_and(|clock| clock.admitted_at_unix_ms == 0)
        {
            return Err(invalid_policy_record("dispatch_clock"));
        }
        if let Some(reruns) = &self.condition_reruns {
            validate_policy_token(reruns.failure_code.as_str(), "failure_code")?;
            if reruns.count == 0 || reruns.expires_at_unix_ms == 0 {
                return Err(invalid_policy_record("condition_reruns"));
            }
        }
        Ok(())
    }
}

/// `policy.activity_state_recorded` (register L14): one (instance, activity profile)'s complete
/// activity state, today's `activity_daily`, `activity_window`, `activity_runtime` and
/// `next_activity_eligible`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyActivityStateRecord {
    pub instance_id: String,
    pub profile_id: String,
    pub local_day: i64,
    pub daily_count: u32,
    pub window_id: String,
    pub window_count: u32,
    pub window_runtime_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_eligible_unix_ms: Option<u64>,
}

impl PolicyActivityStateRecord {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        crate::validate_instance_alias(&self.instance_id)
            .map_err(|_| invalid_policy_record("instance_id"))?;
        validate_policy_token(&self.profile_id, "profile_id")?;
        validate_policy_token(&self.window_id, "window_id")?;
        if self.next_eligible_unix_ms == Some(0) {
            return Err(invalid_policy_record("next_eligible_unix_ms"));
        }
        Ok(())
    }
}

/// The payload of `policy.control_state_recorded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyControlStatePayload {
    action: EventAction,
    state: Box<PolicyControlStateRecord>,
    audit: SanitizedAudit,
}

impl PolicyControlStatePayload {
    pub fn state(&self) -> &PolicyControlStateRecord {
        &self.state
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::PolicyControlState {
            return Err(invalid_policy_record("action"));
        }
        self.state.validate()
    }
}

/// The payload of `policy.activity_state_recorded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyActivityStatePayload {
    action: EventAction,
    state: Box<PolicyActivityStateRecord>,
    audit: SanitizedAudit,
}

impl PolicyActivityStatePayload {
    pub fn state(&self) -> &PolicyActivityStateRecord {
        &self.state
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::PolicyControlState {
            return Err(invalid_policy_record("action"));
        }
        self.state.validate()
    }
}

/// `policy.fact_refresh_transition` (register D1): one transition of the fact-refresh recorder;
/// `outcome` is the transition's code. The envelope's `capture.source` is
/// `fact_refresh_transitions` with the owner epoch as lineage and the transition number as
/// revision; the cycle number takes no part in that check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFactRefreshTransitionPayload {
    action: EventAction,
    cycle_seq: u64,
    instance_id: String,
    fact_key: String,
    outcome: Outcome,
    audit: SanitizedAudit,
}

impl PolicyFactRefreshTransitionPayload {
    pub const fn cycle_seq(&self) -> u64 {
        self.cycle_seq
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn fact_key(&self) -> &str {
        &self.fact_key
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::PolicyFactRefresh {
            return Err(invalid_policy_record("action"));
        }
        crate::validate_instance_alias(&self.instance_id)
            .map_err(|_| invalid_policy_record("instance_id"))?;
        validate_policy_token(&self.fact_key, "fact_key")
    }
}

/// `agent.wake_obligation_recorded` (register L8): one wake obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeObligationPayload {
    action: EventAction,
    wake: AgentWakeData,
    audit: SanitizedAudit,
}

impl AgentWakeObligationPayload {
    pub const fn wake(&self) -> &AgentWakeData {
        &self.wake
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::AgentWake {
            return Err(SanitizationError::new(
                SanitizationCode::InvalidAgentWake,
                "agent_wake_obligation",
            ));
        }
        self.wake.validate()
    }
}

/// The common detail of these payloads; a payload that names its `outcome` member reports it.
macro_rules! observer_detail {
    ($type:ty $(, $outcome:ident)?) => {
        impl PayloadDetail for $type {
            fn action(&self) -> EventAction {
                self.action
            }

            fn diagnostic_code(&self) -> Option<DiagnosticCode> {
                None
            }

            fn effect_disposition(&self) -> Option<EffectDisposition> {
                None
            }

            fn audit(&self) -> &SanitizedAudit {
                &self.audit
            }

            $(
                fn outcome(&self) -> Option<&Outcome> {
                    Some(&self.$outcome)
                }
            )?
        }
    };
}

observer_detail!(LedgerGapPayload, outcome);
observer_detail!(LedgerHealthPayload, outcome);
observer_detail!(LedgerDumpImportedPayload, outcome);
observer_detail!(PolicyControlStatePayload);
observer_detail!(PolicyActivityStatePayload);
observer_detail!(PolicyFactRefreshTransitionPayload, outcome);
observer_detail!(AgentWakeObligationPayload);

fn invalid_ledger_record(field: &'static str) -> SanitizationError {
    SanitizationError::new(SanitizationCode::InvalidSanitizedPayload, field)
}

fn invalid_policy_record(field: &'static str) -> SanitizationError {
    SanitizationError::new(SanitizationCode::InvalidPolicyPayload, field)
}
