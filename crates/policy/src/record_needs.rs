// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR3 (register §1.538; `contracts/resource-targets.md`, "Needs computed from the
//! records"; `contracts/standard-pack-goal-tables.md`, "What is computed"): what the characters a
//! person recorded still need, as the host computes it before each evaluation from the stored
//! records, the root's tables, the progress lists and the holdings, and gives it to the evaluator
//! in memory beside the left-out facts ([`crate::LeftOutFacts::give_record_needs`]). Nothing of it
//! is stored.
//!
//! - **What the host reads through here** ([`NeedsProjection`]): the holding of a pool as the
//!   evaluator reads a target's inventory (the same time-validity projection of the facts and of
//!   the readings given to the pools read from rows of a list, and the same observation), and
//!   whether a progress list is still fresh by the rules a list a pool is read from obeys (its own
//!   expiry, the timeline's resets).
//! - **What the host gives** ([`InstanceRecordNeeds`]): for one instance, each pool's need with how
//!   it was reached ([`PoolNeedKind`]), every part that is not applied, not known, or names what is
//!   not counted ([`RecordNeedsNote`]), the tables' sha256, when the records were set and when the
//!   set's freshness ends; its identity covers all of them.
//! - **What the evaluator does with it**: it stacks it on the stored goals
//!   (`crate::resource_targets::stack_record_needs`), says it in the candidates' reasons
//!   ([`InstanceRecordNeeds::note_reasons`], [`need_reason`]), and hashes its identity into each
//!   candidate's decision identity.
//!
//! Nothing here reads a clock, a file, a lock or the ledger.

use std::collections::BTreeMap;

use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{GoalProblem, ResourceTargetPendingReason};
use actingcommand_selection_policy::UnknownReason;
use sha2::{Digest, Sha256};

use crate::codes::PolicyCode;
use crate::evaluator::{
    TimelineResets, UnknownReasonToken, project_given_validity, project_time_validity,
    survives_resets, timeline_resets, unknown_token,
};
use crate::resource_targets::{
    TargetObservation, bounded_list, observe_target, pending_name, resolve_target_pool,
};
use crate::{
    CompiledCatalog, DecisionReason, EvaluationFacts, EvaluationTime, GivenPoolReadings,
    ListRowsUnknown, PolicyEvaluationResult, ScopeSelector,
};

/// The version word the identity of a computed set is hashed under.
const IDENTITY_VERSION: &str = "actingcommand.goal-records-needs.v1";

/// The needs computed for every instance that holds stored records, by instance alias; an
/// instance without stored records has none, and its evaluation is as without this slice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordNeeds {
    instances: BTreeMap<String, InstanceRecordNeeds>,
}

impl RecordNeeds {
    /// Gives the instance `instance_id` its computed set; a later one replaces it.
    pub fn give(&mut self, instance_id: impl Into<String>, needs: InstanceRecordNeeds) {
        self.instances.insert(instance_id.into(), needs);
    }

    /// The computed set of the instance `instance_id`.
    pub fn get(&self, instance_id: &str) -> Option<&InstanceRecordNeeds> {
        self.instances.get(instance_id)
    }

    /// Whether no instance has a computed set.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Every instance's computed set, by alias.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &InstanceRecordNeeds)> {
        self.instances.iter()
    }
}

/// How a pool's need was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolNeedKind {
    /// The pool of an item without a recipe: what the characters need of it after taking apart
    /// lands on it.
    Lands,
    /// The pool of an item with a recipe whose holding is known: the part of the item's need its
    /// holding covers (the rest is made, and its recipe's items are needed instead).
    Covered,
    /// The pool of an item with a recipe whose holding is not known: the item's whole need, and
    /// nothing is taken apart below it.
    Unsplit,
}

/// One pool's computed need, in the pool's own units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolNeed {
    pub need: u64,
    pub kind: PoolNeedKind,
}

/// One stored record, by its entry and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRef {
    pub entry: String,
    pub key: String,
}

impl RecordRef {
    /// The record as a reason's detail names it: `<entry>/<key>`.
    fn item(&self) -> String {
        format!("{}/{}", self.entry, self.key)
    }
}

