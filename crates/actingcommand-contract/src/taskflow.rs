// SPDX-License-Identifier: AGPL-3.0-only

//! Rust mainline data structures for declarative task-flow contracts.

use crate::codes::{SharedCode, TaskflowCode};
use crate::types::*;
use crate::{SchedulingEffectCondition, SchedulingOutcomeDeclaration};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Explicit operation postcondition polling budget; independent of step timeout.
pub const MAX_POSTCONDITION_TIMEOUT_MS: u64 = MAX_CONTAINED_TASK_TIMEOUT_MS;
pub const MAX_CONTAINED_TASK_TIMEOUT_MS: u64 = 1_800_000;
pub const PHASED_CONTROL_SCHEMA: &str = "Lab-1y.control.v2";
pub const MAX_TASK_PHASES: usize = 32;

/// One linear stage inside the same task, run and lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPhase {
    pub id: String,
    pub operations: Vec<String>,
    pub target_pages: Vec<String>,
}

pub fn validate_task_phases(
    phases: &[TaskPhase],
    operation_ids: &[&str],
) -> Result<(), crate::outcome::Code> {
    let label = |value: &str| {
        !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
    };
    let mut ids = std::collections::BTreeSet::new();
    if phases.is_empty() || phases.len() > MAX_TASK_PHASES {
        return Err(TaskflowCode::TaskPhasesInvalid.code());
    }
    for phase in phases {
        if !label(&phase.id)
            || !ids.insert(&phase.id)
            || phase.operations.is_empty()
            || phase.operations.len() > operation_ids.len()
            || phase
                .operations
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != phase.operations.len()
            || phase
                .operations
                .iter()
                .any(|id| !operation_ids.contains(&id.as_str()))
            || phase.target_pages.is_empty()
            || phase.target_pages.len() > 256
            || phase
                .target_pages
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != phase.target_pages.len()
            || phase
                .target_pages
                .iter()
                .any(|page| !label(page) || page == "any")
        {
            return Err(TaskflowCode::TaskPhasesInvalid.code());
        }
    }
    Ok(())
}

/// Recorded on the existing step chain; completed means this step advances the cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPhaseEvidence {
    pub index: u32,
    pub id: String,
    pub completed: bool,
}

/// A view of already parsed operations for the common phase reachability check.
pub struct TaskPhaseOperation<'a> {
    pub id: &'a str,
    pub from: &'a str,
    pub destinations: Vec<String>,
    pub retryable: bool,
}

// --- Workflow #373 NAV-3b: entry page sets (begin) ---

/// The most pages one task entry page set names (Workflow #373 NAV-3b).
pub const MAX_ENTRY_PAGES: usize = 16;

crate::outcome_vocabulary! {
    /// Why a task's entry page set is refused at admission (vocabulary `entry_problem`,
    /// Workflow #373 NAV-3b).
    pub enum EntryProblem: "entry_problem" {
        Empty => "empty",
        TooMany => "too_many",
        Duplicate => "duplicate",
        AnyInSet => "any_in_set",
        Unresolved => "unresolved",
        PhaseStartMissing => "phase_start_missing",
        PhaseStartAmbiguous => "phase_start_ambiguous",
    }
}

/// One start of a phased route: the stage the route enters at and the page it enters on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskPhaseEntry<'a> {
    pub phase: usize,
    pub page: &'a str,
}

/// The shape of a task's entry page set before its pages are resolved (Workflow #373 NAV-3b):
/// 1..=16 labels, none of them `any` or blank, and no page named twice once the exact game
/// prefix is removed. Source parsing, build and Runtime admission share this check.
pub fn validate_entry_page_set(game: &str, pages: &[String]) -> Result<(), EntryProblem> {
    if pages.is_empty() {
        return Err(EntryProblem::Empty);
    }
    if pages.len() > MAX_ENTRY_PAGES {
        return Err(EntryProblem::TooMany);
    }
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    if pages.iter().any(|page| canonical(page) == "any") {
        return Err(EntryProblem::AnyInSet);
    }
    if pages.iter().any(|page| canonical(page).trim().is_empty()) {
        return Err(EntryProblem::Unresolved);
    }
    let mut seen = std::collections::BTreeSet::new();
    if !pages.iter().all(|page| seen.insert(canonical(page))) {
        return Err(EntryProblem::Duplicate);
    }
    Ok(())
}

