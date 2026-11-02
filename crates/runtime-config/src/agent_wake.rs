// SPDX-License-Identifier: AGPL-3.0-only

//! The `agent_wake` section (Workflow #432 AGT-1a; `contracts/agent-wake.md`; Alice's words of
//! 2026-11-02: the Runtime itself sends a registered channel a message of a few dozen characters,
//! and the body is a file in the program's directory): the limit of the message, the bounds of the
//! body, the pace and the quiet of the delivery, and the rules that say which outcome wakes which
//! channel. A channel's address is not configuration: the channel registry holds it, and a rule's
//! `to` is only a name here.
//!
//! The section is optional and off by default, and it is whole or absent: every row's default is
//! none, so with no `agent_wake` member in the configuration file there are no keys and no rows
//! (an instruction naming a key is `config_key_unknown`), and neither the defaults file nor this
//! crate holds a value of it. A section written in part is refused, naming the first key it leaves
//! out.
//!
//! - **Not settable while the Runtime runs:** `body.dir` (open rows name files under the old
//!   folder) and `delivery.queue_capacity` (sized at start).
//! - **Refusals:** a value of the wrong type or outside its key's range is `config_value_invalid`
//!   as the file layer is rendered, as for every section. A broken rule of the section is
//!   `agent_wake_config_invalid` naming the key (`config_key`) and the rule
//!   (`config_value_problem`) at start and in `check-config`; the same values sent by an
//!   instruction are `config_value_invalid` with the same key and problem.
//! - **What only the assembly can check:** the instance aliases a rule names
//!   ([`AgentWakeConfig::refuse_unknown_instances`]) and whether each registered channel's message
//!   fits ([`refuse_channels_that_cannot_fit`]), since they need the configured instances, the
//!   registry and the state root. A limit an instruction sets below a channel's floor passes the
//!   section check; the send to that channel is refused, loudly, and never carries a cut path.
//!
//! This crate only parses and checks the section; AGT-1a sends nothing and starts no thread.

use crate::codes::{ConfigCode, ConfigValueProblem};
use crate::effective::ConfigFailure;
use crate::recovery_ladder::{Listed, listed};
use crate::scheduling::{BlackoutEntry, BlackoutList, blackout_entries};
use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use actingcommand_contract::FactScalar;
use actingcommand_contract::outcome::BlackoutSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// The section's name: the first segment of its keys.
pub const SECTION: &str = "agent_wake";

/// Whether wakes are delivered.
pub const ENABLED: &str = "agent_wake.enabled";
/// The longest message, in Unicode scalars, its header included.
pub const SUMMARY_MAX_CHARS: &str = "agent_wake.summary.max_chars";
/// The folder of the body files, below the state root.
pub const BODY_DIR: &str = "agent_wake.body.dir";
/// The longest body file.
pub const BODY_MAX_BYTES: &str = "agent_wake.body.max_bytes";
/// How long body files are kept.
pub const BODY_RETENTION_DAYS: &str = "agent_wake.body.retention_days";
/// The most files kept.
pub const BODY_MAX_FILES: &str = "agent_wake.body.max_files";
/// The bounded queue between the append path and the worker.
pub const QUEUE_CAPACITY: &str = "agent_wake.delivery.queue_capacity";
/// How long the worker gathers candidates of one instance.
pub const COALESCE_MS: &str = "agent_wake.delivery.coalesce_ms";
/// How long one send may take.
pub const SEND_DEADLINE_MS: &str = "agent_wake.delivery.send_deadline_ms";
/// The wait before a deferred wake is tried again.
pub const RETRY_INTERVAL_MS: &str = "agent_wake.delivery.retry_interval_ms";
/// How long a wake may stay deferred.
pub const GIVE_UP_MS: &str = "agent_wake.delivery.give_up_ms";
/// The least time between two messages to one channel.
pub const CHANNEL_GAP_MS: &str = "agent_wake.delivery.channel_gap_ms";
/// The newest wakes `host.agent_wake` keeps.
pub const HISTORY_ROWS: &str = "agent_wake.delivery.history_rows";
/// The wait after a panic of the worker.
pub const PANIC_BACKOFF_MS: &str = "agent_wake.delivery.panic_backoff_ms";
/// The longest that wait grows to.
pub const PANIC_BACKOFF_MAX_MS: &str = "agent_wake.delivery.panic_backoff_max_ms";
/// The times no message is sent, written as blackout entries.
pub const QUIET_WINDOWS: &str = "agent_wake.quiet.windows";
/// No message is sent before this time; 0 holds nothing.
pub const QUIET_HOLD_UNTIL_UNIX_MS: &str = "agent_wake.quiet.hold_until_unix_ms";
/// The most wakes of one local day, over all rules.
pub const BUDGET_DAILY_CAP: &str = "agent_wake.budget.daily_cap";
/// The rules that say which outcome wakes which channel.
pub const RULES: &str = "agent_wake.rules";

