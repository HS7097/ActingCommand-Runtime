// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-2 (`contracts/ledger-observer.md` §11.1, §11.2, §11.4): the rows of the index
//! table `ledger_index` that a batch of records changes. The rows are written in the batch's own
//! SQLite transaction, so the index always matches the committed ledger. This module only says
//! which rows a record touches; the store writes them.
//!
//! - `state`: one row per tagged key, overwritten by the key's latest record;
//! - `open`: an intent of a dispatch, lease transition, task terminal, input or application, added
//!   when the intent commits and deleted when its result (same action id) commits;
//! - `request`: the latest record of a write request, a ring of `request_index_records` rows;
//! - `lifecycle`: `last_start`, `last_close` and `last_fatal`.

use crate::PersistedEvent;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    AgentPayload, EventPayload, EventType, FactPayload, FactScope, PolicyPayload, RuntimeFactScope,
    RuntimeLifecyclePhase, RuntimePayload,
};
use serde::Serialize;

/// What the index keeps: the tagged workbench keys and the size of the request ring (§15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IndexPolicy {
    workbench_patterns: Vec<String>,
    request_index_records: u64,
}

impl IndexPolicy {
    pub(super) fn new(workbench_patterns: Vec<String>, request_index_records: u64) -> Self {
        Self {
            workbench_patterns,
            request_index_records,
        }
    }

    /// `tag.workbench_patterns`: a key matches a pattern whose every `*` stands for any text.
    fn tags_workbench_key(&self, key: &str) -> bool {
        self.workbench_patterns
            .iter()
            .any(|pattern| pattern_matches(pattern, key))
    }
}

impl Default for IndexPolicy {
    fn default() -> Self {
        let policy = super::LedgerCapturePolicy::default();
        Self::new(policy.workbench_patterns, policy.request_index_records)
    }
}

/// A `lifecycle` row's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LifecycleSlot {
    Start,
    Close,
    Fatal,
}

impl LifecycleSlot {
    pub(super) const fn index_key(self) -> &'static str {
        match self {
            Self::Start => "last_start",
            Self::Close => "last_close",
            Self::Fatal => "last_fatal",
        }
    }
}

/// One row change of `ledger_index`; `at` is the record's position in the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IndexChange {
    /// `state`: the tagged key's latest record is the batch's record `at`.
    State { key: String, at: usize },
    /// `open`: the batch's record `at` is an intent; its event id is the row's key.
    Open { at: usize },
    /// The batch's record `at` is an intent's result: the `open` rows of the intents with its
    /// action id are deleted.
    Close { at: usize },
    /// `request`: the batch's record `at` is the latest record of its write request.
    Request { at: usize },
    /// `lifecycle`: the batch's record `at` is the latest record of its slot.
    Lifecycle { slot: LifecycleSlot, at: usize },
}

/// The index rows one batch changes, and the size of the request ring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct IndexRows {
    changes: Vec<IndexChange>,
    request_ring: u64,
}

impl IndexRows {
    /// The rows `events` change under `policy`.
    pub(super) fn of(events: &[PersistedEvent], policy: &IndexPolicy) -> Self {
        Self {
            changes: changes(events, policy),
            request_ring: policy.request_index_records,
        }
    }

    pub(super) fn changes(&self) -> &[IndexChange] {
        &self.changes
    }

    pub(super) const fn request_ring(&self) -> u64 {
        self.request_ring
    }
}

/// The row changes of a batch, in record order.
fn changes(events: &[PersistedEvent], policy: &IndexPolicy) -> Vec<IndexChange> {
    let mut changes = Vec::new();
    for (at, event) in events.iter().enumerate() {
        for key in tagged_keys(event, policy) {
            changes.push(IndexChange::State { key, at });
        }
        match intent_role(event.event_type()) {
            Some(IntentRole::Intent) => changes.push(IndexChange::Open { at }),
            Some(IntentRole::Result) if event.links().action_id().is_some() => {
                changes.push(IndexChange::Close { at });
            }
            Some(IntentRole::Result) | None => {}
        }
        if is_request_record(event.event_type()) && event.links().request_id().is_some() {
            changes.push(IndexChange::Request { at });
        }
        if let Some(slot) = lifecycle_slot(event) {
            changes.push(IndexChange::Lifecycle { slot, at });
        }
    }
    changes
}

/// Whether the record carries a tagged key: a tagged record commits on arrival, with FULL sync
/// (§8.1, rule 13).
pub(super) fn is_tagged(event: &PersistedEvent, policy: &IndexPolicy) -> bool {
    !tagged_keys(event, policy).is_empty()
}