/// The stage a phased route enters at for one page of its entry page set (Workflow #373
/// NAV-3b): the one stage that lists an operation whose `from` is that page. No such stage, or
/// more than one, refuses the set.
pub fn phase_entry_start(
    game: &str,
    page: &str,
    phases: &[TaskPhase],
    operations: &[TaskPhaseOperation<'_>],
) -> Result<usize, EntryProblem> {
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    let page = canonical(page);
    let mut starts = phases
        .iter()
        .enumerate()
        .filter(|(_, phase)| {
            operations.iter().any(|operation| {
                phase.operations.iter().any(|id| id == operation.id)
                    && operation.from != "any"
                    && canonical(operation.from) == page
            })
        })
        .map(|(index, _)| index);
    match (starts.next(), starts.next()) {
        (Some(index), None) => Ok(index),
        (None, _) => Err(EntryProblem::PhaseStartMissing),
        (Some(_), Some(_)) => Err(EntryProblem::PhaseStartAmbiguous),
    }
}

// --- Workflow #373 NAV-3b: entry page sets (end) ---

/// The static reachability check of a phased route. Workflow #373 NAV-3b: a route may enter at
/// several starts (an entry page set); the check runs once from each start, and each must
/// reach a mapped terminal.
pub fn validate_phased_route(
    game: &str,
    entries: &[TaskPhaseEntry<'_>],
    phases: &[TaskPhase],
    target_pages: &[String],
    operations: &[TaskPhaseOperation<'_>],
    scheduling: &SchedulingOutcomeDeclaration,
) -> Result<(), crate::outcome::Code> {
    validate_task_phases(
        phases,
        &operations
            .iter()
            .map(|operation| operation.id)
            .collect::<Vec<_>>(),
    )?;
    scheduling
        .validate()
        .map_err(|_| TaskflowCode::TaskPhasesOutcomeInvalid.code())?;
    let prefix = format!("{game}/");
    let canonical = |page: &str| page.strip_prefix(&prefix).unwrap_or(page).to_owned();
    let pages = |values: &[String]| {
        values
            .iter()
            .map(|page| canonical(page))
            .collect::<std::collections::BTreeSet<_>>()
    };
    if entries.is_empty()
        || entries.iter().any(|entry| {
            entry.page.trim().is_empty() || entry.page == "any" || entry.phase >= phases.len()
        })
        || pages(target_pages) != pages(&phases.last().unwrap().target_pages)
    {
        return Err(TaskflowCode::TaskPhasesInvalid.code());
    }
    let designated = scheduling.designated_operation();
    if designated.is_some_and(|id| {
        operations
            .iter()
            .filter(|operation| operation.id == id)
            .count()
            != 1
    }) || operations
        .iter()
        .any(|operation| designated == Some(operation.id) && operation.retryable)
    {
        return Err(TaskflowCode::TaskPhasesDesignatedEffectInvalid.code());
    }
    for entry in entries {
        validate_phased_route_from(
            entry, phases, operations, scheduling, designated, &canonical,
        )?;
    }
    Ok(())
}

/// Today's reachability check of a phased route, from one start.
fn validate_phased_route_from(
    entry: &TaskPhaseEntry<'_>,
    phases: &[TaskPhase],
    operations: &[TaskPhaseOperation<'_>],
    scheduling: &SchedulingOutcomeDeclaration,
    designated: Option<&str>,
    canonical: &dyn Fn(&str) -> String,
) -> Result<(), crate::outcome::Code> {
    let mut pending =
        std::collections::VecDeque::from([(entry.phase, canonical(entry.page), false)]);
    let mut visited = std::collections::BTreeSet::new();
    let mut terminals = 0;
    while let Some((index, page, effect)) = pending.pop_front() {
        if !visited.insert((index, page.clone(), effect)) {
            continue;
        }
        if index == phases.len() {
            let condition = if effect {
                SchedulingEffectCondition::DesignatedEffectCompleted
            } else {
                SchedulingEffectCondition::NoDesignatedEffect
            };
            if scheduling
                .mappings()
                .iter()
                .filter(|mapping| {
                    mapping.effect() == condition
                        && mapping
                            .terminal_pages()
                            .iter()
                            .any(|target| canonical(target) == page)
                })
                .count()
                != 1
            {
                return Err(TaskflowCode::TaskPhasesOutcomeIncomplete.code());
            }
            terminals += 1;
            continue;
        }
        let phase = &phases[index];
        let eligible = |operation: &&TaskPhaseOperation<'_>| {
            phase.operations.iter().any(|id| id == operation.id)
        };
        let operation = operations
            .iter()
            .filter(eligible)
            .find(|operation| operation.from != "any" && canonical(operation.from) == page)
            .or_else(|| {
                operations
                    .iter()
                    .filter(eligible)
                    .find(|operation| operation.from == "any")
            })
            .ok_or(TaskflowCode::TaskPhasesUnreachable.code())?;
        if effect && designated == Some(operation.id) {
            return Err(TaskflowCode::TaskPhasesDesignatedEffectRepeated.code());
        }
        if operation.destinations.is_empty() {
            return Err(TaskflowCode::TaskPhasesPostconditionMissing.code());
        }
        let next_effect = effect || designated == Some(operation.id);
        for destination in &operation.destinations {
            if destination == "any" {
                return Err(TaskflowCode::TaskPhasesPostconditionMissing.code());
            }
            let destination = canonical(destination);
            let next_index = index
                + usize::from(
                    phase
                        .target_pages
                        .iter()
                        .any(|target| canonical(target) == destination),
                );
            pending.push_back((next_index, destination, next_effect));
        }
    }
    if terminals == 0 {
        return Err(TaskflowCode::TaskPhasesUnreachable.code());
    }
    Ok(())
}

pub const fn postcondition_timeout_is_valid(timeout_ms: Option<u64>) -> bool {
    match timeout_ms {
        Some(value) => value > 0 && value <= MAX_POSTCONDITION_TIMEOUT_MS,
        None => true,
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_extraction: Option<OcrFieldTextExtraction>,
}

/// A single bounded suffix, interpreted identically by execution and projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum OcrFieldTextExtraction {
    StripDeclaredSuffixV1 { suffix: Vec<OcrFieldSuffixSegment> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OcrFieldSuffixSegment {
    AsciiDigits { count: u8 },
    Literal { value: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFieldExtractionVersion {
    StripDeclaredSuffixV1,
}

/// Half-open byte offsets in the original normalized text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldTextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldExtractionEvidence {
    pub rule_version: OcrFieldExtractionVersion,
    pub matched_suffix: OcrFieldTextRange,
    pub extracted_text: String,
    pub extracted_range: OcrFieldTextRange,
}

impl OcrFieldTextExtraction {
    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        let Self::StripDeclaredSuffixV1 { suffix } = self;
        if !(1..=16).contains(&suffix.len()) {
            return Err(TaskflowCode::OcrFieldsTextExtractionInvalid.code());
        }
        let mut bytes = 0;
        for segment in suffix {
            bytes += match segment {
                OcrFieldSuffixSegment::AsciiDigits { count } if (1..=8).contains(count) => {
                    usize::from(*count)
                }
                OcrFieldSuffixSegment::Literal { value } if (1..=64).contains(&value.len()) => {
                    value.len()
                }
                _ => return Err(TaskflowCode::OcrFieldsTextExtractionInvalid.code()),
            };
        }
        if bytes > 256 {
            return Err(TaskflowCode::OcrFieldsTextExtractionInvalid.code());
        }
        Ok(())
    }

    /// Call only after the complete dictionary input has no exact or alias match.
    pub fn extract(
        &self,
        normalized_text: &str,
    ) -> Result<Option<OcrFieldExtractionEvidence>, crate::outcome::Code> {
        self.validate()?;
        let Self::StripDeclaredSuffixV1 { suffix } = self;
        let mut end = normalized_text.len();
        for segment in suffix.iter().rev() {
            let length = match segment {
                OcrFieldSuffixSegment::AsciiDigits { count } => usize::from(*count),
                OcrFieldSuffixSegment::Literal { value } => value.len(),
            };
            let Some(start) = end.checked_sub(length) else {
                return Ok(None);
            };
            let Some(text) = normalized_text.get(start..end) else {
                return Ok(None);
            };
            let matches = match segment {
                OcrFieldSuffixSegment::AsciiDigits { .. } => {
                    text.bytes().all(|byte| byte.is_ascii_digit())
                }
                OcrFieldSuffixSegment::Literal { value } => text == value,
            };
            if !matches {
                return Ok(None);
            }
            end = start;
        }
        let prefix = &normalized_text[..end];
        let extracted_text = prefix.trim();
        if extracted_text.is_empty() {
            return Ok(None);
        }
        let start = prefix.len() - prefix.trim_start().len();
        Ok(Some(OcrFieldExtractionEvidence {
            rule_version: OcrFieldExtractionVersion::StripDeclaredSuffixV1,
            matched_suffix: OcrFieldTextRange {
                start: end,
                end: normalized_text.len(),
            },
            extracted_text: extracted_text.to_owned(),
            extracted_range: OcrFieldTextRange {
                start,
                end: start + extracted_text.len(),
            },
        }))
    }

    pub fn verify(
        &self,
        normalized_text: &str,
        evidence: &OcrFieldExtractionEvidence,
    ) -> Result<(), crate::outcome::Code> {
        if self.extract(normalized_text)?.as_ref() != Some(evidence) {
            return Err(TaskflowCode::OcrFieldsTextExtractionEvidenceInvalid.code());
        }
        Ok(())
    }
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
        #[serde(
            default,
            skip_serializing_if = "OcrUnsignedIntegerFormat::is_ascii_decimal"
        )]
        format: OcrUnsignedIntegerFormat,
    },
    DictionaryEntry {
        dictionary: OcrFieldDictionaryReference,
    },
}

// Interface anchor `package` revision 2 (contracts/component-interfaces.md): raise its revision in
// distribution/windows/component-interfaces.json when packs need what older readers lack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrUnsignedIntegerFormat {
    #[default]
    AsciiDecimal,
    CommaGrouped,
    CurrentCapacity,
    /// ASCII decimal digits after at most one optional `x`, `X` or `×` (U+00D7) prefix.
    TimesPrefixed,
    /// Workflow #396 REC-6: `H:MM:SS`, 1 to 3 ASCII digits of hours, then two digits each of
    /// minutes and seconds in `00..=59`; the value is the number of seconds.
    ClockHms,
}

/// Both parts of one parsed `unsigned_integer` observation (Workflow #396 REC-6): the value, and
/// for `current_capacity` the capacity (the right-hand part), which only a value test reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrUnsignedIntegerParts {
    pub value: u64,
    pub capacity: Option<u64>,
}

