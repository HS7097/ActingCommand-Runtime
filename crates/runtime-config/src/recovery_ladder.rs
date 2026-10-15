// SPDX-License-Identifier: AGPL-3.0-only

//! The `recovery_ladder` section (Workflow #387 D1c, frozen model M387 v1.2 §3, §4, §7): which
//! codes start a ladder (an ordered list of trigger items and an exclusion list), the rungs in
//! the order they climb with each rung's bounds, the hold, the cool-down and the five rules of
//! §2.2a. Every value lives in the key table and the defaults file; no code constant stands in
//! for one (Alice 10-09). The section is read whole and checked whole (§5.3): at start and in
//! `check-config` a broken rule is `recovery_ladder_config_invalid`, through an instruction the
//! same values are `config_value_invalid` and the Runtime keeps its configuration.
//!
//! An element a `triggers` or `rungs` list leaves out takes its value from the schema, never
//! from code (§3.1, §4.1): a trigger item's `start`, `start_rung` and `respect_cooldown` from
//! `trigger_defaults`, a rung's bounds from the defaults file's rung of the same action.
//!
//! Workflow #398 RD-4 (model-398 v1.1 §6.2, §6.6, option B): ladder profiles. `profiles` names
//! rung lists of their own, each checked as `rungs` is; `kind_profiles` gives a device kind its
//! profile; an instance's own `recovery_ladder_profile` comes first. A ladder climbs the rungs of
//! the profile its instance takes (instance key, then kind table, then the section's `rungs`).
//! `rules.start_beyond_profile` says what a start rung beyond a profile's rungs does: `exhaust`
//! (every rung skipped before the start, then exhausted), `last_rung` (the climb starts at the
//! profile's last rung) or `refuse` (every start rung must fit every profile).

use crate::codes::{ConfigCode, ConfigValueProblem};
use crate::defaults::shipped_json;
use crate::effective::{CheckContext, ConfigFailure};
use crate::instance::RECOVERY_LADDER_PROFILE;
use crate::resolve::member;
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, Settable, ValueType, instance_key,
};
use crate::value::{canonical_json, scalar_json};
use actingcommand_contract::outcome::{LadderClass, catalog_json};
use actingcommand_contract::{
    ContainedTaskRequest, FactScalar, InstanceDeviceKind, InstanceStuckRecovery, RecoveryRung,
    RecoveryTriggerStage, RecoveryTriggerStart, parse_ladder_class, valid_recovery_ladder_token,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "recovery_ladder";
/// Whether the ladder may start at all (an instance's `stuck_recovery` defaults to it).
pub const ENABLED: &str = "recovery_ladder.enabled";
/// The cool-down, in seconds (an instance's `stuck_recovery_cooldown_secs` defaults to it).
pub const COOLDOWN_SECS: &str = "recovery_ladder.cooldown_secs";
/// What a trigger item that leaves out `start` takes.
pub const DEFAULT_START: &str = "recovery_ladder.trigger_defaults.start";
/// What a trigger item that leaves out `start_rung` takes, and where a resource fence's ladder
/// starts.
pub const DEFAULT_START_RUNG: &str = "recovery_ladder.trigger_defaults.start_rung";
/// What a trigger item that leaves out `respect_cooldown` takes, and whether a resource
/// fence's ladder is held by the cool-down.
pub const DEFAULT_RESPECT_COOLDOWN: &str = "recovery_ladder.trigger_defaults.respect_cooldown";
/// The ordered trigger items (a list).
pub const TRIGGERS: &str = "recovery_ladder.triggers";
/// The codes and prefixes no trigger item matches (a list).
pub const EXCLUDE_CODES: &str = "recovery_ladder.exclude_codes";
/// B1: a direct run's terminal and its records.
pub const RULE_DIRECT_RUNS: &str = "recovery_ladder.rules.direct_runs";
/// B2: a preparation ladder whose preparation a later one superseded.
pub const RULE_PREPARATION_SUPERSEDED: &str = "recovery_ladder.rules.preparation_superseded";
/// B3: a trigger while a ladder holds or waits for the instance.
pub const RULE_ALREADY_RUNNING: &str = "recovery_ladder.rules.already_running";
/// B4: when the cool-down window opens.
pub const RULE_COOLDOWN_OPENS_ON: &str = "recovery_ladder.rules.cooldown_opens_on";
/// B5: a rung whose entry channel is unavailable.
pub const RULE_ENTRY_UNAVAILABLE: &str = "recovery_ladder.rules.entry_unavailable";
/// The rungs in the order they climb, each with its bounds (a list).
pub const RUNGS: &str = "recovery_ladder.rungs";
/// The TTL a ladder claim, and each continuation, is granted with.
pub const CLAIM_TTL_MS: &str = "recovery_ladder.hold.claim_ttl_ms";
/// What a hold renews to beyond a step's bound.
pub const MARGIN_MS: &str = "recovery_ladder.hold.margin_ms";
/// Workflow #398 RD-4 (model-398 §6.6): what a start rung beyond a profile's rungs does.
pub const RULE_START_BEYOND_PROFILE: &str = "recovery_ladder.rules.start_beyond_profile";
/// Workflow #398 RD-4 (model-398 §6.2): the ladder profiles, profile name to `{rungs}` (an
/// object).
pub const PROFILES: &str = "recovery_ladder.profiles";
/// Workflow #398 RD-4 (model-398 §6.2): the profile of each device kind, kind to profile name
/// (an object); a kind it does not name climbs the section's `rungs`.
pub const KIND_PROFILES: &str = "recovery_ladder.kind_profiles";

/// The shortest readiness poll a ladder may be given (§4.2: `100..=readiness_window_ms`):
/// schema, not a threshold.
const MIN_READINESS_POLL_MS: u64 = 100;
/// The shortest ADB probe the application restart rung may be given (§4.2): schema.
const MIN_ADB_PROBE_MS: u64 = 1_000;
/// The prefix marker of a `codes` or `exclude_codes` entry (§3.1 「前缀*」).
const PREFIX_MARKER: char = '*';

macro_rules! two_way_rule {
    (
        $(#[$meta:meta])*
        $name:ident { $first:ident => $first_token:literal, $second:ident => $second_token:literal }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $first,
            $second,
        }

        impl $name {
            /// The rule's two words, its key's allowed range.
            pub const TOKENS: &'static [&'static str] = &[$first_token, $second_token];

            pub const fn as_str(self) -> &'static str {
                match self {
                    Self::$first => $first_token,
                    Self::$second => $second_token,
                }
            }

            fn parse(token: &str) -> Option<Self> {
                match token {
                    $first_token => Some(Self::$first),
                    $second_token => Some(Self::$second),
                    _ => None,
                }
            }
        }
    };
}

