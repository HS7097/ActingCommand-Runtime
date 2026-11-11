// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-8 (`contracts/instance-fact-store.md`, "Pack schedule facts"; SR-11, with #429
//! A.7.1): during which periods a task package may be scheduled on an instance, a state a person,
//! an agent or the operator writes as the instance fact `session.pack.schedule`, one record per
//! scope (instance, server or game).
//!
//! - **Rows** `{package_id, from_unix_ms, until_unix_ms?, schedulable}`: from `from_unix_ms` until
//!   `until_unix_ms` (half-open; without it the row is open-ended) the package is schedulable or
//!   not. A list is refused as the non-fatal request error `session_pack_schedule_invalid`
//!   (`fact_key`, `field`) when it is not an inline `record_list` of such rows, holds more than
//!   the `record_list` bound of rows, has a row whose `until_unix_ms` is not after its
//!   `from_unix_ms`, or two rows of one package that overlap (an open-ended row overlaps
//!   everything after its start) ([`check_record`], [`edit_rows`]).
//! - **Which row applies** ([`resolve`]): each package on its own, layer by layer. Of the layers
//!   that hold a row of the package, the most specific (instance over server over game) decides:
//!   its row that covers the instant, else the package is schedulable. A layer without a row of
//!   the package decides nothing for it. The standard package's default layer is Workflow #429
//!   R-9's ([`pack_default`], which decides nothing yet). `below` is the same resolution without
//!   the deciding layer.
//! - **The evaluation** ([`CycleSchedule`]): reads the records as the evaluation's facts hold them,
//!   so the snapshot identity already covers the records it read; a candidate of a closed package
//!   is deferred `pack_schedule_closed`, waking when the covering row ends. A row of a package no
//!   catalog task uses, and a record that cannot be read, are each reported once per evaluated
//!   cycle and decide nothing; another instance is never touched by one instance's record (#419).
//!
//! Package ids are opaque tokens compared for equality only; nothing here knows what a package
//! does.

use crate::codes::SchedulingRuleHostCode;
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::outcome::{Location, Outcome};
use actingcommand_contract::{
    FactContent, FactRecord, FactScalar as ContractFactScalar, FactScope,
    FactValue as ContractFactValue, PackScheduleBelow, PackScheduleEditRow, PackScheduleSource,
    RuntimeErrorCode,
};
use actingcommand_policy::{
    CandidateEligibility, DecisionReason, DispatchIntent, EligibilityState, EvaluationFacts,
    FactScalar as PolicyFactScalar, FactValue as PolicyFactValue, InstanceSnapshot,
    SchedulingDecisionState, SchedulingRulePolicyCode, ScopeSelector, TaskDecision,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use actingcommand_contract::PACK_SCHEDULE_FACT_KEY;

const PACKAGE_ID: &str = "package_id";
const FROM: &str = "from_unix_ms";
const UNTIL: &str = "until_unix_ms";
const SCHEDULABLE: &str = "schedulable";
const MEMBERS: [&str; 4] = [PACKAGE_ID, FROM, UNTIL, SCHEDULABLE];
/// The `record_list` bound: one scope holds at most this many rows, for all its packages.
const MAX_ROWS: usize = 256;
/// The key table's bound of a `name` value (`package_id` in the codes' values).
const MAX_PACKAGE_ID_BYTES: usize = 256;
/// The key table's bound of a `pointer` value.
const MAX_POINTER_BYTES: usize = 512;
/// The pointer of an edit's package and of its rows.
const EDIT_PACKAGE_POINTER: &str = "/package_id";
const EDIT_ROWS_POINTER: &str = "/rows";
/// What an edited record names as its writer and schema, and the prefix of its snapshot id.
const EDIT_DETECTOR: &str = "runtime.pack-schedule";
const EDIT_SCHEMA: &str = "fact.v1";
const EDIT_SNAPSHOT_PREFIX: &str = "pack-schedule:";

/// One row of a scope's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) package_id: String,
    pub(crate) from_unix_ms: u64,
    pub(crate) until_unix_ms: Option<u64>,
    pub(crate) schedulable: bool,
}

