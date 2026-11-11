// SPDX-License-Identifier: AGPL-3.0-only

//! The operation table (Workflow #434; `contracts/lab-two-entries.md` §9): the file
//! `contracts/lab-operations.json`, schema `actingcommand.lab-operations.v1`, read into typed
//! rows. It is the one source of the Lab's operations: one row per CLI form, MCP tool and serve
//! call (§9.1). This build embeds the file ([`operation_table`]); [`read_operation_table`]
//! reads a copy from disk.
//!
//! Reading checks the members and their words: every row member §9.1 lists is present, none
//! other is, and each word is one of its vocabulary (a request kind one of
//! [`RuntimeOperationKind::ALL`], an initiator one of the six). A table that cannot be used is
//! a registered failure: a file that cannot be opened or read `foreign_os_error` with its `path`
//! and `io_op`, a larger one than [`MAX_TABLE_BYTES`] `mcp_file_too_large`, text the decoder
//! refuses `foreign_data_error` with the decoder's message and the `path`.

use crate::initiator::Initiator;
use actingcommand_contract::RuntimeOperationKind;
use actingcommand_contract::outcome::Outcome;
use serde::Deserialize;
use serde::de::{self, Deserializer, Unexpected};
use std::fmt;
use std::path::Path;

/// The table's schema; it changes only when a member's meaning changes (§9.4).
pub const SCHEMA_VERSION: &str = "actingcommand.lab-operations.v1";
/// The table's place in the repository, from its root.
pub const TABLE_PATH: &str = "contracts/lab-operations.json";
/// The largest table file read.
pub const MAX_TABLE_BYTES: u64 = 1024 * 1024;
/// Workflow #434 LAB-5E: the longest `help` of a row, in bytes (§9.1).
pub const MAX_HELP_BYTES: usize = 160;
/// The table this build embeds.
const TABLE_TEXT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../contracts/lab-operations.json"
));

/// The table this build embeds, read into typed rows.
pub fn operation_table() -> Result<OperationTable, Outcome> {
    OperationTable::parse(TABLE_TEXT.as_bytes())
        .map_err(|error| crate::foreign_data_error(&error).path(TABLE_PATH))
}

/// The table file at `path`, read whole (at most [`MAX_TABLE_BYTES`]) into typed rows.
pub fn read_operation_table(path: &Path) -> Result<OperationTable, Outcome> {
    let bytes = crate::overview::read_bounded(path, MAX_TABLE_BYTES)?;
    OperationTable::parse(&bytes).map_err(|error| crate::foreign_data_error(&error).path(path))
}

/// The operation table (§9.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationTable {
    /// [`SCHEMA_VERSION`]; another schema is refused.
    #[serde(deserialize_with = "schema_version")]
    pub schema_version: String,
    /// The contract the table belongs to.
    pub contract: String,
    /// The six initiator words (§4).
    pub initiators: Vec<InitiatorRow>,
    /// The Lab's modules and front ends (§8).
    pub modules: Vec<ModuleRow>,
    /// The UI list (§3).
    pub ui_kinds: Vec<UiKindRow>,
    /// Every request kind no operation sends.
    pub not_exposed: Vec<NotExposedRow>,
    /// One row per operation.
    pub operations: Vec<Operation>,
}

impl OperationTable {
    /// The table in `bytes` (UTF-8 JSON); the decoder's refusal, which names the member.
    pub fn parse(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The operation named `name` (its CLI words).
    pub fn operation(&self, name: &str) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|operation| operation.name == name)
    }
}

/// An initiator word and its entry (§4.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitiatorRow {
    pub word: Initiator,
    pub entry: Entry,
    pub who: String,
}

/// A module of the Lab or a front end (§8.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRow {
    /// An operation module (`core`, `debugging`, `rescue`, `authoring`) or a front end (`cli`,
    /// `mcp`, `serve`, `watchdog`).
    pub id: String,
    pub role: ModuleRole,
    pub program: String,
    pub detachable: bool,
    pub how: String,
    /// A front end's initiator, as text (`serve` takes its program's).
    pub initiator: Option<String>,
}

/// What a module row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleRole {
    /// A module that serves operations.
    Operations,
    /// A program that reaches the operations.
    FrontEnd,
}