two_way_rule! {
    /// B1 (§2.2a): a direct run's (CLI or console task-run, MCP `ac_run_pack`) terminal, the
    /// records committed while manual work holds the instance's key (the resource fence's set:
    /// a direct task run, a client lease, an application or input command, a Lab operation;
    /// Workflow #387 D1c-H), and a resource fence that manual work met.
    DirectRuns { Exempt => "exempt", Trigger => "trigger" }
}

two_way_rule! {
    /// B2 (§2.2a): a preparation ladder whose preparation a later one superseded.
    PreparationSuperseded { Suppress => "suppress", Keep => "keep" }
}

two_way_rule! {
    /// B3 (§2.2a): a trigger while a ladder holds or waits for the instance.
    AlreadyRunning { Suppress => "suppress", Queue => "queue" }
}

two_way_rule! {
    /// B4 (§2.2a): `admitted` opens the cool-down window when a ladder gets the key, `triggered`
    /// whenever a trigger item matches.
    CooldownOpensOn { Admitted => "admitted", Triggered => "triggered" }
}

two_way_rule! {
    /// B5 (§2.2a): a rung whose entry channel is unavailable is skipped or fails.
    EntryUnavailable { Skip => "skip", Fail => "fail" }
}

/// Workflow #398 RD-4 (model-398 v1.1 §6.6, option B), the sixth rule beside B1–B5: what a
/// trigger's start rung beyond the rungs of the profile its instance climbs does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartBeyondProfile {
    /// The climb starts; every rung of the profile is skipped as before the start rung, and the
    /// ladder ends exhausted at once (the default).
    Exhaust,
    /// The climb starts at the profile's last rung.
    LastRung,
    /// Every trigger item's start rung and `trigger_defaults.start_rung` must lie within every
    /// profile's rungs; the section check refuses any other.
    Refuse,
}

impl StartBeyondProfile {
    /// The rule's three words, its key's allowed range.
    pub const TOKENS: &'static [&'static str] = &["exhaust", "last_rung", "refuse"];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exhaust => Self::TOKENS[0],
            Self::LastRung => Self::TOKENS[1],
            Self::Refuse => Self::TOKENS[2],
        }
    }

    fn parse(token: &str) -> Option<Self> {
        [Self::Exhaust, Self::LastRung, Self::Refuse]
            .into_iter()
            .find(|rule| rule.as_str() == token)
    }

    /// The position, from 1, a climb over `rung_count` rungs starts at for a trigger that names
    /// `start_rung`: under `last_rung` no later than the last rung; otherwise the trigger's own
    /// (beyond the rungs under `exhaust`, where every rung is before it; `refuse` admits no
    /// start beyond a profile).
    pub fn start_rung(self, start_rung: usize, rung_count: usize) -> usize {
        match self {
            Self::LastRung => start_rung.min(rung_count),
            Self::Exhaust | Self::Refuse => start_rung,
        }
    }
}

/// The words of `start`.
const START_TOKENS: &[&str] = &["immediately", "after_performance_release"];

/// The section as the configuration file and the defaults file write it: every member may be
/// left out (a file names only what it changes) and none is unknown (§5.3). Each value keeps
/// its JSON here; the key table checks its type and range, the whole-section check its rules.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryLadderFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cooldown_secs: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trigger_defaults: Option<TriggerDefaultsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    triggers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude_codes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rules: Option<RulesFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rungs: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold: Option<HoldFile>,
    /// Workflow #398 RD-4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profiles: Option<Value>,
    /// Workflow #398 RD-4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind_profiles: Option<Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TriggerDefaultsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_rung: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    respect_cooldown: Option<Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    direct_runs: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preparation_superseded: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    already_running: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cooldown_opens_on: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_unavailable: Option<Value>,
    /// Workflow #398 RD-4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_beyond_profile: Option<Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HoldFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_ttl_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    margin_ms: Option<Value>,
}

const fn row(name: &'static str, value_type: ValueType, range: KeyRange) -> KeyRow {
    row_of(name, value_type, range, "D1c")
}

const fn row_of(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    reader: &'static str,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::File,
        // §5.5: every key of the section can change while the Runtime runs; a ladder climbs by
        // the snapshot it was admitted with (§4.7). Workflow #398 RD-4's keys too (model-398
        // §6.2, §6.6).
        settable: Settable::Yes,
        reader,
        since: "0.12.0",
    }
}