/// Why a progress list gives no reading: the reason a list a pool is read from has for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressListUnknown {
    /// The instance holds no such list, or a timeline reset invalidated it: `fact_missing`.
    FactMissing,
    /// The list is past its expiry: `fact_expired`.
    FactExpired,
    /// The list's confidence is 0: `fact_low_confidence`.
    FactLowConfidence,
    /// The list is no record list, or one of its rows holds no text under the list's key column:
    /// `type_mismatch`.
    TypeMismatch,
}

impl ProgressListUnknown {
    /// The fact reading's reason it is.
    const fn reason(self) -> UnknownReason {
        match self {
            Self::FactMissing => UnknownReason::FactMissing,
            Self::FactExpired => UnknownReason::FactExpired,
            Self::FactLowConfidence => UnknownReason::FactLowConfidence,
            Self::TypeMismatch => UnknownReason::TypeMismatch,
        }
    }
}

impl From<ListRowsUnknown> for ProgressListUnknown {
    fn from(unknown: ListRowsUnknown) -> Self {
        match unknown {
            ListRowsUnknown::FactMissing => Self::FactMissing,
            ListRowsUnknown::FactLowConfidence => Self::FactLowConfidence,
            ListRowsUnknown::TypeMismatch => Self::TypeMismatch,
        }
    }
}

/// One part of a computed set that adds nothing, or names what is not counted, each with the
/// code the run says it with (one code, one cause).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordNeedsNote {
    /// `goal_records_not_applied`: a stored record the root in force does not declare (its entry
    /// applies no table of the root's accepted tables, its key is no key of that table, or its
    /// target names a column or a position the key's ladder does not hold; `record` none for a
    /// stored record its reader does not read), with the word a goal edit of it would be refused
    /// with now; or, fix round 1, a record whose table and key an earlier stored record names
    /// already under another entry (`key_repeated`, which no edit refuses: a key is walked once).
    /// The record adds nothing.
    NotApplied {
        record: Option<RecordRef>,
        problem: GoalProblem,
    },
    /// `goal_records_not_held`: the table's `held_by` list, read whole, holds no row of the key:
    /// known, and the record adds nothing.
    NotHeld { record: RecordRef },
    /// `goal_records_progress_unknown`: the list a column of the record reads (`column`), or the
    /// table's `held_by` list (`column` none: the whole record), gives no reading, and why.
    ProgressUnknown {
        record: RecordRef,
        column: Option<String>,
        fact_key: String,
        reason: ProgressListUnknown,
    },
    /// `goal_records_row_missing` (fix round 1, the review: its own cause): the list is known and
    /// holds no row of the key (a list other than the table's `held_by` one): its reading has not
    /// covered the character.
    RowMissing {
        record: RecordRef,
        column: String,
        fact_key: String,
    },
    /// `goal_records_reading_missing`: the list holds the key's row without the column's reading
    /// column `reading`: the package that reads the list does not write it.
    ReadingMissing {
        record: RecordRef,
        column: String,
        fact_key: String,
        reading: String,
    },
    /// `goal_records_position_unknown`: the column's readings form no position of its ladder (a
    /// reading of another kind than a whole number or a truth value, or a tuple equal to no
    /// position).
    PositionUnknown { record: RecordRef, column: String },
    /// `goal_records_holding_unknown`: the holding of an item with a recipe is not known (`reason`
    /// the observation's, none when its pool names no pool of the instance), so nothing below it
    /// is taken apart.
    HoldingUnknown {
        item: String,
        pool_id: String,
        reason: Option<ResourceTargetPendingReason>,
    },
    /// `goal_records_item_unweighed`: an item without a recipe and without a pool: shown with
    /// what the characters need of it, weighing nothing.
    ItemUnweighed { item: String, need: u64 },
    /// `goal_records_condition`: a step the walk takes is asked a condition that is no column's,
    /// which asks for nothing countable.
    Condition {
        record: RecordRef,
        column: String,
        condition: String,
    },
}

/// The length-prefixed parts an identity is hashed from.
struct IdentityWriter(Sha256);