impl Row {
    /// Whether the row's period holds `at`.
    fn covers(&self, at: u64) -> bool {
        self.from_unix_ms <= at && self.until_unix_ms.is_none_or(|until| at < until)
    }

    /// Whether the two periods share an instant (an open-ended period never ends).
    fn overlaps(&self, other: &Self) -> bool {
        let starts_before_end =
            |from: u64, until: Option<u64>| until.is_none_or(|until| from < until);
        starts_before_end(self.from_unix_ms, other.until_unix_ms)
            && starts_before_end(other.from_unix_ms, self.until_unix_ms)
    }

    /// The row as an `EditPackSchedule` writes it and `PackScheduleView` names it.
    pub(crate) fn edit_row(&self) -> PackScheduleEditRow {
        PackScheduleEditRow {
            from_unix_ms: self.from_unix_ms,
            until_unix_ms: self.until_unix_ms,
            schedulable: self.schedulable,
        }
    }

    /// The row as the record stores it.
    fn stored(&self) -> BTreeMap<String, ContractFactScalar> {
        let mut row = BTreeMap::from([
            (
                PACKAGE_ID.to_owned(),
                ContractFactScalar::String(self.package_id.clone()),
            ),
            (
                FROM.to_owned(),
                ContractFactScalar::TimestampMs(self.from_unix_ms),
            ),
            (
                SCHEDULABLE.to_owned(),
                ContractFactScalar::Boolean(self.schedulable),
            ),
        ]);
        if let Some(until) = self.until_unix_ms {
            row.insert(UNTIL.to_owned(), ContractFactScalar::TimestampMs(until));
        }
        row
    }
}

/// A refused list: where (`field`, a JSON pointer into the list, or into an edit request).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    field: String,
}

impl Refusal {
    fn at(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
        }
    }

    /// `session_pack_schedule_invalid` (`fact_key`, `field`), a non-fatal request error of
    /// `operation`.
    pub(crate) fn into_error(self, operation: impl Into<Location>) -> RuntimeHostError {
        let Self { field } = self;
        RuntimeHostError::request(
            SchedulingRuleHostCode::SessionPackScheduleInvalid,
            operation,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_outcome_values(move |outcome| outcome.fact_key(PACK_SCHEDULE_FACT_KEY).field(field))
    }

    /// The outcome of a record the evaluation could not read (a warning of the cycle).
    fn outcome(&self) -> Outcome {
        Outcome::new(SchedulingRuleHostCode::SessionPackScheduleInvalid)
            .fact_key(PACK_SCHEDULE_FACT_KEY)
            .field(self.field.as_str())
    }
}

/// The write rule of one published `session.pack.schedule` record: an inline `record_list`
/// whose rows pass [`read_rows`].
pub(crate) fn check_record(record: &FactRecord) -> Result<(), Refusal> {
    let FactContent::Inline { value } = &record.content else {
        return Err(Refusal::at(""));
    };
    read_rows(&crate::fact_store::policy_value(value)).map(drop)
}

/// The rows of a stored list (an inline value), or the first rule it breaks.
pub(crate) fn stored_rows(record: &FactRecord) -> Result<Vec<Row>, Refusal> {
    let FactContent::Inline { value } = &record.content else {
        return Err(Refusal::at(""));
    };
    read_rows(&crate::fact_store::policy_value(value))
}

