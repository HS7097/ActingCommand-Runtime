// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline data structures for declarative task-flow contracts.

use crate::types::*;
use crate::{SchedulingEffectCondition, SchedulingOutcomeDeclaration};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Operation 0.8 post-admission fields. Values and their meaning belong to the package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldsDeclaration {
    pub mode: OcrFieldsMode,
    pub page_ids: Vec<String>,
    pub fields: Vec<OcrFieldDeclaration>,
    pub limits: OcrFieldsLimits,
    pub outcome_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldsMode {
    FieldsV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldsLimits {
    pub max_frames: u32,
    pub max_items: u32,
    pub max_string_bytes: u32,
    pub max_total_bytes: u64,
    pub max_truth_entries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDeclaration {
    pub id: String,
    pub group: String,
    pub target_id: String,
    pub required: bool,
    pub privacy: OcrFieldPrivacy,
    pub trim: OcrFieldTrim,
    pub value: OcrFieldType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldPrivacy {
    Public,
    Personal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldTrim {
    WhitespaceV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OcrFieldType {
    UnsignedInteger {
        min: u64,
        max: u64,
    },
    DictionaryEntry {
        dictionary: OcrFieldDictionaryReference,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionaryReference {
    pub path: String,
    pub sha256: String,
}

impl OcrFieldsDeclaration {
    /// A task without input can only collect fields on its declared terminal pages.
    pub fn validate_zero_input_task(
        &self,
        game: &str,
        execution_mode: &str,
        stop_on_confirmation: bool,
        target_pages: &[String],
        scheduling: &SchedulingOutcomeDeclaration,
    ) -> Result<(), &'static str> {
        self.validate()?;
        scheduling
            .validate()
            .map_err(|_| "ocr_fields_zero_input_outcome_invalid")?;
        let [mapping] = scheduling.mappings() else {
            return Err("ocr_fields_zero_input_outcome_invalid");
        };
        if execution_mode != "navigable_route"
            || !stop_on_confirmation
            || scheduling.designated_operation().is_some()
            || mapping.outcome_key() != self.outcome_key
            || mapping.effect() != SchedulingEffectCondition::NoDesignatedEffect
        {
            return Err("ocr_fields_zero_input_outcome_invalid");
        }
        let prefix = format!("{game}/");
        let canonical = |pages: &[String]| {
            pages
                .iter()
                .map(|page| page.strip_prefix(&prefix).unwrap_or(page).to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let targets = canonical(target_pages);
        let fields = canonical(&self.page_ids);
        let terminals = canonical(mapping.terminal_pages());
        if targets.is_empty()
            || targets.iter().any(|page| page.is_empty() || page == "any")
            || targets.len() != target_pages.len()
            || fields.len() != self.page_ids.len()
            || terminals.len() != mapping.terminal_pages().len()
            || targets != fields
            || targets != terminals
        {
            return Err("ocr_fields_zero_input_pages_invalid");
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-/.:".contains(&c))
        };
        let mut pages = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut targets = std::collections::BTreeSet::new();
        let limits = &self.limits;
        if self.page_ids.is_empty()
            || self.page_ids.len() > 2
            || self
                .page_ids
                .iter()
                .any(|page| !identifier(page) || !pages.insert(page))
            || self.fields.is_empty()
            || self.fields.len() > 32
            || self.outcome_key != "fields_recorded"
            || !(1..=256).contains(&limits.max_frames)
            || !(1..=4096).contains(&limits.max_items)
            || !(1..=4096).contains(&limits.max_string_bytes)
            || !(1..=4 * 1024 * 1024).contains(&limits.max_total_bytes)
            || !(1..=4096).contains(&limits.max_truth_entries)
        {
            return Err("ocr_fields_declaration_invalid");
        }
        for field in &self.fields {
            if !identifier(&field.id)
                || !identifier(&field.group)
                || !identifier(&field.target_id)
                || !ids.insert(&field.id)
                || !targets.insert(&field.target_id)
            {
                return Err("ocr_fields_binding_invalid");
            }
            match &field.value {
                OcrFieldType::UnsignedInteger { min, max } if min > max => {
                    return Err("ocr_fields_range_invalid");
                }
                OcrFieldType::DictionaryEntry { dictionary }
                    if dictionary.path.is_empty()
                        || dictionary.path.len() > 256
                        || dictionary.path.contains(['\\', ':'])
                        || dictionary
                            .path
                            .split('/')
                            .any(|p| p.is_empty() || p == "." || p == "..")
                        || dictionary.sha256.len() != 64
                        || !dictionary
                            .sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
                {
                    return Err("ocr_fields_dictionary_reference_invalid");
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The same hash-bound truth-set format used by collection OCR. Mapping is exact after
/// trim/lowercase normalization; canonical output retains the package's spelling.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionary {
    pub schema_version: String,
    pub items: Vec<String>,
    #[serde(default)]
    pub aliases: Option<Vec<OcrFieldDictionaryAlias>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionaryAlias {
    pub observed: String,
    pub canonical: String,
}

impl OcrFieldDictionary {
    pub fn validate(&self, limits: &OcrFieldsLimits) -> Result<(), &'static str> {
        if !matches!(
            self.schema_version.as_str(),
            "actingcommand.ocr-truth-set.v2"
        ) && !(self.schema_version == "actingcommand.ocr-truth-set.v1" && self.aliases.is_none())
        {
            return Err("ocr_fields_dictionary_schema_invalid");
        }
        if self.items.is_empty()
            || self.items.len() > limits.max_truth_entries as usize
            || self.aliases.as_ref().is_some_and(|v| v.len() > 1024)
        {
            return Err("ocr_fields_dictionary_limit_exceeded");
        }
        let valid = |v: &str| {
            !v.trim().is_empty()
                && v.len() <= limits.max_string_bytes as usize
                && v.trim().to_lowercase().len() <= limits.max_string_bytes as usize
        };
        let mut canonical = std::collections::BTreeSet::new();
        for item in &self.items {
            if !valid(item) || !canonical.insert(item.trim().to_lowercase()) {
                return Err("ocr_fields_dictionary_ambiguous");
            }
        }
        let mut aliases = std::collections::BTreeSet::new();
        for alias in self.aliases.iter().flatten() {
            let observed = alias.observed.trim().to_lowercase();
            if !valid(&alias.observed)
                || !valid(&alias.canonical)
                || canonical.contains(&observed)
                || !aliases.insert(observed)
                || !self.items.contains(&alias.canonical)
            {
                return Err("ocr_fields_dictionary_ambiguous");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OcrFieldValue {
    UnsignedInteger(u64),
    DictionaryEntry(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldReason {
    Resolved,
    Empty,
    InvalidInteger,
    Overflow,
    OutOfRange,
    UnknownEntry,
    AmbiguousEntry,
    LimitExceeded,
    ProviderFailed,
    RegionUnresolved,
    NotCollected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRegionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRegionOffset {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrAnchorMatch {
    pub rect: OcrRegionRect,
    pub raw_score: f32,
    pub score: f32,
    pub threshold: f32,
    pub passed: bool,
}

// Recognition and transport admission require finite scores.
impl Eq for OcrAnchorMatch {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrRegionUnresolvedReason {
    AnchorNotMatched,
    CoordinateOverflow,
    OutOfFrame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRegionEvidence {
    pub frame_width: u32,
    pub frame_height: u32,
    pub anchor_target_id: Option<String>,
    pub anchor_match: Option<OcrAnchorMatch>,
    pub offset: Option<OcrRegionOffset>,
    pub width: i32,
    pub height: i32,
    pub roi: Option<OcrRegionRect>,
    pub unresolved: Option<OcrRegionUnresolvedReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldResult {
    pub field_id: String,
    pub target_id: String,
    pub raw_text: Option<String>,
    pub normalized_text: Option<String>,
    pub value: Option<OcrFieldValue>,
    pub reason: OcrFieldReason,
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<OcrRegionEvidence>,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldRecord {
    pub frame_index: u32,
    pub page_id: String,
    pub group: String,
    pub fields: Vec<OcrFieldResult>,
}

pub const OCR_FIELDS_REPORT_SCHEMA: &str = "actingcommand.runtime.post-admission-ocr-fields.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldsReport {
    pub schema_version: String,
    pub declaration: OcrFieldsDeclaration,
    pub frames_collected: u32,
    pub items_collected: u32,
    pub total_observed_utf8_bytes: u64,
    pub records: Vec<OcrFieldRecord>,
    pub failure: Option<OcrFieldReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskFlow {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub game: GameKey,
    pub servers: Vec<ServerKey>,
    pub resolutions: Vec<Resolution>,
    pub entrypoint: String,
    pub tasks: Vec<TaskDefinition>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDefinition {
    pub id: TaskId,
    pub name: String,
    pub steps: Vec<TaskStep>,
    pub on_failure: FailurePolicy,
    pub produces: Vec<String>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStep {
    pub id: String,
    pub description: Option<String>,
    pub primitive: String,
    pub params: BTreeMap<String, TaskParamValue>,
    pub when: Option<String>,
    pub next: Option<String>,
    pub on_failure: Option<FailurePolicy>,
    pub timeout_ms: Option<DurationMillis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailurePolicy {
    pub severity: Severity,
    pub retry_limit: Option<i32>,
    pub retry_delay_ms: Option<DurationMillis>,
    pub fallback_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TaskParamValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<TaskParamValue>),
    Object(BTreeMap<String, TaskParamValue>),
}
