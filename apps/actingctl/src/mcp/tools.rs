// SPDX-License-Identifier: AGPL-3.0-only

//! The single static tool table (#338 §二, §四 工具表). `tools/list`, `--list-tools`, the
//! `mcp-config` example and the skill's generated section all read it. Here too are the
//! tier gate, the result envelope `{ok, result, warnings?}` / `{ok:false, error}` and the
//! output budget (§四 结果形状 / 预算).

use super::observer;
use super::protocol::Era;
use super::runtime::RuntimeAccess;
use serde_json::{Map, Value, json};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// The `instructions` both clients read (§四 instructions).
pub(super) const INSTRUCTIONS: &str = "ActingCommand local control (tools-only). Start with ac_overview. Long operations return a handle (a Runtime request_id) at once; poll ac_get_run with wait_s. If an ac_run_pack call was interrupted before it returned, call ac_overview before submitting again. After an uncertain result call ac_get_run before anything else; never resend a write with new arguments. Approvals, actingd configuration edits and daemon restarts belong to the person. Tiers: observer (read), operator (device/scheduling), author (Lab recording); tools outside the enabled tiers answer tier_not_enabled. Manual: skill `actingcommand`.";

/// The longest a tool call waits inside the server; Runtime IO itself is bounded at 5 s.
pub(super) const CALL_BUDGET: Duration = Duration::from_secs(25);
pub(super) const DEFAULT_LIST: u64 = 20;
pub(super) const MAX_LIST: u64 = 100;
/// The structured payload, which the text content repeats.
const PAYLOAD_LIMIT: usize = 11 * 1024;
/// The whole `tools/call` result: text plus structuredContent.
const RESULT_LIMIT: usize = 24 * 1024;
/// Room for the 2026-07-28 `resultType` and `_meta` server identity.
const MODERN_FIELDS_RESERVE: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tier {
    Observer,
    Operator,
    Author,
}

impl Tier {
    const ALL: [Self; 3] = [Self::Observer, Self::Operator, Self::Author];

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::Operator => "operator",
            Self::Author => "author",
        }
    }
}

/// The tiers this process serves. observer is always on; operator and author only when
/// `--tier` names them.
#[derive(Clone, Copy, Default)]
pub(super) struct TierSet {
    operator: bool,
    author: bool,
}

impl TierSet {
    /// `observer|operator|author[,…]`; `None` for an empty or unknown name.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let mut tiers = Self::default();
        for name in text.split(',') {
            match Tier::ALL
                .into_iter()
                .find(|tier| tier.as_str() == name.trim())?
            {
                Tier::Observer => {}
                Tier::Operator => tiers.operator = true,
                Tier::Author => tiers.author = true,
            }
        }
        Some(tiers)
    }

    pub(super) const fn contains(self, tier: Tier) -> bool {
        match tier {
            Tier::Observer => true,
            Tier::Operator => self.operator,
            Tier::Author => self.author,
        }
    }

    /// The `--tier` argument naming exactly these tiers.
    pub(super) fn argument(self) -> String {
        Tier::ALL
            .into_iter()
            .filter(|tier| self.contains(*tier))
            .map(Tier::as_str)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// What a tool sees of the server while it runs.
pub(super) struct ToolContext<'a> {
    pub(super) runtime: &'a RuntimeAccess,
    /// Set by `notifications/cancelled`: stop waiting; the answer is not sent.
    pub(super) cancelled: &'a AtomicBool,
    /// The end of this call's 25 s budget.
    pub(super) deadline: Instant,
    /// This server process's identity; it binds every cursor it issues.
    pub(super) session: u64,
}

pub(super) type ToolOutcome = Result<ToolSuccess, ToolError>;

pub(super) struct ToolSuccess {
    pub(super) result: Value,
    pub(super) warnings: Vec<Value>,
}

impl ToolSuccess {
    pub(super) const fn new(result: Value) -> Self {
        Self {
            result,
            warnings: Vec::new(),
        }
    }
}