/// The rows of a list value, or the first rule it breaks: its shape row by row (exactly the four
/// members, `until_unix_ms` optional, each of its type, a package id that is a non-empty text),
/// then the periods ([`check_periods`]).
pub(crate) fn read_rows(value: &PolicyFactValue) -> Result<Vec<Row>, Refusal> {
    let PolicyFactValue::RecordList(items) = value else {
        return Err(Refusal::at(""));
    };
    if items.len() > MAX_ROWS {
        return Err(Refusal::at(""));
    }
    let mut rows = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let at = |member: &str| Refusal::at(pointer("", index, Some(member)));
        if let Some(other) = item
            .keys()
            .find(|member| !MEMBERS.contains(&member.as_str()))
        {
            return Err(at(other.as_str()));
        }
        let Some(PolicyFactScalar::String(package_id)) = item.get(PACKAGE_ID) else {
            return Err(at(PACKAGE_ID));
        };
        if !package_id_usable(package_id) {
            return Err(at(PACKAGE_ID));
        }
        let Some(PolicyFactScalar::TimestampMs(from_unix_ms)) = item.get(FROM) else {
            return Err(at(FROM));
        };
        let until_unix_ms = match item.get(UNTIL) {
            None => None,
            Some(PolicyFactScalar::TimestampMs(until)) => Some(*until),
            Some(_) => return Err(at(UNTIL)),
        };
        let Some(PolicyFactScalar::Boolean(schedulable)) = item.get(SCHEDULABLE) else {
            return Err(at(SCHEDULABLE));
        };
        rows.push(Row {
            package_id: package_id.clone(),
            from_unix_ms: *from_unix_ms,
            until_unix_ms,
            schedulable: *schedulable,
        });
    }
    check_periods(&rows, "")?;
    Ok(rows)
}

/// The periods of `rows`, in order: a row whose `until_unix_ms` is not after its `from_unix_ms`
/// (at its `until_unix_ms`), then a row that overlaps an earlier row of its package (at the later
/// row as a whole). `prefix` is the pointer of the list.
fn check_periods(rows: &[Row], prefix: &str) -> Result<(), Refusal> {
    for (index, row) in rows.iter().enumerate() {
        if row
            .until_unix_ms
            .is_some_and(|until| until <= row.from_unix_ms)
        {
            return Err(Refusal::at(pointer(prefix, index, Some(UNTIL))));
        }
        if rows[..index]
            .iter()
            .any(|earlier| earlier.package_id == row.package_id && earlier.overlaps(row))
        {
            return Err(Refusal::at(pointer(prefix, index, None)));
        }
    }
    Ok(())
}

/// The rows an `EditPackSchedule` writes for `package_id`, or the first rule they break: the
/// package (`/package_id`), the bound (`/rows`) and the periods (`/rows/<n>…`).
pub(crate) fn edit_rows(
    package_id: &str,
    rows: &[PackScheduleEditRow],
) -> Result<Vec<Row>, Refusal> {
    if !package_id_usable(package_id) {
        return Err(Refusal::at(EDIT_PACKAGE_POINTER));
    }
    if rows.len() > MAX_ROWS {
        return Err(Refusal::at(EDIT_ROWS_POINTER));
    }
    let rows = rows
        .iter()
        .map(|row| Row {
            package_id: package_id.to_owned(),
            from_unix_ms: row.from_unix_ms,
            until_unix_ms: row.until_unix_ms,
            schedulable: row.schedulable,
        })
        .collect::<Vec<_>>();
    check_periods(&rows, EDIT_ROWS_POINTER)?;
    Ok(rows)
}

/// A scope's list after an edit of `package_id`: the rows of `kept` of every other package, in
/// their order, then `edited`; more rows than the bound are refused (`/rows`).
pub(crate) fn replace_rows(
    kept: Vec<Row>,
    package_id: &str,
    edited: Vec<Row>,
) -> Result<Vec<Row>, Refusal> {
    let list = kept
        .into_iter()
        .filter(|row| row.package_id != package_id)
        .chain(edited)
        .collect::<Vec<_>>();
    if list.len() > MAX_ROWS {
        return Err(Refusal::at(EDIT_ROWS_POINTER));
    }
    Ok(list)
}

