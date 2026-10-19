// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-6b, MCP-19b (register §1.144; plan-mcp19 §2, Q4, Q8, Q9): what actingctl and
//! MCP show of the Runtime's `ScheduleExplanation` (register §1.143, §3 W5), the latest policy
//! evaluation since the Runtime started: per instance its not-running row, its selected task with
//! the state of its dispatch and every candidate that did not run, with its `why`, its reason
//! codes and its wake. The Runtime answers it from the policy host's memory and records nothing.
//!
//! - `actingctl schedule [--instance <alias|instance_id|ADB port>]` prints the answer verbatim,
//!   candidates included (`main.rs`), as MCP `ac_schedule` answers it (`mcp::schedule`).
//! - `overview` / `ac_overview` read it without candidates and give each instance row the
//!   `not_running` row of its entry ([`not_running_by_alias`]).
//! - `diagnose` / `ac_diagnose` read it with candidates and list the deferred rows (due, or
//!   waiting for a gate; not the rows blocked by their own trigger), at most
//!   `scheduling.not_run.diagnose_rows` per instance, the value the program fact
//!   `config.parameters.scheduling` holds ([`not_run`]).

use crate::codes::ActingctlCode;
use crate::overview::Warning;
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    FactScalar, FactValue, RuntimeFactScope, RuntimeFactSnapshot, RuntimeScheduleExplanation,
    ScheduleCandidateState, config_parameter_section, config_parameters_fact_key,
};
use actingcommand_runtime_client::RuntimeClientError;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The configuration key that bounds `not_run` per instance (`contracts/runtime-config.md`,
/// default 10); its row is in the program fact `config.parameters.scheduling`.
pub(crate) const DIAGNOSE_ROWS_KEY: &str = "scheduling.not_run.diagnose_rows";

/// The `not_running` row of each entry of `explanation`, by the entry's instance alias, verbatim:
/// the member `overview` adds to the instance row of that alias.
pub(crate) fn not_running_by_alias(
    explanation: &RuntimeScheduleExplanation,
) -> BTreeMap<&str, Value> {
    explanation
        .instances
        .iter()
        .map(|entry| (entry.instance_alias.as_str(), json!(entry.not_running)))
        .collect()
}

/// The deferred rows of a diagnosis: the rows listed and how many there are in the answer.
pub(crate) struct NotRun {
    /// Per entry of the answer, in its order, its first rows whose `state` is `deferred` (at most
    /// the bound), each the candidate row verbatim with its `instance_alias`.
    pub(crate) rows: Vec<Value>,
    /// Every deferred row of the answer, listed or not.
    pub(crate) total: usize,
}

/// `not_run` of a diagnosis from the two reads it takes: `explanation`, the ScheduleExplanation
/// read with candidates (of the diagnosed instance, or of every instance), and `facts`, the
/// RuntimeFactSnapshot whose `config.parameters.scheduling` holds the bound. A read that failed,
/// or a bound that cannot be read, is one warning each and leaves no `not_run`.
pub(crate) fn not_run(
    explanation: Result<RuntimeScheduleExplanation, RuntimeClientError>,
    facts: Result<RuntimeFactSnapshot, RuntimeClientError>,
    warnings: &mut Vec<Warning>,
) -> Option<NotRun> {
    let bound = match facts {
        Ok(facts) => diagnose_rows(&facts).map_err(Warning::Outcome),
        Err(error) => Err(Warning::Runtime(error)),
    };
    match (explanation.map_err(Warning::Runtime), bound) {
        (Ok(explanation), Ok(bound)) => Some(deferred_rows(&explanation, bound)),
        (explanation, bound) => {
            warnings.extend(explanation.err());
            warnings.extend(bound.err());
            None
        }
    }
}

/// The first `bound` rows of each entry whose `state` is `deferred`, in the answer's order, each
/// with its `instance_alias` first; and how many such rows the answer holds.
fn deferred_rows(explanation: &RuntimeScheduleExplanation, bound: usize) -> NotRun {
    let mut rows = Vec::new();
    let mut total = 0;
    for entry in &explanation.instances {
        let deferred = entry.candidates.iter().filter(|row| {
            row.state == ScheduleCandidateState::Deferred
                || row.state == ScheduleCandidateState::Blocked
        });
        for (index, row) in deferred.enumerate() {
            total += 1;
            if index < bound {
                let mut members = Map::new();
                members.insert("instance_alias".to_owned(), json!(entry.instance_alias));
                if let Value::Object(row) = json!(row) {
                    members.extend(row);
                }
                rows.push(Value::Object(members));
            }
        }
    }
    NotRun { rows, total }
}

/// The value of `scheduling.not_run.diagnose_rows` in the program fact
/// `config.parameters.scheduling` (one row `{key, value, source, settable}` per parameter of the
/// section, as the Runtime records the configuration in effect). A snapshot without the record
/// or the row is `config_facts_missing`; a record that is no row list, or a row whose value is no
/// integer of `0..=4294967295`, does not read back (`invalid_fact_snapshot_record`).
pub(crate) fn diagnose_rows(facts: &RuntimeFactSnapshot) -> Result<usize, Outcome> {
    let missing = || Outcome::new(ActingctlCode::ConfigFactsMissing);
    let unreadable = || Outcome::new(SanitizationCode::InvalidFactSnapshotRecord);
    let key = config_parameters_fact_key(config_parameter_section(DIAGNOSE_ROWS_KEY));
    let record = facts
        .records
        .iter()
        .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == key)
        .ok_or_else(missing)?;
    let FactValue::RecordList(rows) = &record.value else {
        return Err(unreadable());
    };
    let row = rows
        .iter()
        .find(|row| {
            matches!(row.get("key"), Some(FactScalar::String(name)) if name == DIAGNOSE_ROWS_KEY)
        })
        .ok_or_else(missing)?;
    match row.get("value") {
        Some(FactScalar::Integer(value)) => u32::try_from(*value)
            .ok()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(unreadable),
        _ => Err(unreadable()),
    }
}