/// One row per key of the section (§5.5; `contracts/v012-register.md` §5).
pub(crate) static ROWS: &[KeyRow] = &[
    row(ENABLED, ValueType::Boolean, KeyRange::Any),
    row(
        COOLDOWN_SECS,
        ValueType::Integer,
        KeyRange::Integer {
            min: 1,
            max: InstanceStuckRecovery::MAX_COOLDOWN_SECS as i64,
        },
    ),
    row(
        DEFAULT_START,
        ValueType::String,
        KeyRange::Tokens(START_TOKENS),
    ),
    row(
        DEFAULT_START_RUNG,
        ValueType::Integer,
        KeyRange::Integer {
            min: 1,
            max: RecoveryRung::ALL.len() as i64,
        },
    ),
    row(DEFAULT_RESPECT_COOLDOWN, ValueType::Boolean, KeyRange::Any),
    row(TRIGGERS, ValueType::List, KeyRange::Any),
    row(EXCLUDE_CODES, ValueType::List, KeyRange::Any),
    row(
        RULE_DIRECT_RUNS,
        ValueType::String,
        KeyRange::Tokens(DirectRuns::TOKENS),
    ),
    row(
        RULE_PREPARATION_SUPERSEDED,
        ValueType::String,
        KeyRange::Tokens(PreparationSuperseded::TOKENS),
    ),
    row(
        RULE_ALREADY_RUNNING,
        ValueType::String,
        KeyRange::Tokens(AlreadyRunning::TOKENS),
    ),
    row(
        RULE_COOLDOWN_OPENS_ON,
        ValueType::String,
        KeyRange::Tokens(CooldownOpensOn::TOKENS),
    ),
    row(
        RULE_ENTRY_UNAVAILABLE,
        ValueType::String,
        KeyRange::Tokens(EntryUnavailable::TOKENS),
    ),
    row_of(
        RULE_START_BEYOND_PROFILE,
        ValueType::String,
        KeyRange::Tokens(StartBeyondProfile::TOKENS),
        "RD-4",
    ),
    row(RUNGS, ValueType::List, KeyRange::Any),
    row_of(PROFILES, ValueType::Object, KeyRange::Any, "RD-4"),
    row_of(KIND_PROFILES, ValueType::Object, KeyRange::Any, "RD-4"),
    row(CLAIM_TTL_MS, ValueType::DurationMs, KeyRange::Any),
    row(MARGIN_MS, ValueType::DurationMs, KeyRange::Any),
];

/// A `codes` or `exclude_codes` entry: one code, or every code under a prefix (`prefix*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodePattern {
    Exact(String),
    Prefix(String),
}

impl CodePattern {
    fn read(text: &str) -> Self {
        match text.strip_suffix(PREFIX_MARKER) {
            Some(prefix) => Self::Prefix(prefix.to_owned()),
            None => Self::Exact(text.to_owned()),
        }
    }

    pub fn matches(&self, spelling: &str) -> bool {
        match self {
            Self::Exact(code) => code == spelling,
            Self::Prefix(prefix) => spelling.starts_with(prefix.as_str()),
        }
    }
}

/// Direction ① (§3.4): the `count`-th failure within `window_ms` starts the ladder; one failure
/// counts once, however many records it commits (Workflow #387 D1c-H).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BurstWindow {
    pub count: u32,
    pub window_ms: u64,
}

/// One trigger item (§3.1), its left-out members taken from `trigger_defaults`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerItem {
    pub id: String,
    pub stages: Vec<RecoveryTriggerStage>,
    pub classes: Vec<LadderClass>,
    pub codes: Vec<CodePattern>,
    /// Present exactly when `stages` names `stability_warnings`.
    pub burst: Option<BurstWindow>,
    pub start: RecoveryTriggerStart,
    /// The position in `rungs`, from 1, the climb starts at (§4.1).
    pub start_rung: usize,
    pub respect_cooldown: bool,
}

/// How a trigger item matched a code (§6.1 `matched`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    Code,
    Class(LadderClass),
}

/// What the trigger matching (§3.1) made of one code at one stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerVerdict<'a> {
    /// `exclude_codes` names the code: no item is looked at.
    Excluded,
    /// No item names the stage and the code or its class.
    Unmatched,
    /// The first item that matched.
    Matched {
        item: &'a TriggerItem,
        by: MatchedBy,
    },
}

/// The five rules of §2.2a.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LadderRules {
    pub direct_runs: DirectRuns,
    pub preparation_superseded: PreparationSuperseded,
    pub already_running: AlreadyRunning,
    pub cooldown_opens_on: CooldownOpensOn,
    pub entry_unavailable: EntryUnavailable,
}

/// One rung of `rungs` with its bounds (§4.2), in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RungConfig {
    ReturnHome {
        configured_package_deadline_ms: u64,
    },
    ApplicationRestart {
        adb_probe_ms: u64,
        entry_hold_ms: u64,
    },
    EmulatorRestart {
        stop_bound_ms: u64,
        start_bound_ms: u64,
        readiness_window_ms: u64,
        readiness_backoff_ms: Vec<u64>,
        readiness_poll_ms: u64,
    },
}

impl RungConfig {
    pub const fn action(&self) -> RecoveryRung {
        match self {
            Self::ReturnHome { .. } => RecoveryRung::ReturnHome,
            Self::ApplicationRestart { .. } => RecoveryRung::ApplicationRestart,
            Self::EmulatorRestart { .. } => RecoveryRung::EmulatorRestart,
        }
    }

    /// The longest single step of the rung, the bound a hold renews to before it (§4.3); the
    /// return-home rung's run renews its own key to its deadline.
    pub fn longest_step_ms(&self) -> u64 {
        match self {
            Self::ReturnHome {
                configured_package_deadline_ms,
            } => *configured_package_deadline_ms,
            Self::ApplicationRestart { entry_hold_ms, .. } => *entry_hold_ms,
            Self::EmulatorRestart {
                stop_bound_ms,
                start_bound_ms,
                readiness_window_ms,
                ..
            } => (*stop_bound_ms)
                .max(*start_bound_ms)
                .max(*readiness_window_ms),
        }
    }
}

/// The hold (§4.3), in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldConfig {
    pub claim_ttl_ms: u64,
    pub margin_ms: u64,
}