/// A request kind on the UI list (§3).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiKindRow {
    #[serde(deserialize_with = "request_kind")]
    pub kind: RuntimeOperationKind,
    pub class: RequestClass,
    /// Whether the console sends it today.
    pub console_today: bool,
    pub rule: Option<String>,
    /// The slice that takes it off the list (a transitional kind).
    pub until: Option<String>,
}

/// Whether a kind reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestClass {
    Read,
    Write,
}

/// A request kind no operation sends.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotExposedRow {
    #[serde(deserialize_with = "request_kind")]
    pub kind: RuntimeOperationKind,
    pub reason: NotExposedReason,
    pub note: String,
}

/// Why no operation sends a kind (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotExposedReason {
    /// Every connection's first requests (`Health`, `BindChannel`).
    Handshake,
    /// LAB-6 deletes the kind, and its row leaves with it.
    Deleted,
    /// No front end sends it today; the `lab` entry still takes it.
    Unsent,
}

/// One operation: one CLI form, one MCP tool, one serve call (§9.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    /// The CLI words, unique.
    pub name: String,
    pub module: OperationModule,
    pub params: Vec<Param>,
    /// The source command whose further flags carry over unchanged; since Workflow #434 LAB-5A
    /// only on a row no program offers yet (§9.2).
    pub params_more: Option<String>,
    /// Workflow #434 LAB-5E: what the operation does, in one sentence of its own, shared by the
    /// command line's help and the MCP tool's description (§9.1); every offered row has one.
    pub help: Option<String>,
    /// What the operation answers.
    pub output: String,
    /// Whether it changes anything (MCP's `readOnlyHint` is its negation).
    pub writes: bool,
    /// §5; MCP's `destructiveHint` is `irreversible` or [`Self::disruptive`].
    pub irreversible: bool,
    /// Workflow #434 LAB-5A: it stops or restarts something a person may be using, without being
    /// irreversible in the sense of §5; absent is false.
    #[serde(default)]
    pub disruptive: bool,
    pub runtime: RuntimeNeed,
    pub entries: Vec<Entry>,
    /// The request kinds it sends.
    #[serde(deserialize_with = "request_kinds")]
    pub requests: Vec<RuntimeOperationKind>,
    /// Kinds it will send that the contract does not have yet.
    pub planned_requests: Option<Vec<String>>,
    /// The commands, tools, console reads and scripts it replaces.
    pub source: Vec<String>,
    /// For an operation no program has today: the board item it comes from.
    pub new: Option<String>,
    /// The slice that builds it.
    pub slice: String,
    pub status: OperationStatus,
    /// What opens a `reserved` row.
    pub opens_with: Option<String>,
    pub notes: Option<String>,
}

impl Operation {
    /// The MCP tool: `ac_` and the name with spaces and `-` as `_` (§9.1).
    pub fn mcp_tool(&self) -> String {
        format!("ac_{}", self.name.replace([' ', '-'], "_"))
    }
}

/// The module that serves an operation (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationModule {
    Core,
    Debugging,
    Authoring,
    Rescue,
}

/// Whether an operation needs actingd (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeNeed {
    /// Refused while actingd is stopped.
    Required,
    /// Works while it is stopped, does more while it runs.
    Optional,
    /// Never needs it.
    Offline,
}

/// One of the two entries of the door (§1, §2): the channel a connection binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    Lab,
    Ui,
}

/// Whether a row is built by its slice or only holds a position (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    /// Built by `slice`.
    Planned,
    /// A position only: it opens with `opens_with`.
    Reserved,
}

/// One parameter of an operation (§9.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// Its MCP argument; its CLI flag is `--` and the name with `_` as `-`.
    pub name: String,
    pub form: ParamForm,
    #[serde(rename = "type")]
    pub value_type: ParamType,
    pub required: bool,
    /// Whether it may be given more than once.
    #[serde(default)]
    pub repeat: bool,
    pub default: Option<String>,
    /// Workflow #434 LAB-5A: the front ends (the ids of the `modules` rows of role `front_end`) in
    /// which it is required although the row does not require it; nothing reads it yet.
    pub required_in: Option<Vec<String>>,
    pub note: Option<String>,
}