/// The tagged keys of one record (§11.1, §11.2): every key of the instance store, the instance's
/// input boundaries, the workbench keys the patterns name, the decision memory, the activity
/// state and the wake obligations.
fn tagged_keys(event: &PersistedEvent, policy: &IndexPolicy) -> Vec<String> {
    match event.payload() {
        EventPayload::Fact(FactPayload::Published(published)) => published
            .records()
            .map(|record| {
                format!(
                    "instance_facts/{}/{}",
                    fact_scope(&record.scope),
                    record.key
                )
            })
            .collect(),
        EventPayload::Fact(FactPayload::Invalidated(invalidated)) => {
            let invalidation = invalidated.invalidation();
            vec![format!(
                "instance_facts/{}/{}",
                fact_scope(&invalidation.scope),
                invalidation.key
            )]
        }
        EventPayload::Runtime(RuntimePayload::FactRecorded(recorded)) => {
            let record = recorded.record();
            workbench_key(&record.scope, &record.key, policy)
                .into_iter()
                .collect()
        }
        EventPayload::Runtime(RuntimePayload::FactInvalidated(invalidated)) => {
            let invalidation = invalidated.invalidation();
            workbench_key(&invalidation.scope, &invalidation.key, policy)
                .into_iter()
                .collect()
        }
        EventPayload::Policy(PolicyPayload::ControlStateRecorded(recorded)) => {
            let state = recorded.state();
            vec![format!(
                "decision_memory/{}/{}",
                state.instance_id, state.task_id
            )]
        }
        EventPayload::Policy(PolicyPayload::ActivityStateRecorded(recorded)) => {
            let state = recorded.state();
            vec![format!(
                "activity_memory/{}/{}",
                state.instance_id, state.profile_id
            )]
        }
        EventPayload::Agent(AgentPayload::WakeObligationRecorded(recorded)) => {
            id_text(&recorded.wake().wake_id())
                .map(|wake| format!("wake/{wake}"))
                .into_iter()
                .collect()
        }
        _ => input_boundary_key(event).into_iter().collect(),
    }
}

/// `input_boundary/<instance>/committed` and `/failed`: the instance's latest input result of
/// each kind (§11.2).
fn input_boundary_key(event: &PersistedEvent) -> Option<String> {
    let boundary = match event.event_type() {
        EventType::InputCommitted => "committed",
        EventType::InputFailed => "failed",
        _ => return None,
    };
    let instance = id_text(event.links().instance_id()?)?;
    Some(format!("input_boundary/{instance}/{boundary}"))
}

fn workbench_key(scope: &RuntimeFactScope, key: &str, policy: &IndexPolicy) -> Option<String> {
    if !policy.tags_workbench_key(key) {
        return None;
    }
    let scope = match scope {
        RuntimeFactScope::Runtime => "runtime".to_owned(),
        RuntimeFactScope::Instance { instance_id } => format!("instance:{}", id_text(instance_id)?),
    };
    Some(format!("runtime_facts/{scope}/{key}"))
}

fn fact_scope(scope: &FactScope) -> String {
    match scope {
        FactScope::Instance { instance_id } => format!("instance:{instance_id}"),
        FactScope::Server { server_id } => format!("server:{server_id}"),
        FactScope::Game { game_id } => format!("game:{game_id}"),
    }
}

/// The canonical text of an identifier (its serialization).
pub(super) fn id_text<T: Serialize>(value: &T) -> Option<String> {
    match serde_json::to_value(value).ok()? {
        serde_json::Value::String(text) => Some(text),
        _ => None,
    }
}

enum IntentRole {
    Intent,
    Result,
}

/// The intents X1 reads (dispatch, lease, task, input, application; §11.6) and their results.
fn intent_role(event_type: EventType) -> Option<IntentRole> {
    match event_type {
        EventType::PolicyDispatchIntent
        | EventType::LeaseTransitionIntent
        | EventType::TaskTerminalIntent
        | EventType::InputIntent
        | EventType::ApplicationIntent => Some(IntentRole::Intent),
        EventType::PolicyDispatchAdmitted
        | EventType::PolicyDispatchRejected
        | EventType::LeaseGranted
        | EventType::LeaseTransferred
        | EventType::LeaseRenewed
        | EventType::LeaseReleased
        | EventType::LeaseExpired
        | EventType::LeaseTransitionFailed
        | EventType::TaskCompleted
        | EventType::TaskFailed
        | EventType::TaskCancelled
        | EventType::TaskTerminalCommitFailed
        | EventType::InputCommitted
        | EventType::InputFailed
        | EventType::ApplicationCompleted
        | EventType::ApplicationFailed => Some(IntentRole::Result),
        _ => None,
    }
}

/// The records of a write request at the IPC boundary (X3).
fn is_request_record(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::CommandReceived | EventType::CommandValidated | EventType::CommandRejected
    )
}

/// `last_start`: `runtime.started` or `runtime.takeover`; `last_close`: the shutdown request
/// (the close record until AO-3's G16 writes its own); `last_fatal`: a fatal `runtime.failed`.
fn lifecycle_slot(event: &PersistedEvent) -> Option<LifecycleSlot> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::Started(_) | RuntimePayload::Takeover(_)) => {
            Some(LifecycleSlot::Start)
        }
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(lifecycle))
            if matches!(lifecycle.phase(), RuntimeLifecyclePhase::ShutdownRequested) =>
        {
            Some(LifecycleSlot::Close)
        }
        EventPayload::Runtime(RuntimePayload::Failed(_))
            if event
                .payload()
                .outcome()
                .is_some_and(|outcome| outcome.category() == Category::Fatal) =>
        {
            Some(LifecycleSlot::Fatal)
        }
        _ => None,
    }
}

/// A pattern whose every `*` stands for any text, possibly empty.
fn pattern_matches(pattern: &str, key: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return pattern == key;
    };
    let Some(mut rest) = key.strip_prefix(first) else {
        return false;
    };
    let parts = parts.collect::<Vec<_>>();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(found) => rest = &rest[found + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}