/// A recovery ladder ended with nothing left to try.
pub const KIND_LADDER_EXHAUSTED: &str = "ladder_exhausted";
/// An instance waits for a person behind its fence.
pub const KIND_INSTANCE_FENCED: &str = "instance_fenced";
/// A run failed with a code the rule lists.
pub const KIND_RUN_FAILED: &str = "run_failed";
/// The daily results summary. It has no emitter yet (AGT-2b).
pub const KIND_DAILY_SUMMARY: &str = "daily_summary";
/// A person's test of the delivery.
pub const KIND_DELIVERY_TEST: &str = "delivery_test";
/// The kinds a rule may name: configuration and body words, closed here and not the ledger's
/// `AgentWakeKind`.
pub const KINDS: &[&str] = &[
    KIND_LADDER_EXHAUSTED,
    KIND_INSTANCE_FENCED,
    KIND_RUN_FAILED,
    KIND_DAILY_SUMMARY,
    KIND_DELIVERY_TEST,
];

/// The word of a rule's `instances` that matches every instance.
pub const ALL_INSTANCES: &str = "*";

/// The folders of the state root a body folder may not be in. Compared without case.
const RESERVED_DIRS: &[&str] = &["ledger", "artifacts", "release-blobs", "kept"];

/// The most rules. Schema.
pub const MAX_RULES: usize = 64;
/// The longest channel name in Unicode scalars, as the registry refuses a longer one. Schema.
pub const MAX_CHANNEL_CHARS: usize = 16;
/// The longest rule id in bytes. Schema.
pub const MAX_RULE_ID_BYTES: usize = 64;
/// The most codes one rule lists. Schema.
pub const MAX_RULE_CODES: usize = 64;
/// The longest a rule's cool-down may be: a week. Schema.
pub const MAX_COOLDOWN_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// A message with no alias, apart from its channel and body folder: `ActingCommand to ` (17), the
/// line break (1), a kind word with ` 正文 ` (8) and the longest file name,
/// `w<13 digits>-nn.json` (22). `contracts/agent-wake.md` gives the format.
pub const MESSAGE_FIXED_CHARS: usize = 17 + 1 + 8 + 22;

const fn row(
    name: &'static str,
    value_type: ValueType,
    range: KeyRange,
    settable: Settable,
) -> KeyRow {
    KeyRow {
        name: KeyName::Exact(name),
        file_path: FilePath::Key,
        value_type,
        range,
        default: DefaultSource::None,
        settable,
        reader: "AGT-1b",
        since: "0.12.37",
    }
}

const fn milliseconds(name: &'static str, min: i64, max: i64) -> KeyRow {
    row(
        name,
        ValueType::DurationMs,
        KeyRange::Integer { min, max },
        Settable::Yes,
    )
}

const fn integer(name: &'static str, min: i64, max: i64) -> KeyRow {
    row(
        name,
        ValueType::Integer,
        KeyRange::Integer { min, max },
        Settable::Yes,
    )
}