impl OcrUnsignedIntegerFormat {
    fn is_ascii_decimal(&self) -> bool {
        *self == Self::AsciiDecimal
    }

    /// Parses the complete already-trimmed observation without changing its text.
    pub fn parse(self, text: &str, min: u64, max: u64) -> Result<u64, OcrFieldReason> {
        let value = self.parse_parts(text)?.value;
        if value < min || value > max {
            return Err(OcrFieldReason::OutOfRange);
        }
        Ok(value)
    }

    /// Parses the complete already-trimmed observation into both its parts, without a range
    /// check (Workflow #396 REC-6). [`Self::parse`] checks the value against its range; a value
    /// test checks the part it declares.
    pub fn parse_parts(self, text: &str) -> Result<OcrUnsignedIntegerParts, OcrFieldReason> {
        if text.is_empty() {
            return Err(OcrFieldReason::Empty);
        }
        let decimal = |digits: &str| {
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(OcrFieldReason::InvalidInteger);
            }
            digits.parse::<u64>().map_err(|_| OcrFieldReason::Overflow)
        };
        let mut capacity = None;
        let value = match self {
            Self::AsciiDecimal => decimal(text)?,
            Self::CommaGrouped => {
                let mut groups = text.split(',');
                let first = groups.next().ok_or(OcrFieldReason::InvalidInteger)?;
                if first.is_empty()
                    || first.len() > 3
                    || !first.bytes().all(|byte| byte.is_ascii_digit())
                    || (first.starts_with('0') && (first.len() > 1 || text.contains(',')))
                    || !groups.all(|group| {
                        group.len() == 3 && group.bytes().all(|byte| byte.is_ascii_digit())
                    })
                {
                    return Err(OcrFieldReason::InvalidInteger);
                }
                text.bytes()
                    .filter(|byte| *byte != b',')
                    .try_fold(0_u64, |value, byte| {
                        value
                            .checked_mul(10)
                            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                            .ok_or(OcrFieldReason::Overflow)
                    })?
            }
            Self::CurrentCapacity => {
                let (current, declared) =
                    text.split_once('/').ok_or(OcrFieldReason::InvalidInteger)?;
                let current = decimal(current)?;
                capacity = Some(decimal(declared)?);
                current
            }
            Self::TimesPrefixed => decimal(
                text.strip_prefix(|prefix: char| matches!(prefix, 'x' | 'X' | '\u{00D7}'))
                    .unwrap_or(text),
            )?,
            Self::ClockHms => {
                let mut parts = text.split(':');
                let (Some(hours), Some(minutes), Some(seconds), None) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                else {
                    return Err(OcrFieldReason::InvalidInteger);
                };
                let sixty = |digits: &str| {
                    if digits.len() != 2 {
                        return Err(OcrFieldReason::InvalidInteger);
                    }
                    decimal(digits).and_then(|value| {
                        if value < 60 {
                            Ok(value)
                        } else {
                            Err(OcrFieldReason::InvalidInteger)
                        }
                    })
                };
                if !(1..=3).contains(&hours.len()) {
                    return Err(OcrFieldReason::InvalidInteger);
                }
                let hours = decimal(hours)?;
                hours * 3_600 + sixty(minutes)? * 60 + sixty(seconds)?
            }
        };
        Ok(OcrUnsignedIntegerParts { value, capacity })
    }
}