/// `{class, code, message, blocked_by, details}`; boxed so results stay small.
pub(super) struct ToolError(Box<ToolErrorBody>);

struct ToolErrorBody {
    class: &'static str,
    code: String,
    message: String,
    blocked_by: Option<String>,
    details: Map<String, Value>,
}

impl ToolError {
    pub(super) fn new(
        class: &'static str,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self(Box::new(ToolErrorBody {
            class,
            code: code.into(),
            message: message.into(),
            blocked_by: None,
            details: Map::new(),
        }))
    }

    pub(super) fn usage(code: &str, message: impl Into<String>) -> Self {
        Self::new("usage", code, message)
    }

    pub(super) fn with_detail(mut self, key: &str, value: Value) -> Self {
        self.0.details.insert(key.to_owned(), value);
        self
    }

    /// What has to change before the call can succeed.
    pub(super) fn blocked_by(mut self, pointer: impl Into<String>) -> Self {
        self.0.blocked_by = Some(pointer.into());
        self
    }

    pub(super) fn code(&self) -> &str {
        &self.0.code
    }

    pub(super) fn message(&self) -> &str {
        &self.0.message
    }

    /// The same failure reported beside a successful result.
    pub(super) fn into_warning(self) -> Value {
        let body = *self.0;
        json!({
            "code": body.code,
            "message": body.message,
            "blocked_by": body.blocked_by,
            "details": body.details,
        })
    }
}

/// A tool's arguments, checked against its input schema by hand: unknown keys are refused,
/// as `additionalProperties: false` says.
pub(super) struct Arguments<'a> {
    map: &'a Map<String, Value>,
}

impl<'a> Arguments<'a> {
    pub(super) fn new(map: &'a Map<String, Value>, allowed: &[&str]) -> Result<Self, ToolError> {
        match map.keys().find(|key| !allowed.contains(&key.as_str())) {
            Some(unknown) => Err(invalid_argument(unknown, "is not an argument of this tool")),
            None => Ok(Self { map }),
        }
    }

    pub(super) fn has(&self, key: &str) -> bool {
        self.map.get(key).is_some_and(|value| !value.is_null())
    }

    pub(super) fn string(&self, key: &str) -> Result<Option<&'a str>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.as_str())),
            Some(_) => Err(invalid_argument(key, "must be a non-empty string")),
        }
    }

    pub(super) fn required_string(&self, key: &str) -> Result<&'a str, ToolError> {
        self.string(key)?
            .ok_or_else(|| invalid_argument(key, "is required"))
    }

    pub(super) fn integer(
        &self,
        key: &str,
        minimum: u64,
        maximum: u64,
    ) -> Result<Option<u64>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .filter(|number| (minimum..=maximum).contains(number))
                .map(Some)
                .ok_or_else(|| {
                    invalid_argument(
                        key,
                        &format!("must be an integer from {minimum} to {maximum}"),
                    )
                }),
        }
    }
}

pub(super) fn invalid_argument(field: &str, reason: &str) -> ToolError {
    ToolError::usage("arguments_invalid", format!("argument {field} {reason}"))
        .with_detail("field", json!(field))
}