/// One row per key of the section, each with no default: the section is whole or absent.
pub(crate) static ROWS: &[KeyRow] = &[
    row(ENABLED, ValueType::Boolean, KeyRange::Any, Settable::Yes),
    // The fixed characters of a message are the least that can ever fit.
    integer(SUMMARY_MAX_CHARS, MESSAGE_FIXED_CHARS as i64, 2_048),
    row(
        BODY_DIR,
        ValueType::String,
        KeyRange::Any,
        Settable::No(NotSettable::ReadAtAssembly),
    ),
    integer(BODY_MAX_BYTES, 4_096, 1_048_576),
    integer(BODY_RETENTION_DAYS, 1, 365),
    integer(BODY_MAX_FILES, 8, 65_536),
    row(
        QUEUE_CAPACITY,
        ValueType::Integer,
        KeyRange::Integer { min: 1, max: 1_024 },
        Settable::No(NotSettable::ReadAtAssembly),
    ),
    milliseconds(COALESCE_MS, 0, 60_000),
    milliseconds(SEND_DEADLINE_MS, 1_000, 120_000),
    milliseconds(RETRY_INTERVAL_MS, 1_000, 86_400_000),
    milliseconds(GIVE_UP_MS, 60_000, 2_419_200_000),
    milliseconds(CHANNEL_GAP_MS, 0, 86_400_000),
    // `MAX_RUNTIME_FACT_RECORD_ROWS`: `host.agent_wake` is one record list.
    integer(HISTORY_ROWS, 1, 256),
    milliseconds(PANIC_BACKOFF_MS, 1_000, 86_400_000),
    milliseconds(PANIC_BACKOFF_MAX_MS, 1_000, 86_400_000),
    row(QUIET_WINDOWS, ValueType::List, KeyRange::Any, Settable::Yes),
    integer(QUIET_HOLD_UNTIL_UNIX_MS, 0, i64::MAX),
    integer(BUDGET_DAILY_CAP, 1, 256),
    row(RULES, ValueType::List, KeyRange::Any, Settable::Yes),
];

/// `summary` of a file's section: every member may be left out to the file's decoding, none is
/// unknown, and the section's check requires each.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeSummaryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_chars: Option<Value>,
}

/// `body` of a file's section.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeBodyFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dir: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retention_days: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_files: Option<Value>,
}

/// `delivery` of a file's section.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeDeliveryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_capacity: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    coalesce_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    send_deadline_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_interval_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    give_up_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    channel_gap_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    history_rows: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    panic_backoff_ms: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    panic_backoff_max_ms: Option<Value>,
}

/// `quiet` of a file's section.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeQuietFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_until_unix_ms: Option<Value>,
}

/// `budget` of a file's section.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeBudgetFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daily_cap: Option<Value>,
}

/// The section as the configuration file writes it.
// Interface anchor `actingd-config` revision 24 (contracts/component-interfaces.md): a Runtime
// before AGT-1a refuses a file that names the section, because the file denies unknown members; a
// file that does not name it keeps its revision.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<AgentWakeSummaryFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<AgentWakeBodyFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<AgentWakeDeliveryFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quiet: Option<AgentWakeQuietFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<AgentWakeBudgetFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rules: Option<Value>,
}

/// Where the body files live and how long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyConfig {
    pub dir: String,
    pub max_bytes: u64,
    pub retention_days: u64,
    pub max_files: u64,
}

/// The pace of the delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryConfig {
    pub queue_capacity: u64,
    pub coalesce_ms: u64,
    pub send_deadline_ms: u64,
    pub retry_interval_ms: u64,
    pub give_up_ms: u64,
    pub channel_gap_ms: u64,
    pub history_rows: u64,
    pub panic_backoff_ms: u64,
    pub panic_backoff_max_ms: u64,
}

/// When no message is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietConfig {
    pub windows: Vec<BlackoutEntry>,
    pub hold_until_unix_ms: u64,
}

/// The instances a rule covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeInstances {
    /// `"*"`.
    All,
    /// The aliases it lists.
    Listed(Vec<String>),
}

/// One rule: which outcome wakes which channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRule {
    pub id: String,
    pub kinds: Vec<String>,
    pub codes: Vec<String>,
    pub instances: WakeInstances,
    pub to: String,
    pub cooldown_ms: u64,
    pub daily_cap: u64,
}