/// How a value test trims an observed text before parsing it (Workflow #396 REC-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrValueTrim {
    /// Unicode whitespace at both ends, as `str::trim` (the fields' `whitespace_v1`).
    WhitespaceV1,
    /// Every Unicode whitespace character, wherever it stands (`1 / 3` reads as `1/3`).
    WhitespaceAllV1,
}

impl OcrValueTrim {
    pub fn apply(self, text: &str) -> std::borrow::Cow<'_, str> {
        match self {
            Self::WhitespaceV1 => std::borrow::Cow::Borrowed(text.trim()),
            Self::WhitespaceAllV1 => std::borrow::Cow::Owned(
                text.chars()
                    .filter(|character| !character.is_whitespace())
                    .collect(),
            ),
        }
    }
}

/// The comparison of a value test (Workflow #396 REC-6): `parses` needs only the left side to
/// read; the others compare the left value with the right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTestOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Parses,
}

impl ValueTestOp {
    /// Whether `left op right` holds; `parses` always holds once the left side reads.
    pub fn holds(self, left: i128, right: i128) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Ne => left != right,
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
            Self::Parses => true,
        }
    }
}

/// Why one side of a value test could not be read (Workflow #396 REC-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueTestProblem {
    Empty,
    Invalid,
    Overflow,
    OutOfRange,
    RegionUnresolved,
    LowConfidence,
}