/// One row of the tool table.
pub(super) struct ToolDef {
    pub(super) name: &'static str,
    title: &'static str,
    pub(super) tier: Tier,
    description: &'static str,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    input_schema: fn() -> Value,
    result_schema: fn() -> Value,
    pub(super) run: fn(&ToolContext<'_>, &Map<String, Value>) -> ToolOutcome,
}

/// Every tool in this build; `tools/list` serves those of the enabled tiers.
pub(super) static TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "ac_overview",
        title: "ActingCommand overview",
        tier: Tier::Observer,
        description: "Snapshot of the local ActingCommand Runtime; call it first. Returns daemon {online, owner_epoch} (online false with the reason when actingd does not answer), global_pause, and one row per configured instance: alias, instance_id, adb_port, game_id, lease_active, queued requests, its scheduling pause {revision, stage, reason_code} and its monitor; plus lab_tool {present} and install {root, state_root, build}. instance (alias, instance_id or ADB port) narrows the rows to one. It only reads: it starts, stops and pauses nothing, and it neither shows nor infers whether an emulator is running.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: overview_input,
        result_schema: overview_result,
        run: observer::overview,
    },
    ToolDef {
        name: "ac_events",
        title: "Runtime ledger events",
        tier: Tier::Observer,
        description: "One page of Runtime ledger events. Events are returned oldest first from since_unix_ms; pass since_unix_ms for recent events; follow next_cursor. Without since_unix_ms the page starts at the beginning of the ledger. Filters: instance (alias, instance_id or ADB port) and view (events = every event, observation, changes, errors = warning and above, health, lab). Each row carries only seq, ts, type, severity, instance, request_id, run_id, the ledger's diagnostic code and materials [{ref, media_type, size}] for ac_material (materials_truncated when that list was shortened to fit the output budget); no event payload text. limit defaults to 20 (at most 100). next_cursor continues the same query: send it back with the same filters. truncated true means the page was cut to the output budget and next_cursor resumes after its last row. A cursor dies when the Runtime connection or this server restarts (cursor_invalid): read again from the first page.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: events_input,
        result_schema: events_result,
        run: observer::events,
    },
    ToolDef {
        name: "ac_material",
        title: "Runtime material",
        tier: Tier::Observer,
        description: "Reads one committed material named by a materials[].ref of ac_events, after the Runtime verified it. mode text returns up to max_bytes (default 4096, at most 8192) of a text or JSON material from offset, with next_offset while more remains. mode export writes the whole verified material to %TEMP%\\actingcommand-mcp\\materials\\<sha256>.<ext> and returns only {path, sha256, size}; use it for screenshots and archives, which are never inlined.",
        read_only: true,
        destructive: false,
        idempotent: true,
        input_schema: material_input,
        result_schema: material_result,
        run: observer::material,
    },
];

pub(super) fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// The tool definition in the shape of the session's protocol version.
pub(super) fn definition(tool: &ToolDef, era: Era) -> Value {
    let mut definition = Map::new();
    definition.insert("name".to_owned(), json!(tool.name));
    if era.structured() {
        definition.insert("title".to_owned(), json!(tool.title));
    }
    definition.insert("description".to_owned(), json!(tool.description));
    definition.insert("inputSchema".to_owned(), (tool.input_schema)());
    if era.structured() {
        definition.insert(
            "outputSchema".to_owned(),
            output_schema((tool.result_schema)()),
        );
    }
    definition.insert(
        "annotations".to_owned(),
        json!({
            "title": tool.title,
            "readOnlyHint": tool.read_only,
            "destructiveHint": tool.destructive,
            "idempotentHint": tool.idempotent,
            "openWorldHint": false,
        }),
    );
    Value::Object(definition)
}

/// `--list-tools --format json`: every tool in the build with its tier.
pub(super) fn list_json() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|tool| {
                let mut definition = definition(tool, Era::Modern);
                definition["tier"] = json!(tool.tier.as_str());
                definition
            })
            .collect(),
    )
}