/// Workflow #398 RD-4 (model-398 §6.2): one ladder profile, its name and its rungs, each
/// checked as the section's `rungs` are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderProfile {
    pub name: String,
    pub rungs: Vec<RungConfig>,
}

/// The `recovery_ladder` section in effect, read and checked whole. A ladder keeps the one it
/// was admitted with for its whole climb, queue included (§4.7); `digest` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderConfig {
    pub enabled: bool,
    pub cooldown_secs: u32,
    pub triggers: Vec<TriggerItem>,
    pub exclude_codes: Vec<CodePattern>,
    pub rules: LadderRules,
    pub rungs: Vec<RungConfig>,
    pub hold: HoldConfig,
    /// Where a resource fence's ladder starts, and whether the cool-down holds it
    /// (`trigger_defaults`): a fence matches no trigger item.
    pub default_start_rung: usize,
    pub default_respect_cooldown: bool,
    /// Workflow #398 RD-4 (model-398 §6.6): the sixth rule, `rules.start_beyond_profile`.
    pub start_beyond_profile: StartBeyondProfile,
    /// Workflow #398 RD-4 (model-398 §6.2): the profiles, as `profiles` names them.
    pub profiles: Vec<LadderProfile>,
    /// Workflow #398 RD-4: the profile name of each device kind `kind_profiles` names.
    kind_profiles: Vec<(InstanceDeviceKind, String)>,
    /// Workflow #398 RD-4: the profile name each instance's `recovery_ladder_profile` names in
    /// effect, by the instance's registry id; a snapshot takes it with the section.
    instance_profiles: BTreeMap<String, String>,
    digest: String,
}

/// Whether the embedded catalog lists a code, and how (also read by the `performance` section's
/// calibration check, Workflow #385 P-c, and the `scheduling` section's code lists, Workflow #386
/// SR-1).
pub(crate) enum Listed {
    Active,
    Retired,
    Unlisted,
}

fn catalog_codes() -> Option<&'static Map<String, Value>> {
    catalog_json().ok()?.get("codes")?.as_object()
}

pub(crate) fn listed(spelling: &str) -> Listed {
    match catalog_codes().and_then(|codes| codes.get(spelling)) {
        None => Listed::Unlisted,
        Some(entry) if entry.get("status").and_then(Value::as_str) == Some("retired") => {
            Listed::Retired
        }
        Some(_) => Listed::Active,
    }
}

fn prefix_listed(prefix: &str) -> bool {
    catalog_codes().is_some_and(|codes| {
        codes.iter().any(|(spelling, entry)| {
            spelling.starts_with(prefix)
                && entry.get("status").and_then(Value::as_str) != Some("retired")
        })
    })
}

/// The ladder class the embedded catalog gives the code `spelling` (§3.2: looked up by
/// spelling, since the host holds codes as text); `None` when the catalog does not list it.
pub fn ladder_class_of(spelling: &str) -> Option<LadderClass> {
    catalog_codes()?
        .get(spelling)?
        .get("ladder_class")?
        .as_str()
        .and_then(parse_ladder_class)
}

fn failure(config_key: &str, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::with_problem(ConfigCode::RecoveryLadderConfigInvalid, config_key, problem)
}

/// The values of the section's keys, read one by one from the configuration in effect.
struct Reader<'a> {
    value: &'a dyn Fn(&str) -> Option<FactScalar>,
    /// Every key's value as JSON, for the digest.
    seen: Map<String, Value>,
}

impl Reader<'_> {
    fn scalar(&mut self, key: &'static str) -> Result<FactScalar, ConfigFailure> {
        let scalar =
            (self.value)(key).ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
        let value_type = ROWS
            .iter()
            .find(|row| row.name == KeyName::Exact(key))
            .map(|row| row.value_type)
            .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
        let json = scalar_json(value_type, &scalar)
            .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
        self.seen.insert(key.to_owned(), json);
        Ok(scalar)
    }

    fn boolean(&mut self, key: &'static str) -> Result<bool, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Boolean(flag) => Ok(flag),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    fn integer(&mut self, key: &'static str) -> Result<i64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::Integer(number) => Ok(number),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    fn duration(&mut self, key: &'static str) -> Result<u64, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::DurationMs(ms) => Ok(ms),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    fn word(&mut self, key: &'static str) -> Result<String, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => Ok(text),
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    fn list(&mut self, key: &'static str) -> Result<Vec<Value>, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(items)) => Ok(items),
                _ => Err(failure(key, ConfigValueProblem::WrongType)),
            },
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }

    /// Workflow #398 RD-4: an object key's members.
    fn object(&mut self, key: &'static str) -> Result<Map<String, Value>, ConfigFailure> {
        match self.scalar(key)? {
            FactScalar::String(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(members)) => Ok(members),
                _ => Err(failure(key, ConfigValueProblem::WrongType)),
            },
            _ => Err(failure(key, ConfigValueProblem::WrongType)),
        }
    }
}

/// A rule's word, refused outside its two words.
fn rule<T>(
    reader: &mut Reader<'_>,
    key: &'static str,
    parse: fn(&str) -> Option<T>,
) -> Result<T, ConfigFailure> {
    let word = reader.word(key)?;
    parse(&word).ok_or_else(|| failure(key, ConfigValueProblem::OutOfRange))
}

fn parse_start(token: &str) -> Option<RecoveryTriggerStart> {
    RecoveryTriggerStart::ALL
        .iter()
        .copied()
        .find(|start| start.as_str() == token)
}

fn parse_stage(token: &str) -> Option<RecoveryTriggerStage> {
    RecoveryTriggerStage::ALL
        .iter()
        .copied()
        .find(|stage| stage.as_str() == token)
}