/// A dictionary a field reads, by its task-local path. Workflow #336 PKG-S2 (option 丙, M-PKG
/// §3.2, D5): a `sha256` the reference may still carry is an [`IgnoredPin`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OcrFieldDictionaryReference {
    pub path: String,
    #[serde(default, rename = "sha256", skip_serializing)]
    pub ignored_sha256: IgnoredPin,
}

/// Workflow #336 PKG-S2 (option 丙, M-PKG §3.2, D5): the `sha256` key a resource file may still
/// carry on a select step's policy, an OCR dictionary reference or a post-admission truth set.
/// The key is optional and its value, whatever it is, is ignored when read: it is neither
/// validated nor compared, and never written or echoed. PKG-S1b refuses the key again once the
/// resource repositories no longer carry it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IgnoredPin;

impl<'de> Deserialize<'de> for IgnoredPin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Self)
    }
}

/// Readings one task may declare (`contracts/resource-readings.md`).
pub const MAX_RESOURCE_READINGS: usize = 16;
/// The largest value a reading may declare: 2^53 - 1, exact in every JSON consumer.
pub const MAX_RESOURCE_READING_VALUE: u64 = (1 << 53) - 1;

/// One operation `resource_readings` entry (schema 0.8 and 0.9, Workflow #335 S5), a reading
/// mark: the integer the Runtime reads from `target_id` on each frame a successful run reaches
/// the task's page `page_id` on (Workflow #384 D2-S2: any page of the task) and writes as the
/// instance fact `fact_key`. The declaration never enters an event, artifact or report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceReadingDeclaration {
    pub id: String,
    pub fact_key: String,
    pub page_id: String,
    pub target_id: String,
    pub trim: OcrFieldTrim,
    pub value: ResourceReadingType,
    pub minimum_confidence_milli: u16,
    pub valid_for_ms: u64,
}