/// How a parameter is written on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamForm {
    /// A word in its place; positionals keep their order.
    Positional,
    /// A flag with a value.
    Option,
    /// A flag without a value.
    Switch,
}

/// A parameter's type (§9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    /// An alias, an `instance_id` or an ADB port.
    Instance,
    String,
    Integer,
    /// Workflow #434 LAB-5A: a whole number from 0 to 2^64 − 1.
    Unsigned,
    /// Workflow #434 LAB-5A: a decimal number; the CLI hands the command its text.
    Decimal,
    Boolean,
    /// A number with `ms`, `s`, `m` or `h`.
    Duration,
    Path,
    File,
    Dir,
    Json,
    Key,
    /// `KEY=VALUE`.
    KeyValue,
    ListOfStrings,
    /// One of the listed words (`enum:<a|b>`).
    Enum(Vec<String>),
    RequestId,
    /// Workflow #434 LAB-5A: a request id or a run id.
    RunRef,
    /// `op-<ms>`.
    OperationId,
    MaterialRef,
    Code,
    /// An instance or a game.
    Scope,
    TagOrCommit,
    Sha256,
}

impl ParamType {
    /// The type `text` names, as the table writes it; `None` for another word.
    pub fn parse(text: &str) -> Option<Self> {
        if let Some(words) = text.strip_prefix("enum:") {
            let words: Vec<String> = words.split('|').map(str::to_owned).collect();
            return (!words.iter().any(String::is_empty)).then_some(Self::Enum(words));
        }
        Some(match text {
            "instance" => Self::Instance,
            "string" => Self::String,
            "integer" => Self::Integer,
            "unsigned" => Self::Unsigned,
            "decimal" => Self::Decimal,
            "boolean" => Self::Boolean,
            "duration" => Self::Duration,
            "path" => Self::Path,
            "file" => Self::File,
            "dir" => Self::Dir,
            "json" => Self::Json,
            "key" => Self::Key,
            "key_value" => Self::KeyValue,
            "list<string>" => Self::ListOfStrings,
            "request_id" => Self::RequestId,
            "run_ref" => Self::RunRef,
            "operation_id" => Self::OperationId,
            "material_ref" => Self::MaterialRef,
            "code" => Self::Code,
            "scope" => Self::Scope,
            "tag_or_commit" => Self::TagOrCommit,
            "sha256" => Self::Sha256,
            _ => return None,
        })
    }
}

impl<'de> Deserialize<'de> for ParamType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            de::Error::invalid_value(Unexpected::Str(&text), &"a parameter type of §9.1")
        })
    }
}

/// The request kind named `name`: the variant of [`RuntimeOperationKind`] spelled so, which its
/// derived `Debug` prints (a kind has no payload); `None` for another word.
pub fn request_kind_named(name: &str) -> Option<RuntimeOperationKind> {
    RuntimeOperationKind::ALL
        .iter()
        .copied()
        .find(|kind| request_kind_name(*kind) == name)
}

/// The name of `kind`, as the table writes it.
pub fn request_kind_name(kind: RuntimeOperationKind) -> String {
    format!("{kind:?}")
}

/// A refusal of the decoder for a word that is no request kind.
struct RequestKindExpected;

impl de::Expected for RequestKindExpected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a request kind of RuntimeOperationKind")
    }
}

fn request_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<RuntimeOperationKind, D::Error> {
    let name = String::deserialize(deserializer)?;
    request_kind_named(&name)
        .ok_or_else(|| de::Error::invalid_value(Unexpected::Str(&name), &RequestKindExpected))
}

fn request_kinds<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<RuntimeOperationKind>, D::Error> {
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(|name| {
            request_kind_named(name).ok_or_else(|| {
                de::Error::invalid_value(Unexpected::Str(name), &RequestKindExpected)
            })
        })
        .collect()
}

fn schema_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let schema = String::deserialize(deserializer)?;
    if schema == SCHEMA_VERSION {
        Ok(schema)
    } else {
        Err(de::Error::invalid_value(
            Unexpected::Str(&schema),
            &SCHEMA_VERSION,
        ))
    }
}