fn parse_action(token: &str) -> Option<RecoveryRung> {
    RecoveryRung::ALL
        .iter()
        .copied()
        .find(|rung| rung.as_str() == token)
}

/// The codes and prefixes of a `codes` or `exclude_codes` list, each listed by the catalog
/// (§5.3): an unlisted code or a prefix no listed code starts with is `code_unknown`, a retired
/// code `code_retired`.
fn code_patterns(key: &'static str, entries: &[Value]) -> Result<Vec<CodePattern>, ConfigFailure> {
    entries
        .iter()
        .map(|entry| {
            let text = entry
                .as_str()
                .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
            let pattern = CodePattern::read(text);
            match &pattern {
                CodePattern::Exact(code) => match listed(code) {
                    Listed::Active => {}
                    Listed::Retired => return Err(failure(key, ConfigValueProblem::CodeRetired)),
                    Listed::Unlisted => return Err(failure(key, ConfigValueProblem::CodeUnknown)),
                },
                CodePattern::Prefix(prefix) => {
                    if prefix.is_empty() || !prefix_listed(prefix) {
                        return Err(failure(key, ConfigValueProblem::CodeUnknown));
                    }
                }
            }
            Ok(pattern)
        })
        .collect()
}

/// One element of `triggers` as written (§3.1).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerItemFile {
    id: String,
    stages: Vec<String>,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    codes: Vec<Value>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    window_ms: Option<u64>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    start_rung: Option<usize>,
    #[serde(default)]
    respect_cooldown: Option<bool>,
}

/// One element of `rungs` as written (§4.1, §4.2): its action and the bounds it names.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RungItemFile {
    action: String,
    #[serde(default)]
    configured_package_deadline_ms: Option<u64>,
    #[serde(default)]
    adb_probe_ms: Option<u64>,
    #[serde(default)]
    entry_hold_ms: Option<u64>,
    #[serde(default)]
    stop_bound_ms: Option<u64>,
    #[serde(default)]
    start_bound_ms: Option<u64>,
    #[serde(default)]
    readiness_window_ms: Option<u64>,
    #[serde(default)]
    readiness_backoff_ms: Option<Vec<u64>>,
    #[serde(default)]
    readiness_poll_ms: Option<u64>,
}

impl RungItemFile {
    /// Whether the element names a bound of another action than its own.
    fn names_foreign_bound(&self, action: RecoveryRung) -> bool {
        let return_home = self.configured_package_deadline_ms.is_some();
        let application = self.adb_probe_ms.is_some() || self.entry_hold_ms.is_some();
        let emulator = self.stop_bound_ms.is_some()
            || self.start_bound_ms.is_some()
            || self.readiness_window_ms.is_some()
            || self.readiness_backoff_ms.is_some()
            || self.readiness_poll_ms.is_some();
        match action {
            RecoveryRung::ReturnHome => application || emulator,
            RecoveryRung::ApplicationRestart => return_home || emulator,
            RecoveryRung::EmulatorRestart => return_home || application,
        }
    }

    /// The element with every bound it leaves out taken from `default`, the defaults file's
    /// rung of the same action (§4.1).
    fn or(self, default: &Self) -> Self {
        Self {
            action: self.action,
            configured_package_deadline_ms: self
                .configured_package_deadline_ms
                .or(default.configured_package_deadline_ms),
            adb_probe_ms: self.adb_probe_ms.or(default.adb_probe_ms),
            entry_hold_ms: self.entry_hold_ms.or(default.entry_hold_ms),
            stop_bound_ms: self.stop_bound_ms.or(default.stop_bound_ms),
            start_bound_ms: self.start_bound_ms.or(default.start_bound_ms),
            readiness_window_ms: self.readiness_window_ms.or(default.readiness_window_ms),
            readiness_backoff_ms: self
                .readiness_backoff_ms
                .or_else(|| default.readiness_backoff_ms.clone()),
            readiness_poll_ms: self.readiness_poll_ms.or(default.readiness_poll_ms),
        }
    }
}

/// The defaults file's rung of `action` (§4.1: a left-out bound's schema value).
fn shipped_rung(action: RecoveryRung) -> Option<RungItemFile> {
    member(shipped_json()?, RUNGS)?
        .as_array()?
        .iter()
        .filter_map(|item| serde_json::from_value::<RungItemFile>(item.clone()).ok())
        .find(|item| parse_action(&item.action) == Some(action))
}