/// The closed set of reading value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceReadingType {
    UnsignedInteger {
        min: u64,
        max: u64,
        #[serde(
            default,
            skip_serializing_if = "OcrUnsignedIntegerFormat::is_ascii_decimal"
        )]
        format: OcrUnsignedIntegerFormat,
    },
}

impl ResourceReadingDeclaration {
    /// The shared rule of one declaration. The declaration gate checks the same rules field by
    /// field first; each reason names the field it rejects.
    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-/.:".contains(&c))
        };
        if !identifier(&self.id) {
            return Err(TaskflowCode::ResourceReadingIdInvalid.code());
        }
        if !(self.fact_key.starts_with("resource.") || self.fact_key.starts_with("inventory."))
            || crate::fact::validate_fact_key(&self.fact_key).is_err()
        {
            return Err(TaskflowCode::ResourceReadingFactKeyInvalid.code());
        }
        if !identifier(&self.page_id) || self.page_id == "any" {
            return Err(TaskflowCode::ResourceReadingPageIdInvalid.code());
        }
        if !identifier(&self.target_id) {
            return Err(TaskflowCode::ResourceReadingTargetIdInvalid.code());
        }
        let ResourceReadingType::UnsignedInteger { min, max, .. } = self.value;
        if max > MAX_RESOURCE_READING_VALUE {
            return Err(TaskflowCode::ResourceReadingValueMaxInvalid.code());
        }
        if min > max {
            return Err(TaskflowCode::ResourceReadingValueMinInvalid.code());
        }
        if !(1..=1_000).contains(&self.minimum_confidence_milli) {
            return Err(TaskflowCode::ResourceReadingMinimumConfidenceInvalid.code());
        }
        if !(crate::MIN_FACT_TTL_MS..=crate::MAX_FACT_TTL_MS).contains(&self.valid_for_ms) {
            return Err(TaskflowCode::ResourceReadingValidForInvalid.code());
        }
        Ok(())
    }

    /// Trims and parses one observed text with the declared value type and range. This is the
    /// only reading parser; Workflow #308 slice 2 reuses it.
    pub fn parse(&self, text: &str) -> Result<u64, OcrFieldReason> {
        let text = match self.trim {
            OcrFieldTrim::WhitespaceV1 => text.trim(),
        };
        let ResourceReadingType::UnsignedInteger { min, max, format } = self.value;
        format.parse(text, min, max)
    }
}

/// One task's `resource_readings`: 1..=16 valid entries with unique ids and fact keys.
pub fn validate_resource_readings(
    readings: &[ResourceReadingDeclaration],
) -> Result<(), crate::outcome::Code> {
    if readings.is_empty() || readings.len() > MAX_RESOURCE_READINGS {
        return Err(TaskflowCode::ResourceReadingsCountInvalid.code());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut fact_keys = std::collections::BTreeSet::new();
    for reading in readings {
        reading.validate()?;
        if !ids.insert(reading.id.as_str()) {
            return Err(TaskflowCode::ResourceReadingIdDuplicate.code());
        }
        if !fact_keys.insert(reading.fact_key.as_str()) {
            return Err(TaskflowCode::ResourceReadingFactKeyDuplicate.code());
        }
    }
    Ok(())
}

/// An OCR confidence as integer milli, `floor(clamp(confidence, 0, 1) * 1000)`. A missing or
/// non-finite confidence has none. This is the only conversion; Workflow #308 slice 2 reuses it.
pub fn ocr_confidence_milli(confidence: Option<f32>) -> Option<u16> {
    confidence
        .filter(|confidence| confidence.is_finite())
        .map(|confidence| (confidence.clamp(0.0, 1.0) * 1_000.0).floor() as u16)
}

/// The identifier rule of reading marks and adjustments: `[A-Za-z0-9_\-/.:]`, `1..=128` bytes.
fn resource_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-/.:".contains(&c))
}

