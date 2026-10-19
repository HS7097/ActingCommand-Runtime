// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 B15, slice R6 (model-b15 §5.1; `contracts/instance-fact-store.md`, "Remedy
//! choice facts"): the write rule of the instance fact `session.remedy.choices`. A publication of
//! the key is checked whole before the fact write gate, and a refusal is the non-fatal request
//! error `session_remedy_choices_invalid` with `fact_key`, `field` (a JSON pointer into the row
//! list: `/<row>/<member>`, `/<row>` for a row as a whole, `""` for the list as a whole, or
//! `/invalidate_on` for the record's invalidating events) and, for a meaning error, the
//! `instance_id` it was found on; nothing is appended.
//!
//! - **Shape:** an inline `record_list` of at most 256 rows; `row` is `allow`, `none` or
//!   `parameter` with exactly its fields, each a string token `^[a-z][a-z0-9_]{0,63}$`, `none`
//!   never a capability; no row twice, no `none` with an `allow` of the same need, no two
//!   `parameter` rows of one capability and parameter; no `invalidate_on` events. Every scope
//!   (instance, server, game) is one a record may have.
//! - **Meaning:** against the remedy table of the active catalog's bound packages
//!   ([`crate::remedy::build_table`], read at the write), on every instance the scope covers: an
//!   `allow` whose need no remedy bound there declares, or whose capability that need's remedies
//!   do not list; a `parameter` row whose capability no provider bound there declares, whose
//!   parameter that capability does not declare, or whose value is not one of its options.
//!
//! Needs, capabilities, parameters and options are opaque tokens compared for equality only.

use crate::codes::RemedyHostCode;
use crate::{ProcedureManifest, RuntimeHostError};
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::{
    FactContent, FactRecord, FactScalar, FactScope, FactValue, REMEDY_CHOICES_FACT_KEY,
    RuntimeErrorCode,
};
use actingcommand_policy::{
    CompiledCatalog, InstanceSnapshot, RemedyTable, ScopeSelector, TaskSpec,
};
use std::collections::BTreeSet;

const ROW: &str = "row";
const ALLOW: &str = "allow";
const NO_REMEDY: &str = "none";
const PARAMETER: &str = "parameter";
const NEED: &str = "need";
const CAPABILITY: &str = "capability";
const VALUE: &str = "value";
/// The capability name a row may never give.
const CAPABILITY_NEVER: &str = "none";
/// The `record_list` bound.
const MAX_ROWS: usize = 256;
const MAX_NAME_BYTES: usize = 64;
/// The key table's bound of a `pointer` value.
const MAX_POINTER_BYTES: usize = 512;
const INVALIDATE_ON_POINTER: &str = "/invalidate_on";

/// One row of the record, read by its kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Row {
    Allow {
        need: String,
        capability: String,
    },
    NoRemedy {
        need: String,
    },
    Parameter {
        capability: String,
        parameter: String,
        value: String,
    },
}

/// A refused write: where (`field`) and, for a meaning error, on which instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    field: String,
    instance_id: Option<String>,
}

impl Refusal {
    fn at(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            instance_id: None,
        }
    }

    /// `session_remedy_choices_invalid` (`fact_key`, `field`, `instance_id` when known), a
    /// non-fatal request error.
    pub(crate) fn into_error(self) -> RuntimeHostError {
        let Self { field, instance_id } = self;
        RuntimeHostError::request(
            RemedyHostCode::SessionRemedyChoicesInvalid,
            ContractLocation::PublishFacts,
            RuntimeErrorCode::InvalidRequest,
        )
        .with_outcome_values(move |outcome| {
            let outcome = outcome.fact_key(REMEDY_CHOICES_FACT_KEY).field(field);
            match instance_id {
                Some(instance_id) => outcome.instance_id(instance_id),
                None => outcome,
            }
        })
    }
}

/// An instance a record's scope covers: its alias and, when the policy inputs know it, its
/// server and game.
struct Covered {
    instance_id: String,
    server_id: Option<String>,
    game_id: Option<String>,
}

