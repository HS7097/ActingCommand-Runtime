// SPDX-License-Identifier: AGPL-3.0-only

//! The key table (Workflow #387 model v1.2 §5.5): one row per configuration key, with where it
//! lives in the configuration file, its type and range, where its default comes from, whether an
//! instruction may change it while the Runtime runs, which slice reads it and since when. D1b
//! fixes the format; each section adds its rows in its own file (§5.1).

use crate::{existing, host_claims, instance, recovery_ladder, standard_packs};

/// The type of a key's value (§5.5 `type`). A decimal, a list and an object are reported in
/// `config.parameters` as canonical JSON text, since a fact scalar has no such type (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Boolean,
    Integer,
    DurationMs,
    String,
    Decimal,
    List,
    Object,
}

/// The values a key allows (§5.5 `range`): schema, not a threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRange {
    /// Any value of the key's type.
    Any,
    /// An integer or millisecond value within `min..=max`.
    Integer { min: i64, max: i64 },
    /// One of these words.
    Tokens(&'static [&'static str]),
}

/// Where a key's value comes from when the configuration file does not name it (§5.5
/// `default`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSource {
    /// The defaults file `actingd.defaults.json` holds it.
    File,
    /// None: the key is required, or a key the file does not name is not in effect.
    None,
    /// The effective value of the section-level key named here.
    Section(&'static str),
    /// A value still defined in code: a key that existed before the defaults file and whose
    /// default no slice has moved yet (§1.3).
    Code,
}

/// Why an instruction may not change a key while the Runtime runs (§5.5 `why_not`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSettable {
    /// The value is used before the ledger opens, where no instruction value is known yet.
    BeforeLedgerOpen,
    /// The start-up assembly reads the value once.
    ReadAtAssembly,
    /// The configuration file has no member for the key; the value is a code constant.
    NoFileMember,
    /// The value is derived from the configuration, not set by it.
    Derived,
}

/// Whether an instruction may change a key while the Runtime runs (§5.5 `settable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settable {
    Yes,
    No(NotSettable),
}

impl Settable {
    pub const fn is_settable(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// Where a key lives in the configuration file (§5.5 `file_path`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePath {
    /// At the key itself, its dots naming nested members.
    Key,
    /// At another dotted path: an older key whose manifest name differs from its file member.
    At(&'static str),
    /// In the `instances[]` entry whose `instance_id` is the key's instance, under the key's
    /// member name.
    InstanceMember,
    /// Nowhere: the file has no member for the key.
    Absent,
}

/// A key's name (§5.5 `key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyName {
    /// One key.
    Exact(&'static str),
    /// One key per configured instance, `instance.<instance id>.<member>`.
    PerInstance(&'static str),
}

/// One row of the key table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRow {
    pub name: KeyName,
    pub file_path: FilePath,
    pub value_type: ValueType,
    pub range: KeyRange,
    pub default: DefaultSource,
    pub settable: Settable,
    /// The slice whose code reads the key.
    pub reader: &'static str,
    /// The Runtime release from which the key has this row.
    pub since: &'static str,
}

/// The first segment of every per-instance key: the section of the per-instance rows.
pub const INSTANCE_SECTION: &str = "instance";

/// The per-instance key of `member` for the instance whose registry id is `instance_id`.
pub fn instance_key(instance_id: &str, member: &str) -> String {
    format!("{INSTANCE_SECTION}.{instance_id}.{member}")
}

impl KeyRow {
    /// Whether `key` is this row's key: `Some(None)` for an exact key, `Some(Some(id))` for a
    /// per-instance key with the instance id it names, `None` otherwise.
    pub fn matches<'k>(&self, key: &'k str) -> Option<Option<&'k str>> {
        match self.name {
            KeyName::Exact(name) => (name == key).then_some(None),
            KeyName::PerInstance(member) => {
                let rest = key.strip_prefix(INSTANCE_SECTION)?.strip_prefix('.')?;
                let (instance, tail) = rest.split_once('.')?;
                (!instance.is_empty() && tail == member).then_some(Some(instance))
            }
        }
    }

    /// This row's key, for `instance` when the row is per instance; `None` when the two do not
    /// fit (an exact row with an instance, a per-instance row without one).
    pub fn key_for(&self, instance: Option<&str>) -> Option<String> {
        match (self.name, instance) {
            (KeyName::Exact(name), None) => Some(name.to_owned()),
            (KeyName::PerInstance(member), Some(instance)) => Some(instance_key(instance, member)),
            _ => None,
        }
    }
}

/// The rows of the keys the start-up assembly of actingd parses and reports itself, unchanged
/// since before D1b (§1.3): the configuration manifest carries their values.
pub fn assembly_rows() -> &'static [KeyRow] {
    existing::ROWS
}

/// The rows of the sections this crate resolves and renders (§5.1), one file each: their file
/// layer, defaults file and section defaults. The section-level keys come before the
/// per-instance section whose defaults they are.
fn mechanism_sections() -> [&'static [KeyRow]; 4] {
    [
        recovery_ladder::ROWS,
        host_claims::ROWS,
        standard_packs::ROWS,
        instance::ROWS,
    ]
}

/// The rows whose values this crate resolves and renders, in manifest order (§5.1): the
/// `recovery_ladder`, `host_claims` and `standard_packs` sections (D1c), then the per-instance
/// section (D1b).
pub fn mechanism_rows() -> Vec<KeyRow> {
    mechanism_sections()
        .into_iter()
        .flatten()
        .copied()
        .collect()
}

/// Every row of the key table.
pub fn rows() -> impl Iterator<Item = &'static KeyRow> {
    assembly_rows()
        .iter()
        .chain(mechanism_sections().into_iter().flatten())
}

/// The row of `key` and, for a per-instance key, the instance id it names.
pub fn find_row(key: &str) -> Option<(&'static KeyRow, Option<&str>)> {
    rows().find_map(|row| row.matches(key).map(|instance| (row, instance)))
}