impl IdentityWriter {
    fn text(&mut self, text: &str) {
        self.0.update((text.len() as u64).to_be_bytes());
        self.0.update(text.as_bytes());
    }

    fn number(&mut self, number: u64) {
        self.0.update(number.to_be_bytes());
    }

    fn optional(&mut self, number: Option<u64>) {
        match number {
            None => self.text("none"),
            Some(number) => {
                self.text("some");
                self.number(number);
            }
        }
    }

    fn record(&mut self, record: Option<&RecordRef>) {
        match record {
            None => self.text("none"),
            Some(record) => {
                self.text(&record.entry);
                self.text(&record.key);
            }
        }
    }
}

impl RecordNeedsNote {
    /// Writes the note into an identity.
    fn write(&self, writer: &mut IdentityWriter) {
        match self {
            Self::NotApplied { record, problem } => {
                writer.text("not_applied");
                writer.record(record.as_ref());
                writer.text(problem.as_str());
            }
            Self::NotHeld { record } => {
                writer.text("not_held");
                writer.record(Some(record));
            }
            Self::ProgressUnknown {
                record,
                column,
                fact_key,
                reason,
            } => {
                writer.text("progress_unknown");
                writer.record(Some(record));
                writer.text(column.as_deref().unwrap_or(""));
                writer.text(fact_key);
                writer.text(unknown_token(reason.reason()));
            }
            Self::RowMissing {
                record,
                column,
                fact_key,
            } => {
                writer.text("row_missing");
                writer.record(Some(record));
                writer.text(column);
                writer.text(fact_key);
            }
            Self::ReadingMissing {
                record,
                column,
                fact_key,
                reading,
            } => {
                writer.text("reading_missing");
                writer.record(Some(record));
                writer.text(column);
                writer.text(fact_key);
                writer.text(reading);
            }
            Self::PositionUnknown { record, column } => {
                writer.text("position_unknown");
                writer.record(Some(record));
                writer.text(column);
            }
            Self::HoldingUnknown {
                item,
                pool_id,
                reason,
            } => {
                writer.text("holding_unknown");
                writer.text(item);
                writer.text(pool_id);
                writer.text(reason.map_or("unresolved", pending_name));
            }
            Self::ItemUnweighed { item, need } => {
                writer.text("item_unweighed");
                writer.text(item);
                writer.number(*need);
            }
            Self::Condition {
                record,
                column,
                condition,
            } => {
                writer.text("condition");
                writer.record(Some(record));
                writer.text(column);
                writer.text(condition);
            }
        }
    }
}

/// What the host computed for one instance that holds stored records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRecordNeeds {
    tables_sha256: Option<String>,
    records_observed_at_unix_ms: u64,
    pools: BTreeMap<String, PoolNeed>,
    notes: Vec<RecordNeedsNote>,
    fresh_until_unix_ms: Option<u64>,
    identity: String,
}

impl InstanceRecordNeeds {
    /// The set of `pools` (each by the id the root's files write it, with its need) and `notes`,
    /// computed with the tables of the sha256 `tables_sha256` (none: the root holds no accepted
    /// tables file) from the records observed (set) at `records_observed_at_unix_ms`, fresh until
    /// `fresh_until_unix_ms` (the earliest expiry among the lists and holdings it read; none
    /// without one). Its identity hashes every member.
    pub fn new(
        tables_sha256: Option<String>,
        records_observed_at_unix_ms: u64,
        pools: BTreeMap<String, PoolNeed>,
        notes: Vec<RecordNeedsNote>,
        fresh_until_unix_ms: Option<u64>,
    ) -> Self {
        let mut writer = IdentityWriter(Sha256::new());
        writer.text(IDENTITY_VERSION);
        writer.text(tables_sha256.as_deref().unwrap_or(""));
        writer.number(records_observed_at_unix_ms);
        writer.optional(fresh_until_unix_ms);
        writer.number(pools.len() as u64);
        for (pool_id, need) in &pools {
            writer.text(pool_id);
            writer.number(need.need);
            writer.text(match need.kind {
                PoolNeedKind::Lands => "lands",
                PoolNeedKind::Covered => "covered",
                PoolNeedKind::Unsplit => "unsplit",
            });
        }
        writer.number(notes.len() as u64);
        for note in &notes {
            note.write(&mut writer);
        }
        let identity = format!("{:x}", writer.0.finalize());
        Self {
            tables_sha256,
            records_observed_at_unix_ms,
            pools,
            notes,
            fresh_until_unix_ms,
            identity,
        }
    }