/// The section in effect, checked whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWakeConfig {
    pub enabled: bool,
    pub summary_max_chars: u64,
    pub body: BodyConfig,
    pub delivery: DeliveryConfig,
    pub quiet: QuietConfig,
    pub budget_daily_cap: u64,
    pub rules: Vec<WakeRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleMatchFile {
    kinds: Vec<String>,
    #[serde(default)]
    codes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    id: String,
    on: RuleMatchFile,
    instances: Value,
    to: String,
    cooldown_ms: u64,
    daily_cap: u64,
}

/// `agent_wake_config_invalid` for `key`, for `problem`.
pub fn invalid(key: impl Into<String>, problem: ConfigValueProblem) -> ConfigFailure {
    ConfigFailure::with_problem(ConfigCode::AgentWakeConfigInvalid, key, problem)
}

/// A key's value in effect, `required_missing` when the section does not carry it.
fn scalar(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
) -> Result<FactScalar, ConfigFailure> {
    value(key).ok_or_else(|| invalid(key, ConfigValueProblem::RequiredMissing))
}

fn text(value: &dyn Fn(&str) -> Option<FactScalar>, key: &str) -> Result<String, ConfigFailure> {
    match scalar(value, key)? {
        FactScalar::String(text) => Ok(text),
        _ => Err(invalid(key, ConfigValueProblem::WrongType)),
    }
}

fn count(value: &dyn Fn(&str) -> Option<FactScalar>, key: &str) -> Result<u64, ConfigFailure> {
    match scalar(value, key)? {
        FactScalar::Integer(number) => {
            u64::try_from(number).map_err(|_| invalid(key, ConfigValueProblem::OutOfRange))
        }
        _ => Err(invalid(key, ConfigValueProblem::WrongType)),
    }
}

fn duration(value: &dyn Fn(&str) -> Option<FactScalar>, key: &str) -> Result<u64, ConfigFailure> {
    match scalar(value, key)? {
        FactScalar::DurationMs(ms) => Ok(ms),
        _ => Err(invalid(key, ConfigValueProblem::WrongType)),
    }
}

/// A list key's entries: it is reported as canonical JSON text.
fn list(
    value: &dyn Fn(&str) -> Option<FactScalar>,
    key: &str,
) -> Result<Vec<Value>, ConfigFailure> {
    serde_json::from_str::<Vec<Value>>(&text(value, key)?)
        .map_err(|_| invalid(key, ConfigValueProblem::WrongType))
}

/// `body.dir` is a relative folder of plain names below the state root, outside the folders the
/// Runtime keeps its own material in.
fn check_body_dir(dir: &str) -> Result<(), ConfigFailure> {
    let segments = dir.split(['\\', '/']).collect::<Vec<_>>();
    let plain = segments.iter().all(|segment| {
        !segment.is_empty()
            && !matches!(*segment, "." | "..")
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    });
    if !plain {
        return Err(invalid(BODY_DIR, ConfigValueProblem::OutOfRange));
    }
    if RESERVED_DIRS
        .iter()
        .any(|reserved| segments[0].eq_ignore_ascii_case(reserved))
    {
        return Err(invalid(BODY_DIR, ConfigValueProblem::DirReserved));
    }
    Ok(())
}

/// `quiet.windows` by the blackout entries' parser and bound; a bad entry is this section's code,
/// with the entry's member as the failure's pointer.
fn parse_quiet_windows(items: Vec<Value>) -> Result<Vec<BlackoutEntry>, ConfigFailure> {
    blackout_entries(
        &BlackoutList {
            config_key: QUIET_WINDOWS,
            source: BlackoutSource::Host,
            pointer: "/agent_wake/quiet/windows".to_owned(),
        },
        items,
    )
    .map_err(|failure| {
        let refused = invalid(QUIET_WINDOWS, ConfigValueProblem::SectionCheck);
        match failure.field() {
            Some(field) => refused.with_pointer(field),
            None => refused,
        }
    })
}