/// `--list-tools --format markdown`: the skill's generated tool table.
pub(super) fn list_markdown() -> String {
    let mut table =
        String::from("| Tool | Tier | Hints | Arguments | Description |\n|---|---|---|---|---|\n");
    for tool in TOOLS {
        let schema = (tool.input_schema)();
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let arguments = schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties
                    .keys()
                    .map(|name| {
                        if required.contains(&name.as_str()) {
                            format!("`{name}`")
                        } else {
                            format!("`{name}?`")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let mut hints = Vec::new();
        if tool.read_only {
            hints.push("read-only");
        }
        if tool.destructive {
            hints.push("destructive");
        }
        if tool.idempotent {
            hints.push("idempotent");
        }
        let name = tool.name;
        let tier = tool.tier.as_str();
        let hints = hints.join(", ");
        let description = tool.description.replace('|', "\\|");
        table.push_str(&format!(
            "| `{name}` | {tier} | {hints} | {arguments} | {description} |\n"
        ));
    }
    table
}

/// A call to a tool of a tier this process was not started with.
pub(super) fn tier_not_enabled(tool: &ToolDef) -> ToolError {
    ToolError::new(
        "safety",
        "tier_not_enabled",
        format!(
            "{} belongs to tier {}, which this server was not started with",
            tool.name,
            tool.tier.as_str()
        ),
    )
    .with_detail("required_tier", json!(tool.tier.as_str()))
    .blocked_by("the person restarts mcp-serve with --tier including this tier")
}

/// The `tools/call` result: the envelope as compact JSON text, and from 2025-06-18 on also
/// as `structuredContent`. A result over the budget becomes `output_budget_exceeded`.
pub(super) fn call_result(outcome: ToolOutcome, era: Era) -> Value {
    let mut structured = envelope(outcome);
    if !fits(&structured) {
        let payload_bytes = structured.to_string().len();
        structured = envelope(Err(ToolError::usage(
            "output_budget_exceeded",
            "the result is larger than the 24 KiB tool output budget; narrow the request or lower limit",
        )
        .with_detail("payload_bytes", json!(payload_bytes))));
    }
    let is_error = structured.get("ok") == Some(&Value::Bool(false));
    let mut result = Map::new();
    result.insert(
        "content".to_owned(),
        json!([{"type": "text", "text": structured.to_string()}]),
    );
    if era.structured() {
        result.insert("structuredContent".to_owned(), structured);
    }
    result.insert("isError".to_owned(), json!(is_error));
    Value::Object(result)
}

/// Whether a successful `result` stays inside the output budget.
pub(super) fn fits_success(result: &Value) -> bool {
    fits(&json!({"ok": true, "result": result}))
}

fn fits(structured: &Value) -> bool {
    let payload = structured.to_string();
    if payload.len() > PAYLOAD_LIMIT {
        return false;
    }
    let whole = json!({
        "content": [{"type": "text", "text": payload}],
        "structuredContent": structured,
        "isError": false,
    });
    whole.to_string().len() + MODERN_FIELDS_RESERVE <= RESULT_LIMIT
}

fn envelope(outcome: ToolOutcome) -> Value {
    match outcome {
        Ok(success) => {
            let mut envelope = Map::new();
            envelope.insert("ok".to_owned(), json!(true));
            envelope.insert("result".to_owned(), success.result);
            if !success.warnings.is_empty() {
                envelope.insert("warnings".to_owned(), Value::Array(success.warnings));
            }
            Value::Object(envelope)
        }
        Err(error) => {
            let body = *error.0;
            json!({
                "ok": false,
                "error": {
                    "class": body.class,
                    "code": body.code,
                    "message": body.message,
                    "blocked_by": body.blocked_by,
                    "details": body.details,
                },
            })
        }
    }
}

/// Both envelope forms, with the tool's own result schema.
fn output_schema(result: Value) -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": {"type": "boolean"},
            "result": result,
            "warnings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "code": {"type": "string"},
                        "message": {"type": "string"},
                        "blocked_by": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                        "details": {"type": "object"},
                    },
                    "required": ["code", "message"],
                },
            },
            "error": {
                "type": "object",
                "properties": {
                    "class": {
                        "type": "string",
                        "enum": ["usage", "safety", "device", "runtime", "uncertain"],
                    },
                    "code": {"type": "string"},
                    "message": {"type": "string"},
                    "blocked_by": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "details": {"type": "object"},
                },
                "required": ["class", "code", "message", "blocked_by", "details"],
            },
        },
        "required": ["ok"],
        "oneOf": [
            {"type": "object", "properties": {"ok": {"const": true}}, "required": ["result"]},
            {"type": "object", "properties": {"ok": {"const": false}}, "required": ["error"]},
        ],
    })
}

