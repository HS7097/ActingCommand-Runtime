// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #357 DB-2 (model M-357a v1.1 §2.3, §3.4; register §1.65 TB5, the rest of the family
//! `standard_pack_tables`): a selection document against the tables of its repository, before a
//! package ships. The binding is the Runtime's own `SelectionTables::bind`, so K2 has one answer;
//! the coverage check lists the identities of the document's layout that a table has no row for.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use actingcommand_contract::{StandardPackTableIssue, parse_standard_pack_table};
use actingcommand_selection_policy::{
    LoadedTable, SelectionPolicy, SelectionTables, TableBindRefusal, ValueRef,
};
use serde_json::Value;

/// Why a document could not be measured against the tables of its repository.
#[derive(Debug)]
pub enum PolicyTablesRefusal {
    /// The bytes are no selection document.
    Document(serde_json::Error),
    /// The table file `name` is refused.
    Table {
        name: String,
        issue: StandardPackTableIssue,
    },
    /// K2: a table differs from the document's declaration.
    Binding(TableBindRefusal),
}

impl fmt::Display for PolicyTablesRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(error) => write!(formatter, "{error}"),
            Self::Table { name, issue } => write!(formatter, "{name}: {issue}"),
            Self::Binding(refusal) => write!(formatter, "{refusal}"),
        }
    }
}

/// A table of a document with identities of its layout that no row holds: a warning, or an error
/// when the document reads the table under `abort_evaluation` (`aborts`), because then one such
/// candidate ends every evaluation and a scan cannot stop early (§3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCoverageGap {
    pub table_id: String,
    /// How many identities of the layout's catalog no row holds.
    pub uncovered: usize,
    pub aborts: bool,
}

/// The table ids `policy_bytes` declares; none for bytes that are no selection document, which
/// the `selection_policy` family refuses on its own.
pub fn declared_policy_tables(policy_bytes: &[u8]) -> Vec<String> {
    serde_json::from_slice::<SelectionPolicy>(policy_bytes).map_or_else(
        |_| Vec::new(),
        |policy| {
            policy
                .tables
                .into_iter()
                .map(|declaration| declaration.table_id)
                .collect()
        },
    )
}

/// Binds `tables` (each `(file name, bytes)`, for `tables/<file name>`) to the document and
/// measures their coverage of the identities its layout declares in `task` (the task document
/// that declares the select step). A refused table or a binding refusal is the `Err`.
pub fn check_policy_tables(
    policy_bytes: &[u8],
    task: &Value,
    tables: &[(String, Vec<u8>)],
) -> Result<Vec<TableCoverageGap>, PolicyTablesRefusal> {
    let policy: SelectionPolicy =
        serde_json::from_slice(policy_bytes).map_err(PolicyTablesRefusal::Document)?;
    let mut loaded = Vec::with_capacity(tables.len());
    for (name, bytes) in tables {
        let table = parse_standard_pack_table(bytes, None, name).map_err(|issue| {
            PolicyTablesRefusal::Table {
                name: name.clone(),
                issue,
            }
        })?;
        // The instant 0 expires no column: the coverage check reads keys, not cells.
        loaded.push(Arc::new(LoadedTable::load(table, 0)));
    }
    SelectionTables::bind(&policy, &loaded).map_err(PolicyTablesRefusal::Binding)?;
    let mut gaps = Vec::new();
    for table in &loaded {
        let table_id = table.table_id();
        let keys = table.keys().collect::<BTreeSet<_>>();
        let uncovered = identities_read_from(&policy, table_id, task)
            .iter()
            .filter(|identity| !keys.contains(identity.as_str()))
            .count();
        if uncovered > 0 {
            gaps.push(TableCoverageGap {
                table_id: table_id.to_owned(),
                uncovered,
                aborts: policy.table_read_aborts(table_id),
            });
        }
    }
    Ok(gaps)
}