/// A rule's `instances`: `"*"` or a list of at least one alias. Whether each alias is configured
/// is the assembly's check ([`AgentWakeConfig::refuse_unknown_instances`]).
fn parse_instances(instances: Value) -> Result<WakeInstances, ConfigFailure> {
    match instances {
        Value::String(word) if word == ALL_INSTANCES => Ok(WakeInstances::All),
        Value::Array(_) => {
            let aliases = serde_json::from_value::<Vec<String>>(instances)
                .map_err(|_| invalid(RULES, ConfigValueProblem::WrongType))?;
            if aliases.is_empty() {
                return Err(invalid(RULES, ConfigValueProblem::InstanceUnknown));
            }
            Ok(WakeInstances::Listed(aliases))
        }
        _ => Err(invalid(RULES, ConfigValueProblem::InstanceUnknown)),
    }
}

/// One entry of `rules`. Its `to` is a channel name of the registry's shape; whether the registry
/// lists it is a note at the assembly, and a failed send at run time.
fn parse_rule(entry: Value) -> Result<WakeRule, ConfigFailure> {
    let file = serde_json::from_value::<RuleFile>(entry)
        .map_err(|_| invalid(RULES, ConfigValueProblem::WrongType))?;
    let id_valid = !file.id.is_empty()
        && file.id.len() <= MAX_RULE_ID_BYTES
        && file
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    let to_valid = (1..=MAX_CHANNEL_CHARS).contains(&file.to.chars().count())
        && !file.to.chars().any(char::is_control);
    if !id_valid
        || !to_valid
        || file.on.codes.len() > MAX_RULE_CODES
        || file.cooldown_ms > MAX_COOLDOWN_MS
        || file.daily_cap == 0
    {
        return Err(invalid(RULES, ConfigValueProblem::OutOfRange));
    }
    if file.on.kinds.is_empty() {
        return Err(invalid(RULES, ConfigValueProblem::MatchMissing));
    }
    if file
        .on
        .kinds
        .iter()
        .any(|kind| !KINDS.contains(&kind.as_str()))
    {
        return Err(invalid(RULES, ConfigValueProblem::KindUnknown));
    }
    // A failed run wakes only for the codes the rule lists: no list, no wake.
    if file.on.codes.is_empty() && file.on.kinds.iter().any(|kind| kind == KIND_RUN_FAILED) {
        return Err(invalid(RULES, ConfigValueProblem::MatchMissing));
    }
    for code in &file.on.codes {
        match listed(code) {
            Listed::Active => {}
            Listed::Retired => return Err(invalid(RULES, ConfigValueProblem::CodeRetired)),
            Listed::Unlisted => return Err(invalid(RULES, ConfigValueProblem::CodeUnknown)),
        }
    }
    Ok(WakeRule {
        id: file.id,
        kinds: file.on.kinds,
        codes: file.on.codes,
        instances: parse_instances(file.instances)?,
        to: file.to,
        cooldown_ms: file.cooldown_ms,
        daily_cap: file.daily_cap,
    })
}

fn parse_rules(entries: Vec<Value>) -> Result<Vec<WakeRule>, ConfigFailure> {
    if entries.len() > MAX_RULES {
        return Err(invalid(RULES, ConfigValueProblem::OutOfRange));
    }
    let rules = entries
        .into_iter()
        .map(parse_rule)
        .collect::<Result<Vec<_>, _>>()?;
    let ids = rules
        .iter()
        .map(|rule| rule.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != rules.len() {
        return Err(invalid(RULES, ConfigValueProblem::DuplicateId));
    }
    Ok(rules)
}

/// The floor of `channel`'s message: its longest form with no alias, for a body folder of
/// `body_folder` (`<state root>\<dir>\`). The kind word and the path are never cut, so nothing
/// shorter can carry them.
pub fn message_floor_chars(channel: &str, body_folder: &str) -> usize {
    MESSAGE_FIXED_CHARS + channel.chars().count() + body_folder.chars().count()
}

/// The body folder as a message writes it: `<state root>\<dir>\`.
pub fn body_folder_text(state_root: &Path, dir: &str) -> String {
    format!("{}\\{dir}\\", state_root.display())
}

/// What `check-config` guarantees: every registered channel's message fits `max_chars`. Otherwise
/// `agent_wake_config_invalid` naming `summary.max_chars` (`message_floor`).
pub fn refuse_channels_that_cannot_fit<'a>(
    max_chars: u64,
    state_root: &Path,
    dir: &str,
    channels: impl IntoIterator<Item = &'a str>,
) -> Result<(), ConfigFailure> {
    let body_folder = body_folder_text(state_root, dir);
    let fits = |channel: &str| {
        u64::try_from(message_floor_chars(channel, &body_folder))
            .is_ok_and(|floor| floor <= max_chars)
    };
    if channels.into_iter().all(fits) {
        Ok(())
    } else {
        Err(invalid(SUMMARY_MAX_CHARS, ConfigValueProblem::MessageFloor))
    }
}