/// The rungs (§4.1, §4.2), each bound checked against the device layer's own bounds and the
/// longest renewal (§4.3). A refusal names `config_key`: `rungs`, or (Workflow #398 RD-4) the
/// profile whose rungs these are.
fn rungs(
    items: Vec<Value>,
    margin_ms: u64,
    renewal_cap_ms: u64,
    context: &CheckContext,
    config_key: &str,
) -> Result<Vec<RungConfig>, ConfigFailure> {
    let invalid = || failure(config_key, ConfigValueProblem::RungsInvalid);
    let bound = || failure(config_key, ConfigValueProblem::BoundOutOfRange);
    let capped = |step_ms: u64| {
        if step_ms.saturating_add(margin_ms) <= renewal_cap_ms {
            Ok(())
        } else {
            Err(failure(config_key, ConfigValueProblem::RenewalCapExceeded))
        }
    };
    if !(1..=RecoveryRung::ALL.len()).contains(&items.len()) {
        return Err(invalid());
    }
    let mut actions = BTreeSet::new();
    let mut rungs = Vec::with_capacity(items.len());
    for item in items {
        let item = serde_json::from_value::<RungItemFile>(item).map_err(|_| invalid())?;
        let action = parse_action(&item.action).ok_or_else(invalid)?;
        if !actions.insert(action.as_str()) || item.names_foreign_bound(action) {
            return Err(invalid());
        }
        let default = shipped_rung(action).ok_or_else(invalid)?;
        let item = item.or(&default);
        let rung = match action {
            RecoveryRung::ReturnHome => {
                let deadline = item.configured_package_deadline_ms.ok_or_else(invalid)?;
                // The return-home rung's run renews its own key to its deadline and the
                // heartbeat bound, which stays within the longest renewal exactly when the
                // deadline is within a task's longest deadline.
                if !(1..=ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS).contains(&deadline) {
                    return Err(bound());
                }
                RungConfig::ReturnHome {
                    configured_package_deadline_ms: deadline,
                }
            }
            RecoveryRung::ApplicationRestart => {
                let adb_probe_ms = item.adb_probe_ms.ok_or_else(invalid)?;
                let entry_hold_ms = item.entry_hold_ms.ok_or_else(invalid)?;
                if adb_probe_ms < MIN_ADB_PROBE_MS
                    || entry_hold_ms
                        < adb_probe_ms.saturating_add(context.device.application_stop_ms)
                {
                    return Err(bound());
                }
                capped(entry_hold_ms)?;
                RungConfig::ApplicationRestart {
                    adb_probe_ms,
                    entry_hold_ms,
                }
            }
            RecoveryRung::EmulatorRestart => {
                let stop_bound_ms = item.stop_bound_ms.ok_or_else(invalid)?;
                let start_bound_ms = item.start_bound_ms.ok_or_else(invalid)?;
                let readiness_window_ms = item.readiness_window_ms.ok_or_else(invalid)?;
                let readiness_backoff_ms = item.readiness_backoff_ms.ok_or_else(invalid)?;
                let readiness_poll_ms = item.readiness_poll_ms.ok_or_else(invalid)?;
                let first_backoff = readiness_backoff_ms.first().copied();
                if stop_bound_ms < context.device.emulator_stop_ms
                    || start_bound_ms < context.device.emulator_start_ms
                    || readiness_backoff_ms.contains(&0)
                    || first_backoff.is_none_or(|first| readiness_window_ms < first)
                    || !(MIN_READINESS_POLL_MS..=readiness_window_ms).contains(&readiness_poll_ms)
                {
                    return Err(bound());
                }
                capped(stop_bound_ms)?;
                capped(start_bound_ms)?;
                capped(readiness_window_ms)?;
                RungConfig::EmulatorRestart {
                    stop_bound_ms,
                    start_bound_ms,
                    readiness_window_ms,
                    readiness_backoff_ms,
                    readiness_poll_ms,
                }
            }
        };
        rungs.push(rung);
    }
    Ok(rungs)
}