/// The key rule of reading marks and adjustments: a realistic instance fact key.
fn resource_fact_key(value: &str) -> bool {
    (value.starts_with("resource.") || value.starts_with("inventory."))
        && crate::fact::validate_fact_key(value).is_ok()
}

/// Adjustments one task may declare (Workflow #384 D2-S3), as many as reading marks.
pub const MAX_RESOURCE_ADJUSTMENTS: usize = MAX_RESOURCE_READINGS;

/// One operation `resource_adjustments` entry (Workflow #384 D2-S3, D2 model §3.6): once the
/// task's operation `operation_id` is confirmed, the instance fact `fact_key` has changed. The
/// new value comes only from live recognition. Without `amount` it is a reading mark of the key
/// that the run reads after the confirmation; with `amount` it is the change the screen shows
/// on the confirmation frame, added to or subtracted from the key's current record. A package
/// never declares an amount itself. The declaration never enters an event, artifact or report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdjustmentDeclaration {
    pub id: String,
    pub fact_key: String,
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<ResourceAdjustmentAmount>,
}

/// Where an adjustment reads the change the screen shows: one OCR target of the task, and
/// whether the change is spent or gained. No other source of an amount exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAdjustmentAmount {
    pub target_id: String,
    pub direction: ResourceAdjustmentDirection,
}

/// Whether an adjustment's amount is subtracted from or added to the key's current value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceAdjustmentDirection {
    Consume,
    Gain,
}

impl ResourceAdjustmentDeclaration {
    /// The shared rule of one declaration. The declaration gate checks the same rules field by
    /// field first; each reason names the field it rejects.
    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
        if !resource_identifier(&self.id) {
            return Err(TaskflowCode::ResourceAdjustmentIdInvalid.code());
        }
        if !resource_fact_key(&self.fact_key) {
            return Err(TaskflowCode::ResourceAdjustmentFactKeyInvalid.code());
        }
        if !resource_identifier(&self.operation_id) {
            return Err(TaskflowCode::ResourceAdjustmentOperationIdInvalid.code());
        }
        if self
            .amount
            .as_ref()
            .is_some_and(|amount| !resource_identifier(&amount.target_id))
        {
            return Err(TaskflowCode::ResourceAdjustmentTargetIdInvalid.code());
        }
        Ok(())
    }
}

impl ResourceAdjustmentAmount {
    /// Trims one observed change and parses it as ASCII decimal digits after at most one sign
    /// of its direction (`+` for a gain, `-` for a consumption), within `0..=2^53-1`. The
    /// digits are read by the reading parser (`OcrUnsignedIntegerFormat`).
    pub fn parse(&self, text: &str) -> Result<u64, OcrFieldReason> {
        let text = text.trim();
        let sign = match self.direction {
            ResourceAdjustmentDirection::Consume => '-',
            ResourceAdjustmentDirection::Gain => '+',
        };
        let digits = text.strip_prefix(sign).unwrap_or(text);
        OcrUnsignedIntegerFormat::AsciiDecimal.parse(digits, 0, MAX_RESOURCE_READING_VALUE)
    }
}