impl AgentWakeConfig {
    /// The section from `value` (the configuration in effect), checked whole (module doc).
    pub fn parse(value: &dyn Fn(&str) -> Option<FactScalar>) -> Result<Self, ConfigFailure> {
        let enabled = match scalar(value, ENABLED)? {
            FactScalar::Boolean(enabled) => enabled,
            _ => return Err(invalid(ENABLED, ConfigValueProblem::WrongType)),
        };
        let dir = text(value, BODY_DIR)?;
        check_body_dir(&dir)?;
        let delivery = DeliveryConfig {
            queue_capacity: count(value, QUEUE_CAPACITY)?,
            coalesce_ms: duration(value, COALESCE_MS)?,
            send_deadline_ms: duration(value, SEND_DEADLINE_MS)?,
            retry_interval_ms: duration(value, RETRY_INTERVAL_MS)?,
            give_up_ms: duration(value, GIVE_UP_MS)?,
            channel_gap_ms: duration(value, CHANNEL_GAP_MS)?,
            history_rows: count(value, HISTORY_ROWS)?,
            panic_backoff_ms: duration(value, PANIC_BACKOFF_MS)?,
            panic_backoff_max_ms: duration(value, PANIC_BACKOFF_MAX_MS)?,
        };
        if delivery.panic_backoff_ms > delivery.panic_backoff_max_ms {
            return Err(invalid(PANIC_BACKOFF_MS, ConfigValueProblem::SectionCheck));
        }
        let budget_daily_cap = count(value, BUDGET_DAILY_CAP)?;
        // Cooldowns and day counts are read back from `host.agent_wake`, so the rows it keeps must
        // hold a whole day's wakes.
        if budget_daily_cap > delivery.history_rows {
            return Err(invalid(BUDGET_DAILY_CAP, ConfigValueProblem::CapsExceed));
        }
        let rules = parse_rules(list(value, RULES)?)?;
        if rules.iter().any(|rule| rule.daily_cap > budget_daily_cap) {
            return Err(invalid(RULES, ConfigValueProblem::CapsExceed));
        }
        Ok(Self {
            enabled,
            summary_max_chars: count(value, SUMMARY_MAX_CHARS)?,
            body: BodyConfig {
                dir,
                max_bytes: count(value, BODY_MAX_BYTES)?,
                retention_days: count(value, BODY_RETENTION_DAYS)?,
                max_files: count(value, BODY_MAX_FILES)?,
            },
            delivery,
            quiet: QuietConfig {
                windows: parse_quiet_windows(list(value, QUIET_WINDOWS)?)?,
                hold_until_unix_ms: count(value, QUIET_HOLD_UNTIL_UNIX_MS)?,
            },
            budget_daily_cap,
            rules,
        })
    }

    /// What only actingd's assembly can check, at start and in `check-config`: each alias a rule
    /// lists is a configured instance's (`instance_unknown`).
    pub fn refuse_unknown_instances<'a>(
        &self,
        aliases: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), ConfigFailure> {
        let aliases = aliases.into_iter().collect::<BTreeSet<_>>();
        for rule in &self.rules {
            if let WakeInstances::Listed(listed) = &rule.instances
                && listed.iter().any(|alias| !aliases.contains(alias.as_str()))
            {
                return Err(invalid(RULES, ConfigValueProblem::InstanceUnknown));
            }
        }
        Ok(())
    }
}