    /// Each pool's need, by the id the root's files write it.
    pub fn pools(&self) -> &BTreeMap<String, PoolNeed> {
        &self.pools
    }

    /// Every part that adds nothing or names what is not counted, in the order computed.
    pub fn notes(&self) -> &[RecordNeedsNote] {
        &self.notes
    }

    /// The sha256 of the tables file the set was computed with.
    pub fn tables_sha256(&self) -> Option<&str> {
        self.tables_sha256.as_deref()
    }

    /// When the records the set was computed from were set (their record's observation).
    pub fn records_observed_at_unix_ms(&self) -> u64 {
        self.records_observed_at_unix_ms
    }

    /// The earliest expiry among the lists and holdings the set read.
    pub fn fresh_until_unix_ms(&self) -> Option<u64> {
        self.fresh_until_unix_ms
    }

    /// The lowercase hex SHA-256 of the whole set.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The word a stored record without a target of its own shows as its policy's identity in
    /// `resource_weights`: `goal-records:<identity>`.
    pub(crate) fn policy_word(&self) -> String {
        format!("goal-records:{}", self.identity)
    }

    /// The reasons that say the set's notes on every candidate of the instance, at most one for
    /// each code and value: `goal_records_not_applied` (by `goal_problem`), `goal_records_not_held`,
    /// `goal_records_progress_unknown` (by `fact_key` and `unknown_reason`),
    /// `goal_records_row_missing` and `goal_records_reading_missing` (each by `fact_key`),
    /// `goal_records_position_unknown`,
    /// `goal_records_holding_unknown` (one for each pool, `pool_id` and the `unknown_reason` of
    /// its observation), `goal_records_item_unweighed` and `goal_records_condition`, in this order;
    /// each lists its records, columns or items in the order computed.
    pub(crate) fn note_reasons(&self) -> Vec<DecisionReason> {
        let mut not_applied = BTreeMap::<GoalProblem, Vec<String>>::new();
        let mut not_held = Vec::new();
        let mut progress = BTreeMap::<(&str, &'static str), (UnknownReason, Vec<String>)>::new();
        let mut row = BTreeMap::<&str, Vec<String>>::new();
        let mut reading = BTreeMap::<&str, Vec<String>>::new();
        let mut position = Vec::new();
        let mut holding = Vec::new();
        let mut unweighed = Vec::new();
        let mut conditions = Vec::new();
        for note in &self.notes {
            match note {
                RecordNeedsNote::NotApplied { record, problem } => {
                    not_applied.entry(*problem).or_default().push(
                        record
                            .as_ref()
                            .map_or_else(|| "record".to_owned(), RecordRef::item),
                    );
                }
                RecordNeedsNote::NotHeld { record } => not_held.push(record.item()),
                RecordNeedsNote::ProgressUnknown {
                    record,
                    column,
                    fact_key,
                    reason,
                } => {
                    let unknown = reason.reason();
                    progress
                        .entry((fact_key.as_str(), unknown_token(unknown)))
                        .or_insert_with(|| (unknown, Vec::new()))
                        .1
                        .push(match column {
                            Some(column) => format!("{}/{column}", record.item()),
                            None => record.item(),
                        });
                }
                RecordNeedsNote::RowMissing {
                    record,
                    column,
                    fact_key,
                } => row
                    .entry(fact_key.as_str())
                    .or_default()
                    .push(format!("{}/{column}", record.item())),
                RecordNeedsNote::ReadingMissing {
                    record,
                    column,
                    fact_key,
                    reading: missing,
                } => reading
                    .entry(fact_key.as_str())
                    .or_default()
                    .push(format!("{}/{column}:{missing}", record.item())),
                RecordNeedsNote::PositionUnknown { record, column } => {
                    position.push(format!("{}/{column}", record.item()));
                }
                RecordNeedsNote::HoldingUnknown {
                    item,
                    pool_id,
                    reason,
                } => holding.push((item.as_str(), pool_id.as_str(), *reason)),
                RecordNeedsNote::ItemUnweighed { item, need } => {
                    unweighed.push(format!("{item}:{need}"));
                }
                RecordNeedsNote::Condition {
                    record,
                    column,
                    condition,
                } => conditions.push(format!("{}/{column}:{condition}", record.item())),
            }
        }
        let listed = |items: &[String]| bounded_list(String::new(), items, ",");
        let mut reasons = Vec::new();
        for (problem, items) in &not_applied {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsNotApplied).goal_problem(*problem),
                detail: listed(items.as_slice()),
            });
        }
        if !not_held.is_empty() {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsNotHeld),
                detail: listed(not_held.as_slice()),
            });
        }
        for ((fact_key, _), (unknown, items)) in &progress {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsProgressUnknown)
                    .fact_key(*fact_key)
                    .unknown_reason(UnknownReasonToken(*unknown)),
                detail: listed(items.as_slice()),
            });
        }
        for (fact_key, items) in &row {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsRowMissing).fact_key(*fact_key),
                detail: listed(items.as_slice()),
            });
        }
        for (fact_key, items) in &reading {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsReadingMissing).fact_key(*fact_key),
                detail: listed(items.as_slice()),
            });
        }
        if !position.is_empty() {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsPositionUnknown),
                detail: listed(position.as_slice()),
            });
        }
        for (item, pool_id, reason) in holding {
            let mut outcome = Outcome::new(PolicyCode::GoalRecordsHoldingUnknown).pool_id(pool_id);
            if let Some(reason) = reason {
                outcome = outcome.unknown_reason(UnknownReasonToken(pending_unknown(reason)));
            }
            reasons.push(DecisionReason {
                outcome,
                detail: format!("item={item}"),
            });
        }
        if !unweighed.is_empty() {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsItemUnweighed),
                detail: listed(unweighed.as_slice()),
            });
        }
        if !conditions.is_empty() {
            reasons.push(DecisionReason {
                outcome: Outcome::new(PolicyCode::GoalRecordsCondition),
                detail: listed(conditions.as_slice()),
            });
        }
        reasons
    }
}