/// The identities of the layout's features that the document's reads of `table_id` key by.
fn identities_read_from(
    policy: &SelectionPolicy,
    table_id: &str,
    task: &Value,
) -> BTreeSet<String> {
    let key_fields = policy
        .gates
        .iter()
        .flat_map(|gate| predicate_values(&gate.predicate))
        .chain(policy.scoring.iter().map(|term| &term.value))
        .filter_map(|value| match value {
            ValueRef::Table {
                table_id: read,
                key_field,
                ..
            } if read == table_id => Some(key_field.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let layouts = task.get("candidate_layouts").and_then(Value::as_array);
    let Some(layout) = layouts.into_iter().flatten().find(|layout| {
        layout.get("id").and_then(Value::as_str)
            == Some(policy.applies_to.candidate_layout_id.as_str())
    }) else {
        return BTreeSet::new();
    };
    layout
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|feature| {
            feature
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| key_fields.contains(name))
        })
        .filter_map(|feature| {
            feature
                .pointer("/identity/entries")
                .and_then(Value::as_array)
        })
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn predicate_values(predicate: &actingcommand_selection_policy::Predicate) -> Vec<&ValueRef> {
    use actingcommand_selection_policy::Predicate;
    match predicate {
        Predicate::IntegerAtLeast { value, .. }
        | Predicate::IntegerAtMost { value, .. }
        | Predicate::IntegerEquals { value, .. }
        | Predicate::BooleanEquals { value, .. }
        | Predicate::StringIn { value, .. } => vec![value],
        Predicate::IntegerCompare { left, right, .. } => vec![left, right],
        Predicate::All { of } | Predicate::Any { of } => {
            of.iter().flat_map(predicate_values).collect()
        }
        Predicate::Not { of } => predicate_values(of),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table(rows: Value) -> (String, Vec<u8>) {
        let value = json!({
            "schema_version": "actingcommand.table.v1",
            "game": "neutral",
            "table_id": "unit_scores",
            "row_key": ["unit"],
            "columns": [
                {"name": "unit", "type": "key"},
                {"name": "score_milli", "type": "integer", "min": 0, "max": 1000, "nullable": true}
            ],
            "rows": rows,
        });
        (
            "unit_scores.json".to_owned(),
            serde_json::to_vec(&value).expect("table bytes"),
        )
    }

    fn document(on_unknown: &str, column_type: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.selection-policy.v1",
            "policy_id": "by-table",
            "applies_to": {"candidate_layout_id": "layout-a", "outcome_keys": {
                "selected": "s", "empty": "e", "insufficient": "i", "ambiguous": "a", "unknown": "u"
            }},
            "fields": [{"name": "member", "value_type":
                {"type": "enum_string", "allowed": ["unit.a", "unit.b"]}}],
            "facts": [],
            "tables": [{"table_id": "unit_scores", "path": "tables/unit_scores.json",
                        "row_key": ["unit"], "columns": {"score_milli": column_type}}],
            "gates": [],
            "scoring": [{"term_id": "score",
                         "value": {"source": "table", "table_id": "unit_scores",
                                   "key_field": "member", "pins": [],
                                   "value_column": "score_milli"},
                         "transform": {"kind": "identity"}, "weight_milli": 1000,
                         "on_unknown": {"kind": on_unknown}}],
            "selection": {"mode": "exactly_one", "required_count": 1},
            "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
        }))
        .expect("policy bytes")
    }

    fn task() -> Value {
        json!({"candidate_layouts": [{"id": "layout-a", "features": [
            {"name": "member", "identity": {"entries": [{"id": "unit.a"}, {"id": "unit.b"}]}}
        ]}]})
    }

    /// TB5: K2 through the one binding; an identity with no row is a gap, an error only under
    /// `abort_evaluation`; a table that covers every identity is no gap.
    #[test]
    fn db2_a_document_is_bound_and_its_identity_coverage_measured() {
        let integer = json!({"type": "integer"});
        let declared = document("drop_candidate", integer.clone());
        assert_eq!(declared_policy_tables(&declared), vec!["unit_scores"]);
        let covered = table(json!([["unit.a", 1], ["unit.b", null]]));
        assert_eq!(
            check_policy_tables(&declared, &task(), &[covered]).expect("bound"),
            Vec::new()
        );
        let partial = table(json!([["unit.a", 1]]));
        assert_eq!(
            check_policy_tables(&declared, &task(), std::slice::from_ref(&partial)).expect("bound"),
            vec![TableCoverageGap {
                table_id: "unit_scores".to_owned(),
                uncovered: 1,
                aborts: false,
            }]
        );
        let aborting = document("abort_evaluation", integer);
        assert!(
            check_policy_tables(&aborting, &task(), std::slice::from_ref(&partial)).expect("bound")
                [0]
            .aborts
        );
        let mistyped = document("drop_candidate", json!({"type": "boolean"}));
        let refusal = check_policy_tables(&mistyped, &task(), &[partial])
            .expect_err("column type")
            .to_string();
        assert!(refusal.contains("table_mismatch=column_type"), "{refusal}");
    }
}