/// The record an `EditPackSchedule` publishes: `rows` as `scope`'s whole list, observed at
/// `observed_at_unix_ms`, without expiry or invalidating events; its `source_snapshot_id` is
/// `pack-schedule:<sha256 hex of (scope, rows, observed_at_unix_ms)>`, so every edit is a new
/// version.
pub(crate) fn edit_record(
    scope: FactScope,
    rows: &[Row],
    observed_at_unix_ms: u64,
    operation: impl Into<Location>,
) -> RuntimeHostResult<FactRecord> {
    let value = ContractFactValue::RecordList(rows.iter().map(Row::stored).collect());
    let identity = serde_json::to_vec(&(&scope, &value, observed_at_unix_ms)).map_err(|_| {
        RuntimeHostError::fatal(
            crate::codes::HostCode::FactSnapshotEncodeFailed,
            operation,
            RuntimeErrorCode::RuntimeHalted,
        )
    })?;
    let digest = format!("{:x}", Sha256::digest(identity));
    Ok(FactRecord {
        scope,
        key: PACK_SCHEDULE_FACT_KEY.to_owned(),
        content: FactContent::Inline { value },
        observed_at_unix_ms,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: EDIT_DETECTOR.to_owned(),
        source_snapshot_id: format!("{EDIT_SNAPSHOT_PREFIX}{digest}"),
        schema_version: EDIT_SCHEMA.to_owned(),
        resource_bundle_hash: digest,
        invalidate_on: Vec::new(),
    })
}

/// A package id a row may name: a text of 1 to 256 bytes without a control character.
fn package_id_usable(package_id: &str) -> bool {
    !package_id.is_empty()
        && package_id.len() <= MAX_PACKAGE_ID_BYTES
        && !package_id.chars().any(char::is_control)
}

/// The JSON pointer of row `index` of the list at `prefix` (and `member`, RFC 6901 escaped); the
/// row's own pointer when the member's would pass the key table's bound.
fn pointer(prefix: &str, index: usize, member: Option<&str>) -> String {
    let row = format!("{prefix}/{index}");
    match member {
        Some(member) => {
            let full = format!("{row}/{}", member.replace('~', "~0").replace('/', "~1"));
            if full.len() <= MAX_POINTER_BYTES {
                full
            } else {
                row
            }
        }
        None => row,
    }
}

/// The three layers of `session.pack.schedule` records that apply to one instance, most specific
/// first: instance, server, game. A layer without a readable, unexpired record holds none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Layers {
    rows: [Option<Vec<Row>>; 3],
}

/// The sources of the three layers, in order.
const LAYERS: [PackScheduleSource; 3] = [
    PackScheduleSource::Instance,
    PackScheduleSource::Server,
    PackScheduleSource::Game,
];

impl Layers {
    /// The layers of `rows`, given in the order of [`LAYERS`].
    pub(crate) fn new(rows: [Option<Vec<Row>>; 3]) -> Self {
        Self { rows }
    }

    /// Every package a row of a layer names.
    fn packages(&self) -> impl Iterator<Item = &str> {
        self.rows
            .iter()
            .flatten()
            .flatten()
            .map(|row| row.package_id.as_str())
    }
}

/// How one package resolves on one instance at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) schedulable: bool,
    pub(crate) source: PackScheduleSource,
    /// The row that decided; none when `source` is `none`.
    pub(crate) row: Option<Row>,
    /// When the decision next changes: the deciding row's end, or the start of the deciding
    /// layer's next row of the package; none when no row bounds it.
    pub(crate) next_change_unix_ms: Option<u64>,
    /// The decision without the deciding layer's rows of the package; none when `source` is the
    /// pack default or `none`.
    pub(crate) below: Option<PackScheduleBelow>,
}

/// SR-11 for `package_id` over `layers` at `at` (see the module's description), with `below`.
pub(crate) fn resolve(layers: &Layers, package_id: &str, at: u64) -> Decision {
    let decided = resolve_from(layers, package_id, at, 0);
    let below = LAYERS
        .iter()
        .position(|source| *source == decided.source)
        .map(|deciding| {
            let below = resolve_from(layers, package_id, at, deciding + 1);
            PackScheduleBelow {
                schedulable: below.schedulable,
                source: below.source,
            }
        });
    Decision { below, ..decided }
}

/// SR-11 for `package_id` over the layers from `first` on (without `below`).
fn resolve_from(layers: &Layers, package_id: &str, at: u64, first: usize) -> Decision {
    // 1. The instruction layer: the most specific layer that holds a row of the package.
    for (rows, source) in layers.rows.iter().zip(LAYERS).skip(first) {
        let own = rows
            .iter()
            .flatten()
            .filter(|row| row.package_id == package_id)
            .collect::<Vec<_>>();
        if own.is_empty() {
            continue;
        }
        if let Some(&row) = own.iter().find(|row| row.covers(at)) {
            return Decision {
                schedulable: row.schedulable,
                source,
                row: Some(row.clone()),
                next_change_unix_ms: row.until_unix_ms,
                below: None,
            };
        }
        let next = own
            .iter()
            .map(|row| row.from_unix_ms)
            .filter(|from| *from > at)
            .min();
        return unruled(package_id, at, next);
    }
    unruled(package_id, at, None)
}