/// The fact reading's reason of a target's pending observation.
const fn pending_unknown(reason: ResourceTargetPendingReason) -> UnknownReason {
    match reason {
        ResourceTargetPendingReason::Missing => UnknownReason::FactMissing,
        ResourceTargetPendingReason::Expired => UnknownReason::FactExpired,
        ResourceTargetPendingReason::LowConfidence => UnknownReason::FactLowConfidence,
        ResourceTargetPendingReason::InvalidValue => UnknownReason::TypeMismatch,
    }
}

/// `goal_records_need` (info) of the pool `pool_id` on a candidate its stacked goal covers: what
/// the characters need of it (`needed`), the stock its stored goal asks (`stock`, 0 without one),
/// its holding (`held`), how much of the holding goes to the characters (`to_characters`, the
/// lesser of the two), the gap (`max(0, needed + stock - held)`) and the tables' sha256; the holding
/// and what follows from it `pending` while the evaluation does not know it.
pub(crate) fn need_reason(
    pool_id: &str,
    need: PoolNeed,
    stock: u64,
    held_and_gap: Option<(u64, u64)>,
    tables_sha256: Option<&str>,
) -> DecisionReason {
    let (held, to_characters, gap) = match held_and_gap {
        Some((held, gap)) => (
            held.to_string(),
            held.min(need.need).to_string(),
            gap.to_string(),
        ),
        None => (
            "pending".to_owned(),
            "pending".to_owned(),
            "pending".to_owned(),
        ),
    };
    DecisionReason {
        outcome: Outcome::new(PolicyCode::GoalRecordsNeed).pool_id(pool_id),
        detail: format!(
            "needed={} stock={stock} held={held} to_characters={to_characters} gap={gap} tables={}",
            need.need,
            tables_sha256.unwrap_or("none")
        ),
    }
}