/// The write rule of one `session.remedy.choices` record: its shape, then, when a row names a
/// need or a parameter, its names against the remedy table of `catalog` (the active catalog,
/// `None` when none is active: no package is bound) read through `manifest` at `now_unix_ms`, on
/// every instance of `instances` (the policy inputs) its scope covers. An instance scope covers
/// its instance even when the policy inputs do not name it; it then has no bound package.
pub(crate) fn check(
    record: &FactRecord,
    catalog: Option<&CompiledCatalog>,
    manifest: Option<&ProcedureManifest>,
    instances: &[InstanceSnapshot],
    now_unix_ms: u64,
) -> Result<(), Refusal> {
    let rows = shape(record)?;
    if rows.iter().all(|row| matches!(row, Row::NoRemedy { .. })) {
        return Ok(());
    }
    let covered = covered(&record.scope, instances)
        .into_iter()
        .take(0)
        .collect::<Vec<_>>();
    match catalog {
        Some(catalog) => {
            let built = crate::remedy::build_table(catalog, manifest, instances, now_unix_ms);
            meaning(
                &rows,
                &built.table,
                &catalog.catalog().tasks.tasks,
                &covered,
            )
        }
        None => meaning(
            &rows,
            &RemedyTable {
                remedies: Vec::new(),
                providers: Vec::new(),
                refused_edges: Vec::new(),
            },
            &[],
            &covered,
        ),
    }
}

/// The record's rows, or the first shape rule it breaks.
fn shape(record: &FactRecord) -> Result<Vec<Row>, Refusal> {
    if !record.invalidate_on.is_empty() {
        return Err(Refusal::at(INVALIDATE_ON_POINTER));
    }
    let FactContent::Inline {
        value: FactValue::RecordList(items),
    } = &record.content
    else {
        return Err(Refusal::at(""));
    };
    if items.len() > MAX_ROWS {
        return Err(Refusal::at(""));
    }
    let mut rows = Vec::with_capacity(items.len());
    let mut seen = BTreeSet::new();
    let mut allowed = BTreeSet::new();
    let mut declined = BTreeSet::new();
    let mut parameters = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let at = |member: &str| Refusal::at(pointer(index, Some(member)));
        let Some(FactScalar::String(kind)) = item.get(ROW) else {
            return Err(at(ROW));
        };
        let fields: &[&str] = match kind.as_str() {
            ALLOW => &[NEED, CAPABILITY],
            NO_REMEDY => &[NEED],
            PARAMETER => &[CAPABILITY, PARAMETER, VALUE],
            _ => return Err(at(ROW)),
        };
        if let Some(other) = item
            .keys()
            .find(|member| member.as_str() != ROW && !fields.contains(&member.as_str()))
        {
            return Err(at(other.as_str()));
        }
        let mut names = Vec::with_capacity(fields.len());
        for &field in fields {
            let Some(FactScalar::String(name)) = item.get(field) else {
                return Err(at(field));
            };
            if !is_name(name) || (field == CAPABILITY && name == CAPABILITY_NEVER) {
                return Err(at(field));
            }
            names.push(name.clone());
        }
        let row = match names.as_slice() {
            [need, capability] => Row::Allow {
                need: need.clone(),
                capability: capability.clone(),
            },
            [need] => Row::NoRemedy { need: need.clone() },
            [capability, parameter, value] => Row::Parameter {
                capability: capability.clone(),
                parameter: parameter.clone(),
                value: value.clone(),
            },
            _ => return Err(at(ROW)),
        };
        let conflict = match &row {
            Row::Allow { need, .. } => {
                allowed.insert(need.clone());
                false
            }
            Row::NoRemedy { need } => {
                declined.insert(need.clone());
                false
            }
            // Two values for one parameter: nothing says which holds (the same value again is a
            // row twice).
            Row::Parameter {
                capability,
                parameter,
                ..
            } => !parameters.insert((capability.clone(), parameter.clone())),
        };
        if conflict || !seen.insert(row.clone()) {
            return Err(Refusal::at(pointer(index, None)));
        }
        rows.push(row);
    }
    Ok(rows)
}