/// The trigger items (§3.1), their left-out members from `trigger_defaults`.
fn triggers(
    items: Vec<Value>,
    defaults: (RecoveryTriggerStart, usize, bool),
    rung_count: usize,
) -> Result<Vec<TriggerItem>, ConfigFailure> {
    let (default_start, default_start_rung, default_respect_cooldown) = defaults;
    let mut ids = BTreeSet::new();
    let mut triggers = Vec::with_capacity(items.len());
    for item in items {
        let item = serde_json::from_value::<TriggerItemFile>(item)
            .map_err(|_| failure(TRIGGERS, ConfigValueProblem::WrongType))?;
        if !valid_recovery_ladder_token(&item.id) {
            return Err(failure(TRIGGERS, ConfigValueProblem::WrongType));
        }
        if !ids.insert(item.id.clone()) {
            return Err(failure(TRIGGERS, ConfigValueProblem::DuplicateId));
        }
        let mut stages = Vec::with_capacity(item.stages.len());
        for token in &item.stages {
            let stage = parse_stage(token)
                .filter(|stage| {
                    // A2: the ladder's own preparation never triggers; a fence matches no item,
                    // and neither does a person's drill (Workflow #390 DRILL-1).
                    !matches!(
                        stage,
                        RecoveryTriggerStage::RecoveryPreparation
                            | RecoveryTriggerStage::ResourceFence
                            | RecoveryTriggerStage::Drill
                    )
                })
                .ok_or_else(|| failure(TRIGGERS, ConfigValueProblem::StageNotAllowed))?;
            if stages.contains(&stage) {
                return Err(failure(TRIGGERS, ConfigValueProblem::StageNotAllowed));
            }
            stages.push(stage);
        }
        if stages.is_empty() {
            return Err(failure(TRIGGERS, ConfigValueProblem::StageNotAllowed));
        }
        if item.classes.is_empty() && item.codes.is_empty() {
            return Err(failure(TRIGGERS, ConfigValueProblem::MatchMissing));
        }
        let classes = item
            .classes
            .iter()
            .map(|token| {
                parse_ladder_class(token)
                    .ok_or_else(|| failure(TRIGGERS, ConfigValueProblem::CodeUnknown))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let codes = code_patterns(TRIGGERS, &item.codes)?;
        let burst = match (
            stages.contains(&RecoveryTriggerStage::StabilityWarnings),
            item.count,
            item.window_ms,
        ) {
            (true, Some(count), Some(window_ms)) if count >= 1 && window_ms > 0 => {
                Some(BurstWindow { count, window_ms })
            }
            (false, None, None) => None,
            _ => return Err(failure(TRIGGERS, ConfigValueProblem::BurstInvalid)),
        };
        let start = match &item.start {
            Some(token) => parse_start(token)
                .ok_or_else(|| failure(TRIGGERS, ConfigValueProblem::OutOfRange))?,
            None => default_start,
        };
        let (start_rung, start_rung_key) = match item.start_rung {
            Some(start_rung) => (start_rung, TRIGGERS),
            None => (default_start_rung, DEFAULT_START_RUNG),
        };
        if !(1..=rung_count).contains(&start_rung) {
            return Err(failure(
                start_rung_key,
                ConfigValueProblem::StartRungOutOfRange,
            ));
        }
        triggers.push(TriggerItem {
            id: item.id,
            stages,
            classes,
            codes,
            burst,
            start,
            start_rung,
            respect_cooldown: item.respect_cooldown.unwrap_or(default_respect_cooldown),
        });
    }
    Ok(triggers)
}

/// Workflow #398 RD-4: one profile of `profiles` as written (model-398 §6.2).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    rungs: Vec<Value>,
}

/// Workflow #398 RD-4 (model-398 §6.6): the key a refusal of the profile `name` names, its
/// member of `profiles`.
fn profile_key(name: &str) -> String {
    format!("{PROFILES}.{name}")
}

/// Workflow #398 RD-4 (model-398 §6.2): the profiles, each name a token and each profile's
/// `rungs` checked as the section's are, a refusal naming the profile.
fn profiles(
    members: Map<String, Value>,
    margin_ms: u64,
    renewal_cap_ms: u64,
    context: &CheckContext,
) -> Result<Vec<LadderProfile>, ConfigFailure> {
    let mut profiles = Vec::with_capacity(members.len());
    for (name, profile) in members {
        if !valid_recovery_ladder_token(&name) {
            return Err(failure(PROFILES, ConfigValueProblem::WrongType));
        }
        let key = profile_key(&name);
        let profile = serde_json::from_value::<ProfileFile>(profile)
            .map_err(|_| failure(&key, ConfigValueProblem::WrongType))?;
        let rungs = rungs(profile.rungs, margin_ms, renewal_cap_ms, context, &key)?;
        profiles.push(LadderProfile { name, rungs });
    }
    Ok(profiles)
}

/// Workflow #398 RD-4 (model-398 §6.2): `kind_profiles`, each member a device kind naming one
/// of `profiles`; a kind outside the device kinds or a name no profile has is `out_of_range`.
fn kind_profiles(
    members: Map<String, Value>,
    profiles: &[LadderProfile],
) -> Result<Vec<(InstanceDeviceKind, String)>, ConfigFailure> {
    members
        .into_iter()
        .map(|(kind, name)| {
            let kind = serde_json::from_value::<InstanceDeviceKind>(Value::String(kind))
                .map_err(|_| failure(KIND_PROFILES, ConfigValueProblem::OutOfRange))?;
            let Value::String(name) = name else {
                return Err(failure(KIND_PROFILES, ConfigValueProblem::WrongType));
            };
            if profiles.is_empty() && !profiles.iter().any(|profile| profile.name == name) {
                return Err(failure(KIND_PROFILES, ConfigValueProblem::OutOfRange));
            }
            Ok((kind, name))
        })
        .collect()
}

/// Workflow #398 RD-4 (model-398 §6.6, `refuse`): every trigger item's start rung and
/// `trigger_defaults.start_rung` lie within every profile's rungs; the first profile one does
/// not fit is `start_rung_out_of_range` naming that profile. Under `exhaust` and `last_rung` a
/// start rung is checked against the section's `rungs` only, as D1c does.
fn starts_fit_profiles(
    rule: StartBeyondProfile,
    profiles: &[LadderProfile],
    triggers: &[TriggerItem],
    default_start_rung: usize,
) -> Result<(), ConfigFailure> {
    if rule != StartBeyondProfile::Refuse {
        return Ok(());
    }
    for profile in profiles {
        let count = profile.rungs.len();
        let count = count + RecoveryRung::ALL.len();
        if default_start_rung > count || triggers.iter().any(|item| item.start_rung > count) {
            return Err(failure(
                &profile_key(&profile.name),
                ConfigValueProblem::StartRungOutOfRange,
            ));
        }
    }
    Ok(())
}

impl LadderConfig {
    /// The section from `value` (the configuration in effect), checked whole (§5.3) with the
    /// facts of other crates in `context`. Any broken rule is `recovery_ladder_config_invalid`
    /// with the key it concerns and why (`config_value_problem`).
    pub fn parse(
        value: &dyn Fn(&str) -> Option<FactScalar>,
        context: &CheckContext,
    ) -> Result<Self, ConfigFailure> {
        let mut reader = Reader {
            value,
            seen: Map::new(),
        };
        let enabled = reader.boolean(ENABLED)?;
        let cooldown_secs = u32::try_from(reader.integer(COOLDOWN_SECS)?)
            .ok()
            .filter(|secs| (1..=InstanceStuckRecovery::MAX_COOLDOWN_SECS).contains(secs))
            .ok_or_else(|| failure(COOLDOWN_SECS, ConfigValueProblem::OutOfRange))?;
        let default_start = rule(&mut reader, DEFAULT_START, parse_start)?;
        let default_start_rung = usize::try_from(reader.integer(DEFAULT_START_RUNG)?)
            .map_err(|_| failure(DEFAULT_START_RUNG, ConfigValueProblem::OutOfRange))?;
        let default_respect_cooldown = reader.boolean(DEFAULT_RESPECT_COOLDOWN)?;
        let rules = LadderRules {
            direct_runs: rule(&mut reader, RULE_DIRECT_RUNS, DirectRuns::parse)?,
            preparation_superseded: rule(
                &mut reader,
                RULE_PREPARATION_SUPERSEDED,
                PreparationSuperseded::parse,
            )?,
            already_running: rule(&mut reader, RULE_ALREADY_RUNNING, AlreadyRunning::parse)?,
            cooldown_opens_on: rule(&mut reader, RULE_COOLDOWN_OPENS_ON, CooldownOpensOn::parse)?,
            entry_unavailable: rule(&mut reader, RULE_ENTRY_UNAVAILABLE, EntryUnavailable::parse)?,
        };
        let start_beyond_profile = rule(
            &mut reader,
            RULE_START_BEYOND_PROFILE,
            StartBeyondProfile::parse,
        )?;
        let claim_ttl_ms = reader.duration(CLAIM_TTL_MS)?;
        let margin_ms = reader.duration(MARGIN_MS)?;
        // §4.3: a claim's TTL above the heartbeat bound and within the longest renewal (the
        // scheduler refuses any other as a fatal configuration error), and no shorter than the
        // margin.
        let heartbeat_ms = context.maximum_client_heartbeat_interval_ms;
        let renewal_cap_ms =
            ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS.saturating_add(heartbeat_ms);
        if claim_ttl_ms <= heartbeat_ms || claim_ttl_ms > renewal_cap_ms || claim_ttl_ms < margin_ms
        {
            return Err(failure(
                CLAIM_TTL_MS,
                ConfigValueProblem::ClaimTtlOutOfRange,
            ));
        }
        let rung_items = reader.list(RUNGS)?;
        let rungs = rungs(rung_items, margin_ms, renewal_cap_ms, context, RUNGS)?;
        if !(1..=rungs.len()).contains(&default_start_rung) {
            return Err(failure(
                DEFAULT_START_RUNG,
                ConfigValueProblem::StartRungOutOfRange,
            ));
        }
        let trigger_items = reader.list(TRIGGERS)?;
        let triggers = triggers(
            trigger_items,
            (default_start, default_start_rung, default_respect_cooldown),
            rungs.len(),
        )?;
        let exclude_items = reader.list(EXCLUDE_CODES)?;
        let exclude_codes = code_patterns(EXCLUDE_CODES, &exclude_items)?;
        // Workflow #398 RD-4 (model-398 §6.2, §6.6).
        let profile_members = reader.object(PROFILES)?;
        let profiles = profiles(profile_members, margin_ms, renewal_cap_ms, context)?;
        let kind_members = reader.object(KIND_PROFILES)?;
        let kind_profiles = kind_profiles(kind_members, &profiles)?;
        starts_fit_profiles(
            start_beyond_profile,
            &profiles,
            &triggers,
            default_start_rung,
        )?;
        let digest = format!(
            "{:x}",
            Sha256::digest(canonical_json(&Value::Object(reader.seen)).as_bytes())
        );
        Ok(Self {
            enabled,
            cooldown_secs,
            triggers,
            exclude_codes,
            rules,
            rungs,
            hold: HoldConfig {
                claim_ttl_ms,
                margin_ms,
            },
            default_start_rung,
            default_respect_cooldown,
            start_beyond_profile,
            profiles,
            kind_profiles,
            instance_profiles: BTreeMap::new(),
            digest,
        })
    }

    /// Workflow #398 RD-4 (model-398 §6.2): binds the profile the instance whose registry id is
    /// `instance_id` names in effect (`instance.<id>.recovery_ladder_profile`, `value`). A name
    /// no profile of the section has is refused as the section is (`out_of_range`, naming the
    /// instance's key).
    pub(crate) fn bind_instance_profile(
        &mut self,
        instance_id: &str,
        value: &FactScalar,
    ) -> Result<(), ConfigFailure> {
        let key = instance_key(instance_id, RECOVERY_LADDER_PROFILE);
        let FactScalar::String(name) = value else {
            return Err(failure(&key, ConfigValueProblem::WrongType));
        };
        if !self.profiles.iter().any(|profile| profile.name == *name) {
            return Err(failure(&key, ConfigValueProblem::OutOfRange));
        }
        self.instance_profiles
            .insert(instance_id.to_owned(), name.clone());
        Ok(())
    }

    /// Workflow #398 RD-4 (model-398 §6.2): the profile the instance whose registry id is
    /// `instance_id` climbs by: the one its own `recovery_ladder_profile` names, else the one
    /// `kind_profiles` gives its device kind; `None` when neither names one, and it climbs the
    /// section's `rungs`.
    pub fn profile_for(
        &self,
        instance_id: &str,
        kind: InstanceDeviceKind,
    ) -> Option<&LadderProfile> {
        let name = self.instance_profiles.get(instance_id).or_else(|| {
            self.kind_profiles
                .iter()
                .find(|(named, _)| *named == kind)
                .map(|(_, name)| name)
        })?;
        self.profiles.iter().find(|profile| profile.name == *name)
    }

    /// The SHA-256 of the section's values, canonical JSON, as 64 lowercase hexadecimal digits
    /// (§4.7 `config_digest`).
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub const fn cooldown_ms(&self) -> u64 {
        (self.cooldown_secs as u64).saturating_mul(1_000)
    }

    /// §3.1: the exclusion list first, then the first item, in order, that names `stage` and
    /// either the code `spelling` (by code) or its class (by class). A code the catalog does
    /// not list has class `none`.
    pub fn classify(&self, stage: RecoveryTriggerStage, spelling: &str) -> TriggerVerdict<'_> {
        if self
            .exclude_codes
            .iter()
            .any(|pattern| pattern.matches(spelling))
        {
            return TriggerVerdict::Excluded;
        }
        let class = ladder_class_of(spelling).unwrap_or(LadderClass::NotApplicable);
        for item in self
            .triggers
            .iter()
            .filter(|item| item.stages.contains(&stage))
        {
            if item.codes.iter().any(|pattern| pattern.matches(spelling)) {
                return TriggerVerdict::Matched {
                    item,
                    by: MatchedBy::Code,
                };
            }
            if item.classes.contains(&class) {
                return TriggerVerdict::Matched {
                    item,
                    by: MatchedBy::Class(class),
                };
            }
        }
        TriggerVerdict::Unmatched
    }

    /// The configured rung of `action`, when `rungs` has it.
    pub fn rung(&self, action: RecoveryRung) -> Option<&RungConfig> {
        self.rungs.iter().find(|rung| rung.action() == action)
    }
}