fn instance_argument() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "Instance alias, instance_id or ADB port.",
    })
}

fn overview_input() -> Value {
    json!({
        "type": "object",
        "properties": {"instance": instance_argument()},
        "additionalProperties": false,
    })
}

fn overview_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "daemon": {
                "type": "object",
                "properties": {
                    "online": {"type": "boolean"},
                    "owner_epoch": {"type": "string"},
                    "error": {"type": "object"},
                },
                "required": ["online"],
            },
            "global_pause": {"type": "object"},
            "instances": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "alias": {"type": "string"},
                        "instance_id": {"type": "string"},
                        "adb_port": {"type": "integer"},
                        "game_id": {"type": "string"},
                        "lease_active": {"type": "boolean"},
                        "queued": {"type": "integer"},
                        "pause": {"type": "object"},
                        "monitor": {"type": "object"},
                    },
                    "required": ["alias", "instance_id", "lease_active", "queued"],
                },
            },
            "lab_tool": {
                "type": "object",
                "properties": {"present": {"type": "boolean"}},
                "required": ["present"],
            },
            "install": {
                "type": "object",
                "properties": {
                    "root": {"type": "string"},
                    "state_root": {"type": "string"},
                    "build": {"type": "object"},
                },
            },
            "incomplete": {"type": "boolean"},
        },
        "required": ["daemon", "instances", "lab_tool", "install"],
    })
}

fn events_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "instance": instance_argument(),
            "view": {
                "type": "string",
                "enum": ["events", "observation", "changes", "errors", "health", "lab"],
                "description": "Ledger view; without it every event.",
            },
            "since_unix_ms": {
                "type": "integer",
                "minimum": 0,
                "description": "Only events at or after this Unix time in milliseconds.",
            },
            "cursor": {
                "type": "string",
                "minLength": 1,
                "description": "next_cursor of the previous page, sent with the same filters.",
            },
            "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIST, "default": DEFAULT_LIST},
        },
        "additionalProperties": false,
    })
}

fn events_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "events": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "seq": {"type": "integer"},
                        "ts": {"type": "integer"},
                        "type": {"type": "string"},
                        "severity": {"type": "string"},
                        "instance": {"type": "string"},
                        "request_id": {"type": "string"},
                        "run_id": {"type": "string"},
                        "code": {"type": "string"},
                        "materials": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "ref": {"type": "string"},
                                    "media_type": {"type": "string"},
                                    "size": {"type": "integer"},
                                },
                                "required": ["ref", "media_type", "size"],
                            },
                        },
                        "materials_truncated": {"type": "boolean"},
                    },
                    "required": ["seq", "ts", "type", "severity"],
                },
            },
            "next_cursor": {"type": "string"},
            "truncated": {"type": "boolean"},
            "snapshot_ledger_position": {"type": "integer"},
        },
        "required": ["events", "snapshot_ledger_position"],
    })
}

fn material_input() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ref": {
                "type": "string",
                "minLength": 1,
                "description": "A materials[].ref from ac_events.",
            },
            "mode": {"type": "string", "enum": ["text", "export"]},
            "offset": {
                "type": "integer",
                "minimum": 0,
                "default": 0,
                "description": "mode text only: the first byte to return.",
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": 8192,
                "default": 4096,
                "description": "mode text only.",
            },
        },
        "required": ["ref", "mode"],
        "additionalProperties": false,
    })
}

fn material_result() -> Value {
    json!({
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "offset": {"type": "integer"},
            "bytes": {"type": "integer"},
            "next_offset": {"type": "integer"},
            "total_bytes": {"type": "integer"},
            "media_type": {"type": "string"},
            "path": {"type": "string"},
            "sha256": {"type": "string"},
            "size": {"type": "integer"},
        },
    })
}