/// `goal_records_not_stacked` (warning): the instance's stored target record is a v1 policy, on
/// which no computed need is stacked.
pub(crate) fn not_stacked_reason() -> DecisionReason {
    DecisionReason {
        outcome: Outcome::new(PolicyCode::GoalRecordsNotStacked),
        detail: "the instance's stored target record is a v1 policy, which weighs its named tasks \
                 alone; the needs its records compute are not stacked on it"
            .to_owned(),
    }
}

/// What the host reads of one evaluation's inputs to compute the needs (module doc): the facts and
/// the readings given to the pools read from rows of a list as the evaluator projects them at the
/// evaluation's time, and the timeline's resets at that time.
pub struct NeedsProjection<'a> {
    catalog: &'a CompiledCatalog,
    projected: EvaluationFacts,
    given: GivenPoolReadings,
    resets: TimelineResets<'a>,
    time: EvaluationTime,
}

/// The holding of a pool as the evaluator reads a target's inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedHolding {
    /// Known: the amount held, fresh until the observation's end.
    Known {
        held: u64,
        fresh_until_unix_ms: Option<u64>,
    },
    /// Not known: the observation's reason, none when the id names no pool of the instance (or
    /// one that is not observable or out of its scope).
    Unknown(Option<ResourceTargetPendingReason>),
}

impl<'a> NeedsProjection<'a> {
    /// The projection of `facts` and `readings` an evaluation of `catalog` at `time` reads.
    pub fn new(
        catalog: &'a CompiledCatalog,
        facts: &EvaluationFacts,
        readings: &GivenPoolReadings,
        time: EvaluationTime,
    ) -> PolicyEvaluationResult<Self> {
        Ok(Self {
            catalog,
            projected: project_time_validity(catalog, facts, time)?,
            given: project_given_validity(catalog, readings, &facts.instances, time)?,
            resets: timeline_resets(catalog, time)?,
            time,
        })
    }

    /// The holding on the instance `instance_id` of the pool the root's files write `resource`:
    /// the pool a target of that id names on the instance, observed as a target's inventory is
    /// (`observe_target`), the one reading the evaluator reads.
    pub fn holding(&self, instance_id: &str, resource: &str) -> NeedHolding {
        let Some(instance) = self
            .projected
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
        else {
            return NeedHolding::Unknown(None);
        };
        let Ok((pool, fact_key)) = resolve_target_pool(self.catalog, resource, instance) else {
            return NeedHolding::Unknown(None);
        };
        match observe_target(
            &self.projected,
            &self.given,
            instance,
            pool,
            &fact_key,
            self.time,
        ) {
            TargetObservation::Known {
                current,
                fresh_until_unix_ms,
                ..
            } => NeedHolding::Known {
                held: current.unsigned_abs(),
                fresh_until_unix_ms,
            },
            TargetObservation::Pending { reason } => NeedHolding::Unknown(Some(reason)),
        }
    }

    /// Whether the instance `instance_id`'s list `fact_key`, observed at `observed_at_unix_ms`
    /// with the expiry `expires_at_unix_ms`, is fresh at the evaluation's time, by the rules a list
    /// a pool is read from obeys: a timeline reset that invalidates it at or after its observation
    /// leaves it missing (`fact_missing`), one still to come bounds its expiry, and past its
    /// expiry it is `fact_expired`. Fresh, its expiry.
    pub fn list_freshness(
        &self,
        instance_id: &str,
        fact_key: &str,
        observed_at_unix_ms: u64,
        expires_at_unix_ms: Option<u64>,
    ) -> Result<Option<u64>, ProgressListUnknown> {
        let mut expiry = expires_at_unix_ms;
        let scope = ScopeSelector::Instance {
            instance_id: instance_id.to_owned(),
        };
        if !survives_resets(
            &self.resets,
            &self.projected.instances,
            &scope,
            fact_key,
            observed_at_unix_ms,
            &mut expiry,
        ) {
            return Err(ProgressListUnknown::FactMissing);
        }
        if expiry.is_some_and(|expires| self.time.unix_ms > expires) {
            return Err(ProgressListUnknown::FactExpired);
        }
        Ok(expiry)
    }
}