/// One task's `resource_adjustments` (Workflow #384 D2-S3): `1..=16` valid entries with unique
/// ids, each naming one of the task's `operation_ids` and adjusting a key at most once per
/// operation. An adjustment without `amount` needs a reading mark of its key in `readings`.
pub fn validate_resource_adjustments(
    adjustments: &[ResourceAdjustmentDeclaration],
    readings: Option<&[ResourceReadingDeclaration]>,
    operation_ids: &[&str],
) -> Result<(), crate::outcome::Code> {
    if adjustments.is_empty() || adjustments.len() > MAX_RESOURCE_ADJUSTMENTS {
        return Err(TaskflowCode::ResourceAdjustmentsCountInvalid.code());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut changes = std::collections::BTreeSet::new();
    for adjustment in adjustments {
        adjustment.validate()?;
        if !operation_ids.contains(&adjustment.operation_id.as_str()) {
            return Err(TaskflowCode::ResourceAdjustmentOperationIdInvalid.code());
        }
        if adjustment.amount.is_none()
            && !readings
                .unwrap_or_default()
                .iter()
                .any(|reading| reading.fact_key == adjustment.fact_key)
        {
            return Err(TaskflowCode::ResourceAdjustmentMarkMissing.code());
        }
        if !ids.insert(adjustment.id.as_str()) {
            return Err(TaskflowCode::ResourceAdjustmentIdDuplicate.code());
        }
        if !changes.insert((
            adjustment.operation_id.as_str(),
            adjustment.fact_key.as_str(),
        )) {
            return Err(TaskflowCode::ResourceAdjustmentFactKeyDuplicate.code());
        }
    }
    Ok(())
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
    ) -> Result<(), crate::outcome::Code> {
        self.validate()?;
        scheduling
            .validate()
            .map_err(|_| TaskflowCode::OcrFieldsZeroInputOutcomeInvalid.code())?;
        let [mapping] = scheduling.mappings() else {
            return Err(TaskflowCode::OcrFieldsZeroInputOutcomeInvalid.code());
        };
        if execution_mode != "navigable_route"
            || !stop_on_confirmation
            || scheduling.designated_operation().is_some()
            || mapping.outcome_key() != self.outcome_key
            || mapping.effect() != SchedulingEffectCondition::NoDesignatedEffect
        {
            return Err(TaskflowCode::OcrFieldsZeroInputOutcomeInvalid.code());
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
            return Err(TaskflowCode::OcrFieldsZeroInputPagesInvalid.code());
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), crate::outcome::Code> {
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
            return Err(SharedCode::OcrFieldsDeclarationInvalid.code());
        }
        for field in &self.fields {
            if let Some(extraction) = &field.text_extraction {
                if !matches!(field.value, OcrFieldType::DictionaryEntry { .. }) {
                    return Err(TaskflowCode::OcrFieldsTextExtractionTypeInvalid.code());
                }
                extraction.validate()?;
            }
            if !identifier(&field.id)
                || !identifier(&field.group)
                || !identifier(&field.target_id)
                || !ids.insert(&field.id)
                || !targets.insert(&field.target_id)
            {
                return Err(TaskflowCode::OcrFieldsBindingInvalid.code());
            }
            match &field.value {
                OcrFieldType::UnsignedInteger { min, max, .. } if min > max => {
                    return Err(TaskflowCode::OcrFieldsRangeInvalid.code());
                }
                OcrFieldType::DictionaryEntry { dictionary }
                    if dictionary.path.is_empty()
                        || dictionary.path.len() > 256
                        || dictionary.path.contains(['\\', ':'])
                        || dictionary
                            .path
                            .split('/')
                            .any(|p| p.is_empty() || p == "." || p == "..") =>
                {
                    return Err(SharedCode::OcrFieldsDictionaryReferenceInvalid.code());
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
    pub fn validate(&self, limits: &OcrFieldsLimits) -> Result<(), crate::outcome::Code> {
        if !matches!(
            self.schema_version.as_str(),
            "actingcommand.ocr-truth-set.v2"
        ) && !(self.schema_version == "actingcommand.ocr-truth-set.v1" && self.aliases.is_none())
        {
            return Err(TaskflowCode::OcrFieldsDictionarySchemaInvalid.code());
        }
        if self.items.is_empty()
            || self.items.len() > limits.max_truth_entries as usize
            || self.aliases.as_ref().is_some_and(|v| v.len() > 1024)
        {
            return Err(SharedCode::OcrFieldsDictionaryLimitExceeded.code());
        }
        let valid = |v: &str| {
            !v.trim().is_empty()
                && v.len() <= limits.max_string_bytes as usize
                && v.trim().to_lowercase().len() <= limits.max_string_bytes as usize
        };
        let mut canonical = std::collections::BTreeSet::new();
        for item in &self.items {
            if !valid(item) || !canonical.insert(item.trim().to_lowercase()) {
                return Err(TaskflowCode::OcrFieldsDictionaryAmbiguous.code());
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
                return Err(TaskflowCode::OcrFieldsDictionaryAmbiguous.code());
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
    /// Workflow R-11 (OCR-CONF): the text resolved, but the observation's confidence is missing
    /// or below the `minimum_confidence` its OCR target declares, so no value is accepted.
    LowConfidence,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<OcrFieldExtractionEvidence>,
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