/// The first row that names a need, capability, parameter or option the packages bound on a
/// covered instance do not declare.
fn meaning(
    rows: &[Row],
    table: &RemedyTable,
    tasks: &[TaskSpec],
    covered: &[Covered],
) -> Result<(), Refusal> {
    for instance in covered {
        let bound = |task_id: &str| {
            tasks
                .iter()
                .find(|task| task.id == task_id)
                .is_some_and(|task| covers(&task.scope, instance))
        };
        let remedies = table
            .remedies
            .iter()
            .filter(|remedy| bound(&remedy.task_id))
            .collect::<Vec<_>>();
        let provided = table
            .providers
            .iter()
            .filter(|provider| bound(&provider.task_id))
            .flat_map(|provider| provider.provides.iter())
            .collect::<Vec<_>>();
        for (index, row) in rows.iter().enumerate() {
            let refuse = |member: &str| Refusal {
                field: pointer(index, Some(member)),
                instance_id: Some(instance.instance_id.clone()),
            };
            match row {
                Row::Allow { need, capability } => {
                    let of_need = remedies
                        .iter()
                        .filter(|remedy| remedy.need == *need)
                        .collect::<Vec<_>>();
                    if of_need.is_empty() {
                        return Err(refuse(NEED));
                    }
                    if !of_need
                        .iter()
                        .any(|remedy| remedy.capabilities.contains(capability))
                    {
                        return Err(refuse(CAPABILITY));
                    }
                }
                Row::NoRemedy { .. } => {}
                Row::Parameter {
                    capability,
                    parameter,
                    value,
                } => {
                    let declarations = provided
                        .iter()
                        .filter(|declaration| declaration.capability == *capability)
                        .collect::<Vec<_>>();
                    if declarations.is_empty() {
                        return Err(refuse(CAPABILITY));
                    }
                    let declared = declarations
                        .iter()
                        .flat_map(|declaration| declaration.parameters.iter())
                        .filter(|declared| declared.name == *parameter)
                        .collect::<Vec<_>>();
                    if declared.is_empty() {
                        return Err(refuse(PARAMETER));
                    }
                    if !declared
                        .iter()
                        .any(|declared| declared.options.contains(value))
                    {
                        return Err(refuse(VALUE));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The instances `scope` covers: an instance scope its instance (with the server and game the
/// policy inputs give it, if any), a server or game scope every instance of the policy inputs on
/// that server or game.
fn covered(scope: &FactScope, instances: &[InstanceSnapshot]) -> Vec<Covered> {
    let of = |instance: &InstanceSnapshot| Covered {
        instance_id: instance.instance_id.clone(),
        server_id: Some(instance.server_id.clone()),
        game_id: Some(instance.game_id.clone()),
    };
    match scope {
        FactScope::Instance { instance_id } => vec![
            instances
                .iter()
                .find(|instance| instance.instance_id == *instance_id)
                .map_or_else(
                    || Covered {
                        instance_id: instance_id.clone(),
                        server_id: None,
                        game_id: None,
                    },
                    of,
                ),
        ],
        FactScope::Server { server_id } => instances
            .iter()
            .filter(|instance| instance.server_id == *server_id)
            .map(of)
            .collect(),
        FactScope::Game { game_id } => instances
            .iter()
            .filter(|instance| instance.game_id == *game_id)
            .map(of)
            .collect(),
    }
}

/// Whether a task of `scope` covers `instance`.
fn covers(scope: &ScopeSelector, instance: &Covered) -> bool {
    match scope {
        ScopeSelector::Instance { instance_id } => *instance_id == instance.instance_id,
        ScopeSelector::Server { server_id } => instance.server_id.as_ref() == Some(server_id),
        ScopeSelector::Game { game_id } => instance.game_id.as_ref() == Some(game_id),
    }
}

/// A name of a row: `^[a-z][a-z0-9_]{0,63}$`.
fn is_name(name: &str) -> bool {
    name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// The JSON pointer of row `index` (and `member`, RFC 6901 escaped); the row's own pointer when
/// the member's would pass the key table's bound.
fn pointer(index: usize, member: Option<&str>) -> String {
    let row = format!("/{index}");
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
