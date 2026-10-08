// SPDX-License-Identifier: AGPL-3.0-only

//! Debug and test builds check every emitted outcome against the embedded catalog: required
//! keys present, only declared keys, records fields, the category, and that a fatal link sits
//! only under a fatal top. Received outcomes are relayed unchanged and not checked as emissions;
//! a code the catalog does not list (a test registry) skips the key checks.

use super::{Category, CodeStr, KeyKind, Outcome, key_kind};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct CatalogDocument {
    codes: HashMap<String, CatalogCode>,
}

#[derive(Deserialize)]
struct CatalogCode {
    category: Category,
    #[serde(default)]
    values: HashMap<String, Presence>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Presence {
    Required,
    Optional,
}

/// The codes of a catalog with their categories and keys.
pub(crate) struct CatalogIndex {
    codes: HashMap<String, CatalogCode>,
}

impl CatalogIndex {
    pub(crate) fn parse(text: &str) -> Result<Self, serde_json::Error> {
        let document: CatalogDocument = serde_json::from_str(text)?;
        Ok(Self {
            codes: document.codes,
        })
    }
}

/// One way an emitted outcome breaks its catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckFailure {
    CategoryMismatch {
        outcome: CodeStr,
    },
    RequiredKeyMissing {
        outcome: CodeStr,
        key: String,
    },
    KeyNotDeclared {
        outcome: CodeStr,
        key: String,
    },
    RecordFieldMissing {
        outcome: CodeStr,
        key: String,
        field: &'static str,
    },
    RecordFieldNotDeclared {
        outcome: CodeStr,
        key: String,
        field: String,
    },
    FatalLinkUnderNonFatalTop {
        top: CodeStr,
        link: CodeStr,
    },
}

impl fmt::Display for CheckFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CategoryMismatch { outcome } => {
                write!(formatter, "{outcome}: category differs from the catalog")
            }
            Self::RequiredKeyMissing { outcome, key } => {
                write!(formatter, "{outcome}: required key {key} is missing")
            }
            Self::KeyNotDeclared { outcome, key } => {
                write!(
                    formatter,
                    "{outcome}: key {key} is not declared for the code"
                )
            }
            Self::RecordFieldMissing {
                outcome,
                key,
                field,
            } => write!(formatter, "{outcome}: {key} record lacks field {field}"),
            Self::RecordFieldNotDeclared {
                outcome,
                key,
                field,
            } => write!(
                formatter,
                "{outcome}: {key} record has undeclared field {field}"
            ),
            Self::FatalLinkUnderNonFatalTop { top, link } => {
                write!(formatter, "{top}: fatal link {link} under a non-fatal top")
            }
        }
    }
}

fn joined(failures: &[CheckFailure]) -> String {
    failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

fn index() -> &'static CatalogIndex {
    static INDEX: OnceLock<CatalogIndex> = OnceLock::new();
    INDEX.get_or_init(|| {
        CatalogIndex::parse(super::catalog())
            .unwrap_or_else(|error| panic!("the embedded outcome catalog does not parse: {error}"))
    })
}

/// Checks a whole outcome as it leaves the builder: every emitted node, and the fatal-link
/// rule at the top.
pub(crate) fn debug_assert_well_formed(outcome: &Outcome) {
    if cfg!(debug_assertions) {
        let failures = failures(outcome, index(), true);
        if !failures.is_empty() {
            panic!(
                "outcome {outcome} breaks the outcome catalog: {}",
                joined(&failures)
            );
        }
    }
}

/// Checks a link as it is attached. Its own top may still go under a fatal top, so the
/// fatal-link rule waits for the whole outcome.
pub(crate) fn debug_assert_link(link: &Outcome) {
    if cfg!(debug_assertions) {
        let failures = failures(link, index(), false);
        if !failures.is_empty() {
            panic!(
                "link {link} breaks the outcome catalog: {}",
                joined(&failures)
            );
        }
    }
}

pub(crate) fn failures(outcome: &Outcome, index: &CatalogIndex, top: bool) -> Vec<CheckFailure> {
    let mut failures = Vec::new();
    if top && outcome.category != Category::Fatal {
        for link in &outcome.causes {
            if let Some(fatal) = first_fatal(&link.outcome) {
                failures.push(CheckFailure::FatalLinkUnderNonFatalTop {
                    top: outcome.code.clone(),
                    link: fatal.clone(),
                });
            }
        }
    }
    check_node(outcome, index, &mut failures);
    failures
}

fn first_fatal(outcome: &Outcome) -> Option<&CodeStr> {
    if outcome.category == Category::Fatal {
        return Some(&outcome.code);
    }
    outcome
        .causes
        .iter()
        .find_map(|link| first_fatal(&link.outcome))
}

fn check_node(outcome: &Outcome, index: &CatalogIndex, failures: &mut Vec<CheckFailure>) {
    if outcome.received {
        return;
    }
    if let Some(entry) = index.codes.get(outcome.code.as_str()) {
        if entry.category != outcome.category {
            failures.push(CheckFailure::CategoryMismatch {
                outcome: outcome.code.clone(),
            });
        }
        for (key, presence) in &entry.values {
            if *presence == Presence::Required && !outcome.values.contains_key(key) {
                failures.push(CheckFailure::RequiredKeyMissing {
                    outcome: outcome.code.clone(),
                    key: key.clone(),
                });
            }
        }
        for (key, value) in &outcome.values {
            if !declared(key, &entry.values) {
                failures.push(CheckFailure::KeyNotDeclared {
                    outcome: outcome.code.clone(),
                    key: key.clone(),
                });
            }
            if let Some(KeyKind::Records(fields)) = key_kind(key) {
                check_records(outcome, key, fields, value, failures);
            }
        }
    }
    for link in &outcome.causes {
        check_node(&link.outcome, index, failures);
    }
}

/// A key the entry lists, or the `<key>_total` of a listed list or records key.
fn declared(key: &str, values: &HashMap<String, Presence>) -> bool {
    if values.contains_key(key) {
        return true;
    }
    key.strip_suffix("_total").is_some_and(|base| {
        values.contains_key(base)
            && matches!(key_kind(base), Some(KeyKind::List(_) | KeyKind::Records(_)))
    })
}

fn check_records(
    outcome: &Outcome,
    key: &str,
    fields: &'static [(&'static str, bool)],
    value: &JsonValue,
    failures: &mut Vec<CheckFailure>,
) {
    let Some(records) = value.as_array() else {
        return;
    };
    for record in records.iter().filter_map(JsonValue::as_object) {
        for (field, required) in fields {
            if *required && !record.contains_key(*field) {
                failures.push(CheckFailure::RecordFieldMissing {
                    outcome: outcome.code.clone(),
                    key: key.to_owned(),
                    field: *field,
                });
            }
        }
        for field in record.keys() {
            if !fields.iter().any(|(name, _)| *name == field.as_str()) {
                failures.push(CheckFailure::RecordFieldNotDeclared {
                    outcome: outcome.code.clone(),
                    key: key.to_owned(),
                    field: field.clone(),
                });
            }
        }
    }
}