/// SR-11 items 2 and 3, when no instruction row covers `at`: the pack default layer, else
/// schedulable (`none`). `next` is when the deciding instruction layer's next row begins.
fn unruled(package_id: &str, at: u64, next: Option<u64>) -> Decision {
    if let Some(decision) = pack_default(package_id, at) {
        return decision;
    }
    Decision {
        schedulable: true,
        source: PackScheduleSource::None,
        row: None,
        next_change_unix_ms: next,
        below: None,
    }
}

/// SR-11 item 2, the standard package layer: the default row of `package_id` that covers `at`,
/// which the instance's standard package declares in a data file at its root, compiled when the
/// catalog is activated (`source` `pack_default`). **Workflow #429 R-9 adds it here**; until then
/// no package has a default row, so this layer decides nothing.
fn pack_default(_package_id: &str, _at: u64) -> Option<Decision> {
    None
}

/// The record of one layer as the evaluation reads it: absent when expired at `at`.
fn layer_fact<'a>(
    facts: &'a EvaluationFacts,
    scope: &ScopeSelector,
    at: u64,
) -> Option<&'a PolicyFactValue> {
    facts
        .facts
        .iter()
        .find(|fact| {
            fact.fact_key == PACK_SCHEDULE_FACT_KEY
                && fact.scope == *scope
                && fact.expires_at_unix_ms.is_none_or(|expires| expires > at)
        })
        .map(|fact| &fact.value)
}

/// The three scopes that apply to `instance`, in the order of [`LAYERS`].
fn instance_scopes(instance: &InstanceSnapshot) -> [ScopeSelector; 3] {
    [
        ScopeSelector::Instance {
            instance_id: instance.instance_id.clone(),
        },
        ScopeSelector::Server {
            server_id: instance.server_id.clone(),
        },
        ScopeSelector::Game {
            game_id: instance.game_id.clone(),
        },
    ]
}

/// The pack schedule of one evaluation (Workflow #429 R-8, `scheduling-blackout.md`, "Order of
/// the deferrals", position 5): each instance's layers as the evaluation's facts hold them, the
/// package each catalog task runs, and what the cycle reports. Without any `session.pack.schedule`
/// record it holds nothing, defers nothing and reports nothing.
#[derive(Debug, Default)]
pub(crate) struct CycleSchedule {
    at_unix_ms: u64,
    /// By task id, the package its bound package declares (the remedy table's scan).
    task_packages: BTreeMap<String, String>,
    /// By instance alias.
    layers: BTreeMap<String, Layers>,
    /// The cycle's reports: an unknown package, a record that cannot be read.
    notes: Vec<TaskDecision>,
}

impl CycleSchedule {
    /// The pack schedule of an evaluation of `facts` at `at_unix_ms`; `task_packages` is the
    /// package of each catalog task (none when no table was built).
    pub(crate) fn new(
        facts: &EvaluationFacts,
        task_packages: Option<&BTreeMap<String, String>>,
        at_unix_ms: u64,
    ) -> Self {
        if !facts
            .facts
            .iter()
            .any(|fact| fact.fact_key == PACK_SCHEDULE_FACT_KEY)
        {
            return Self::default();
        }
        let task_packages = task_packages.cloned().unwrap_or_default();
        let mut unreadable = BTreeMap::<String, Refusal>::new();
        let mut layers = BTreeMap::new();
        for instance in &facts.instances {
            let scopes = instance_scopes(instance);
            let rows = scopes.each_ref().map(|scope| {
                let value = layer_fact(facts, scope, at_unix_ms)?;
                match read_rows(value) {
                    Ok(rows) => Some(rows),
                    // #419: a record this evaluation cannot read decides nothing for the instances
                    // it covers, and is reported once; no other record is touched.
                    Err(refusal) => {
                        unreadable.insert(scope_label(scope), refusal);
                        None
                    }
                }
            });
            layers.insert(instance.instance_id.clone(), Layers::new(rows));
        }
        let used = task_packages
            .values()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let unknown = layers
            .values()
            .flat_map(Layers::packages)
            .filter(|package_id| !used.contains(package_id))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let mut notes = unknown
            .into_iter()
            .map(|package_id| {
                note(DecisionReason {
                    outcome: Outcome::new(SchedulingRulePolicyCode::PackScheduleUnknownPackage)
                        .package_id(package_id.as_str()),
                    detail: format!(
                        "a session.pack.schedule row names the package '{package_id}', which no \
                         task of the active catalog uses; the row has no effect"
                    ),
                })
            })
            .collect::<Vec<_>>();
        notes.extend(unreadable.into_iter().map(|(scope, refusal)| {
            note(DecisionReason {
                outcome: refusal.outcome(),
                detail: format!(
                    "the session.pack.schedule record of {scope} cannot be read at {}; it \
                     decides nothing in this evaluation",
                    refusal.field
                ),
            })
        }));
        Self {
            at_unix_ms,
            task_packages,
            layers,
            notes,
        }
    }

    /// Position 5: the deferral of `intent` when the instance's pack schedule closes the package
    /// its task runs, `pack_schedule_closed` with the package and, when the covering row ends,
    /// that end as `next_eligible_unix_ms` and the candidate's wake; `None` when it is
    /// schedulable. A fact refresh's deferral names the code as its gate.
    pub(crate) fn deferral(
        &self,
        intent: &DispatchIntent,
        refresh: bool,
    ) -> Option<CandidateEligibility> {
        let layers = self.layers.get(&intent.instance_id)?;
        let package_id = self.task_packages.get(&intent.task_id)?;
        let decision = resolve(layers, package_id, self.at_unix_ms);
        if decision.schedulable {
            return None;
        }
        let wake = decision.row.as_ref().and_then(|row| row.until_unix_ms);
        let mut outcome = Outcome::new(SchedulingRulePolicyCode::PackScheduleClosed)
            .package_id(package_id.as_str());
        if let Some(wake) = wake {
            outcome = outcome.next_eligible_unix_ms(wake);
        }
        let reason = DecisionReason {
            outcome,
            detail: format!(
                "Runtime pack schedule: package_id={package_id}; source={:?}; \
                 next_eligible_unix_ms={wake:?}",
                decision.source
            ),
        };
        Some(if refresh {
            CandidateEligibility::Refresh {
                reason,
                next_wake_unix_ms: wake,
                gate: SchedulingRulePolicyCode::PackScheduleClosed.code(),
                limit: None,
            }
        } else {
            CandidateEligibility::Deferred {
                reason,
                next_wake_unix_ms: wake,
            }
        })
    }

    /// The cycle's reports, each once: one decision of the key, with no instance, per package a
    /// row names that no catalog task uses (`pack_schedule_unknown_package`) and per record that
    /// cannot be read (`session_pack_schedule_invalid`).
    pub(crate) fn into_notes(self) -> Vec<TaskDecision> {
        self.notes
    }
}

/// One report of the cycle: a decision of the key itself, with no instance, eligibility unknown,
/// state blocked, no rank and the one reason.
fn note(reason: DecisionReason) -> TaskDecision {
    TaskDecision {
        task_id: PACK_SCHEDULE_FACT_KEY.to_owned(),
        instance_id: None,
        eligibility: EligibilityState::Unknown,
        state: SchedulingDecisionState::Blocked,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: vec![reason],
        next_wake_unix_ms: None,
    }
}

/// A scope as a report names it.
fn scope_label(scope: &ScopeSelector) -> String {
    match scope {
        ScopeSelector::Instance { instance_id } => format!("instance {instance_id}"),
        ScopeSelector::Server { server_id } => format!("server {server_id}"),
        ScopeSelector::Game { game_id } => format!("game {game_id}"),
    }
}
