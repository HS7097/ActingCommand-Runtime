// SPDX-License-Identifier: AGPL-3.0-only

//! The entry's logic (Workflow #421 MCP-ENTRY, D1 to D6), driven by events through a [`Host`]
//! so the tests run it with a fake child and a fake clock.
//!
//! - It pipes JSON-RPC between the client and one current child, ids unchanged, and keeps the
//!   client's handshake (the legacy `initialize` and `notifications/initialized`, or the
//!   2026-07-28 `_meta`) to replay it.
//! - It watches cheap metadata of the install; a change that stays put for `settle_ms` makes it
//!   ask the current install's `actingctl mcp-entry plan` which build to run.
//! - A planned switch waits for a moment between calls with no process-local handle outstanding
//!   in the current child (`actingcommand/entryStatus`; a child before it answers "method not
//!   found", which counts as none when no call is in flight). Then it starts a candidate,
//!   replays the handshake, checks the protocol version and `tools/list`, and only then makes it
//!   current. A failed candidate is reported and the current child keeps serving.
//! - A request in flight when its child ends gets `mcp_entry_outcome_unknown` and is never sent
//!   again; the next request goes to a fresh child.
//! - After a switch the client gets the change notice of its generation: legacy
//!   `notifications/tools/list_changed`, or the same notification on each open
//!   `subscriptions/listen` stream (the entry owns those streams across children). Whether
//!   `tools/list` followed is recorded.
//! - Every change of state is written to the status file.

use crate::codes::EntryCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io;

/// The entry's generation: the `g<N>` directory it is placed in. It changes only when the
/// contract between the entry and its children changes.
pub(crate) const GENERATION: u32 = 1;
/// The contract version the entry asks `mcp-entry plan` for.
pub(crate) const ENTRY_PROTOCOL: u32 = 1;
pub(crate) const STATUS_SCHEMA: &str = "actingcommand.mcp-entry-status.v1";
const INTERNAL_PREFIX: &str = "actingcommand-entry:";
const ENTRY_STATUS_METHOD: &str = "actingcommand/entryStatus";
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";
const MODERN_VERSION: &str = "2026-07-28";
const PROBE_VERSION: &str = "2025-11-25";
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's implementation-defined server error, for the answers the entry gives itself.
const SERVER_ERROR: i64 = -32000;
/// How many earlier children the status keeps.
const HISTORY_LIMIT: usize = 8;

/// The plan's tunables (`install.mcp_entry.*`). [`Tunables::FIRST`] serves only until the first
/// plan answers, which the entry asks for as soon as it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Tunables {
    pub(crate) poll_ms: u64,
    pub(crate) settle_ms: u64,
    pub(crate) verify_timeout_ms: u64,
    pub(crate) drain_max_ms: u64,
    /// Fix round 1 (review M4): the cap of the doubling wait after consecutive refusals.
    pub(crate) refused_retry_max_ms: u64,
}

impl Tunables {
    pub(crate) const FIRST: Self = Self {
        poll_ms: 2_000,
        settle_ms: 10_000,
        verify_timeout_ms: 60_000,
        drain_max_ms: 600_000,
        refused_retry_max_ms: 300_000,
    };
}

/// The cheap metadata the entry watches: size and modification time of the install's Runtime
/// manifest and `actingctl.exe` (`None` while a file is missing).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Stamp {
    pub(crate) files: Vec<Option<(u64, u128)>>,
}

/// The client a configuration registered the entry for (`--client`): it names the refresh
/// entry of a client that does not fetch `tools/list` after the notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientKind {
    Claude,
    Codex,
    Antigravity,
    Dsh,
}

impl ClientKind {
    pub(crate) fn parse(word: &str) -> Option<Self> {
        match word {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "antigravity" => Some(Self::Antigravity),
            "dsh" => Some(Self::Dsh),
            _ => None,
        }
    }

    const fn refresh_entry(self) -> RefreshEntry {
        match self {
            Self::Claude => RefreshEntry::ToolsListChanged,
            Self::Codex => RefreshEntry::CodexReload,
            Self::Antigravity => RefreshEntry::AntigravityRefresh,
            Self::Dsh => RefreshEntry::DshPluginReload,
        }
    }
}

/// Where a client takes the new tool definitions from when it did not fetch `tools/list` after
/// the notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RefreshEntry {
    /// The client fetches `tools/list` on the notice by itself.
    ToolsListChanged,
    /// Codex: reload the MCP server.
    CodexReload,
    /// Antigravity: Refresh in its MCP manager.
    AntigravityRefresh,
    /// DSH: reload the plugin through its lifecycle; its whitelist names stay as they are.
    DshPluginReload,
    /// A client the configuration did not name: restart it.
    ClientRestart,
}

/// What the host does for the core. Lines are single JSON-RPC messages without the newline.
pub(crate) trait Host {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
    /// The watched metadata now.
    fn stamp(&self) -> Stamp;
    /// Whether an install operation holds the writer lock. An unfinished operation is the
    /// plan's refusal (`operation_unfinished`, review M1), not the entry's decision.
    fn busy(&mut self) -> bool;
    /// Starts `command` as child `id`; its output arrives as [`Core::child_line`] and its end
    /// as [`Core::child_exited`]. Returns its pid.
    fn spawn(&mut self, id: u64, command: &[String]) -> io::Result<u32>;
    /// Writes one line to child `id`; false when it cannot.
    fn send(&mut self, id: u64, line: &[u8]) -> bool;
    /// Closes child `id`'s input and makes sure it ends.
    fn stop(&mut self, id: u64);
    /// Runs `mcp-entry plan` in the background; its answer arrives as [`Core::plan_done`].
    fn start_plan(&mut self, current_build: Option<&str>, timeout_ms: u64);
    fn to_client(&mut self, line: &[u8]);
    fn write_status(&mut self, status: &Value);
    /// A line for the entry's stderr.
    fn note(&mut self, text: &str);
}

/// How a plan run ended: the plan's printed answer, or the failure to get one.
pub(crate) enum PlanEnd {
    Answered(Value),
    Failed(Value),
}

/// The build a plan names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Build {
    pub(crate) commit: String,
    pub(crate) manifest_sha256: String,
}

struct Plan {
    switch: bool,
    build: Build,
    exe: String,
    tunables: Tunables,
}

struct Planned {
    build: Build,
    exe: String,
    stamp: Stamp,
}

struct PlanRun {
    stamp: Stamp,
    initial: bool,
}

#[derive(Debug, Clone)]
struct Slot {
    id: u64,
    pid: u32,
    build: Option<String>,
    since_ms: u64,
    /// `tools/call` requests forwarded to it.
    calls: u64,
    /// Fix round 1 (review H1): the install's stamp when it was started, the image it runs.
    spawn_stamp: Stamp,
    /// Fix round 1 (review L3): a fresh child's predecessor's build, until the first plan.
    previous_build: Option<String>,
}

/// A candidate refused for one stamp (review L2: tried once more after `settle_ms`).
struct Rejected {
    stamp: Stamp,
    at_ms: u64,
    retried: bool,
}

/// The client lines held while a fresh child starts, with the requests they carry.
type Held = (Vec<u8>, Vec<(String, Pending)>);

/// The open `actingcommand/entryStatus` query: its id, its deadline and the current child's
/// `calls` when it was sent (review L7).
struct StatusQuery {
    key: String,
    deadline_ms: u64,
    calls: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    /// A candidate for a planned switch.
    Switch,
    /// A new child after the current one ended.
    Fresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    Initialize,
    Discover,
    ToolsList,
    Ready,
}

struct Starting {
    slot: Slot,
    purpose: Purpose,
    step: Step,
    request: Option<String>,
    deadline_ms: u64,
}

#[derive(Debug, Clone)]
struct Pending {
    id: Value,
    call: bool,
    tool: Option<String>,
}

#[derive(Default)]
struct Handshake {
    init_id: Option<String>,
    init_params: Option<Value>,
    initialized: bool,
    negotiated: Option<String>,
    modern_meta: Option<Value>,
    client_name: Option<String>,
    client_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Ended {
    Switched,
    Exited,
}

#[derive(Debug, Clone, Serialize)]
struct PastChild {
    pid: u32,
    build: Option<String>,
    since_unix_ms: u64,
    until_unix_ms: u64,
    ended: Ended,
}

#[derive(Debug, Clone, Serialize)]
struct SwitchRecord {
    at_unix_ms: u64,
    from_pid: u32,
    from_build: Option<String>,
    to_pid: u32,
    to_build: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeferKind {
    /// The current child holds process-local handles (D4).
    Handles,
    /// The current child did not say how many it holds.
    HandlesUnknown,
    /// A client call is in flight.
    CallsInFlight,
    /// An install operation holds the writer lock.
    InstallBusy,
    /// The plan refused or could not run.
    PlanRefused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Deferral {
    kind: DeferKind,
    since_unix_ms: u64,
    outstanding_handles: Option<u64>,
    handle_kinds: Vec<String>,
    /// Fix round 1 (review M2): the outstanding handles, to collect a stuck one with
    /// `ac_get_run`.
    handles: Vec<String>,
    past_drain_max: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CandidateProblem {
    StartFailed,
    InitializeFailed,
    ProtocolVersionChanged,
    ToolsListFailed,
    VerifyTimeout,
    Exited,
}

#[derive(Debug, Clone, Serialize)]
struct CandidateFailure {
    at_unix_ms: u64,
    purpose: Purpose,
    pid: Option<u32>,
    build: Option<String>,
    problem: CandidateProblem,
}

#[derive(Debug, Clone, Serialize)]
struct PlanRecord {
    at_unix_ms: u64,
    switch: Option<bool>,
    build: Option<Build>,
    /// The plan's printed outcome, or what is known of a plan that did not answer.
    outcome: Value,
}

#[derive(Debug, Clone, Serialize)]
struct Notice {
    sent_at_unix_ms: u64,
    legacy: bool,
    listen_subscriptions: usize,
    tools_list_after: bool,
    tools_list_at_unix_ms: Option<u64>,
}

/// What the entry was started with.
pub(crate) struct EntryConfig {
    /// `<root>\runtime\actingctl.exe`: the child before any plan names one.
    pub(crate) default_exe: String,
    /// Everything after `--`, passed to every child unchanged.
    pub(crate) child_args: Vec<String>,
    pub(crate) client_kind: Option<ClientKind>,
    pub(crate) pid: u32,
    /// #411 P49 (review L4): the entry process's creation time as the operating system reports
    /// it, in Unix milliseconds; `None` when the operating system did not say. With the pid it
    /// names this process: `actingctl mcp-entry list` compares both, since pids are reused.
    pub(crate) process_created_at_unix_ms: Option<u64>,
}

pub(crate) struct Core<H: Host> {
    pub(crate) host: H,
    config: EntryConfig,
    started_ms: u64,
    tunables: Tunables,
    next_child: u64,
    seq: u64,
    current: Option<Slot>,
    starting: Option<Starting>,
    /// The stamp the current child's build stands for; `None` until a plan named its build.
    accepted: Option<Stamp>,
    pending: Option<(Stamp, u64)>,
    rejected: Option<Rejected>,
    plan_running: Option<PlanRun>,
    plan_retry_at: u64,
    /// Consecutive plan refusals (review M4): the wait before the next plan doubles with each.
    plan_refusals: u32,
    planned: Option<Planned>,
    handles_clear: bool,
    status_query: Option<StatusQuery>,
    handshake: Handshake,
    inflight: BTreeMap<String, Pending>,
    discover_ids: Vec<String>,
    /// Open `subscriptions/listen` requests by id, and whether each asked for tool changes.
    listens: BTreeMap<String, (Value, bool)>,
    held: VecDeque<Held>,
    history: VecDeque<PastChild>,
    last_switch: Option<SwitchRecord>,
    deferred: Option<Deferral>,
    failure: Option<CandidateFailure>,
    last_plan: Option<PlanRecord>,
    notice: Option<Notice>,
    closing: bool,
    dirty: bool,
}

fn key_of(id: &Value) -> String {
    id.to_string()
}

/// The id of an answer to one of the entry's own requests.
fn internal_key(message: &Value) -> Option<String> {
    if message.get("method").is_some() {
        return None;
    }
    message
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with(INTERNAL_PREFIX))
        .map(str::to_owned)
}

fn parse_plan(answer: &Value) -> Option<Plan> {
    if answer.get("entry_protocol")?.as_u64()? != u64::from(ENTRY_PROTOCOL) {
        return None;
    }
    let switch = answer.get("switch")?.as_bool()?;
    let build = serde_json::from_value::<Build>(answer.get("build")?.clone()).ok()?;
    let exe = answer
        .get("child")?
        .as_array()?
        .first()?
        .as_str()?
        .to_owned();
    let tunables = parse_tunables(answer)?;
    Some(Plan {
        switch,
        build,
        exe,
        tunables,
    })
}

/// The tunables of a plan's answer, a refusal's too (review M4).
fn parse_tunables(answer: &Value) -> Option<Tunables> {
    serde_json::from_value::<Tunables>(answer.get("tunables")?.clone()).ok()
}

#[derive(Serialize)]
struct RpcError<'a> {
    code: i64,
    message: &'a str,
    data: Value,
}

#[derive(Serialize)]
struct RpcErrorReply<'a> {
    jsonrpc: &'static str,
    id: &'a Value,
    error: RpcError<'a>,
}

impl<H: Host> Core<H> {
    pub(crate) fn new(host: H, config: EntryConfig) -> Self {
        let started_ms = host.now_ms();
        Self {
            host,
            config,
            started_ms,
            tunables: Tunables::FIRST,
            next_child: 0,
            seq: 0,
            current: None,
            starting: None,
            accepted: None,
            pending: None,
            rejected: None,
            plan_running: None,
            plan_retry_at: 0,
            plan_refusals: 0,
            planned: None,
            handles_clear: false,
            status_query: None,
            handshake: Handshake::default(),
            inflight: BTreeMap::new(),
            discover_ids: Vec::new(),
            listens: BTreeMap::new(),
            held: VecDeque::new(),
            history: VecDeque::new(),
            last_switch: None,
            deferred: None,
            failure: None,
            last_plan: None,
            notice: None,
            closing: false,
            dirty: true,
        }
    }

    pub(crate) const fn poll_ms(&self) -> u64 {
        self.tunables.poll_ms
    }

    /// The current child's id, while there is one.
    pub(crate) fn current_child(&self) -> Option<u64> {
        self.current.as_ref().map(|slot| slot.id)
    }

    /// Starts the first child and asks the plan which build it is.
    pub(crate) fn start(&mut self) {
        self.start_fresh();
        let now = self.host.now_ms();
        self.watch(now);
        self.finish_event();
    }

    // ---- client side ------------------------------------------------------------------------

    pub(crate) fn client_line(&mut self, line: &[u8]) {
        if self.closing {
            return;
        }
        let mut requests = Vec::new();
        let forward = match serde_json::from_slice::<Value>(line) {
            Ok(Value::Array(items)) => {
                for item in &items {
                    self.observe_client(item, &mut requests);
                }
                true
            }
            Ok(message) => self.observe_client(&message, &mut requests),
            // The child answers what it cannot parse.
            Err(_) => true,
        };
        if forward {
            self.deliver(line.to_vec(), requests);
        }
        self.finish_event();
    }

    pub(crate) fn client_closed(&mut self) {
        self.closing = true;
        if let Some(starting) = self.starting.take() {
            self.host.stop(starting.slot.id);
        }
        if let Some(current) = &self.current {
            self.host.stop(current.id);
        }
        self.dirty = true;
        self.finish_event();
    }

    /// Notes what a client message changes; false when the entry answers it itself.
    fn observe_client(&mut self, message: &Value, requests: &mut Vec<(String, Pending)>) -> bool {
        if !message.is_object() {
            return true;
        }
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        let params = message.get("params");
        if let Some(meta) = params
            .and_then(|params| params.get("_meta"))
            .filter(|meta| meta.get(META_PROTOCOL_VERSION).is_some())
        {
            if self.handshake.modern_meta.as_ref() != Some(meta) {
                self.handshake.modern_meta = Some(meta.clone());
                self.dirty = true;
            }
            if let Some(info) = meta.get(META_CLIENT_INFO) {
                self.client_info(info);
            }
        }
        match (method, id) {
            (Some("subscriptions/listen"), Some(id)) => {
                self.listen(id.clone(), params);
                false
            }
            (Some("notifications/cancelled"), None) => {
                let Some(target) = params.and_then(|params| params.get("requestId")) else {
                    return true;
                };
                let key = key_of(target);
                if self.listens.remove(&key).is_some() {
                    self.dirty = true;
                    return false;
                }
                // A cancelled call gets no answer; the child's own count covers it until it ends.
                self.inflight.remove(&key);
                true
            }
            (Some("notifications/initialized"), None) => {
                self.handshake.initialized = true;
                self.dirty = true;
                true
            }
            (Some(name), Some(id)) => {
                let key = key_of(id);
                match name {
                    "initialize" => {
                        self.handshake.init_id = Some(key.clone());
                        self.handshake.init_params = params.cloned();
                        if let Some(info) = params.and_then(|params| params.get("clientInfo")) {
                            self.client_info(info);
                        }
                        self.dirty = true;
                    }
                    "server/discover" => self.discover_ids.push(key.clone()),
                    "tools/list" => self.tools_listed(),
                    _ => {}
                }
                let call = name == "tools/call";
                let tool = if call {
                    params
                        .and_then(|params| params.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                };
                requests.push((
                    key,
                    Pending {
                        id: id.clone(),
                        call,
                        tool,
                    },
                ));
                true
            }
            _ => true,
        }
    }

    fn client_info(&mut self, info: &Value) {
        let name = info.get("name").and_then(Value::as_str).map(str::to_owned);
        let version = info
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if name.is_some() && (self.handshake.client_name != name) {
            self.handshake.client_name = name;
            self.handshake.client_version = version;
            self.dirty = true;
        }
    }

    /// `subscriptions/listen` (2026-07-28): the entry owns the stream across children and
    /// honours tool-list changes only.
    fn listen(&mut self, id: Value, params: Option<&Value>) {
        let tools = params
            .and_then(|params| params.get("notifications"))
            .and_then(|filter| filter.get("toolsListChanged"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut honoured = Map::new();
        if tools {
            honoured.insert("toolsListChanged".to_owned(), Value::Bool(true));
        }
        let mut meta = Map::new();
        meta.insert(META_SUBSCRIPTION_ID.to_owned(), id.clone());
        self.listens.insert(key_of(&id), (id, tools));
        self.send_client(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/subscriptions/acknowledged",
            "params": {"_meta": meta, "notifications": honoured},
        }));
        self.dirty = true;
    }

    fn tools_listed(&mut self) {
        let now = self.host.now_ms();
        if let Some(notice) = self
            .notice
            .as_mut()
            .filter(|notice| !notice.tools_list_after)
        {
            notice.tools_list_after = true;
            notice.tools_list_at_unix_ms = Some(now);
            self.dirty = true;
        }
    }

    /// Sends a client line to the current child, or holds it while a fresh child starts.
    fn deliver(&mut self, line: Vec<u8>, requests: Vec<(String, Pending)>) {
        let fresh_starting = self
            .starting
            .as_ref()
            .is_some_and(|starting| starting.purpose == Purpose::Fresh);
        let Some(current) = self.current.as_mut().filter(|_| !fresh_starting) else {
            self.held.push_back((line, requests));
            if self.starting.is_none() {
                self.start_fresh();
            }
            return;
        };
        let child = current.id;
        let calls = requests.iter().filter(|(_, pending)| pending.call).count();
        current.calls += calls as u64;
        if calls > 0 {
            self.handles_clear = false;
        }
        for (key, pending) in &requests {
            self.inflight.insert(key.clone(), pending.clone());
        }
        if !self.host.send(child, &line) {
            self.host.note(&format!(
                "cannot write to child {child}; its requests were not sent"
            ));
            for (key, pending) in requests {
                self.inflight.remove(&key);
                self.answer_error(&pending, EntryCode::ChildUnavailable);
            }
        }
        self.dirty = true;
    }

    fn answer_error(&mut self, pending: &Pending, code: EntryCode) {
        let mut outcome = Outcome::new(code);
        if let Some(tool) = &pending.tool {
            outcome = outcome.mcp_tool(tool.as_str());
        }
        let mut data = Map::new();
        data.insert(
            "outcome".to_owned(),
            serde_json::to_value(outcome.envelope(Detail::Full)).unwrap_or(Value::Null),
        );
        if code == EntryCode::OutcomeUnknown {
            // Inspect before anything is sent again (D3 rule 3).
            data.insert(
                "inspect_with".to_owned(),
                json!(["ac_get_run", "ac_overview"]),
            );
        }
        let reply = RpcErrorReply {
            jsonrpc: "2.0",
            id: &pending.id,
            error: RpcError {
                code: SERVER_ERROR,
                message: code.as_str(),
                data: Value::Object(data),
            },
        };
        match serde_json::to_vec(&reply) {
            Ok(line) => self.host.to_client(&line),
            Err(error) => self.host.note(&format!("cannot encode an answer: {error}")),
        }
    }

    fn send_client(&mut self, message: &Value) {
        self.host.to_client(message.to_string().as_bytes());
    }

    // ---- child side -------------------------------------------------------------------------

    pub(crate) fn child_line(&mut self, child: u64, line: &[u8]) {
        let parsed = serde_json::from_slice::<Value>(line).ok();
        if let Some(key) = parsed.as_ref().and_then(internal_key) {
            if let Some(message) = &parsed {
                self.internal_answer(child, &key, message);
            }
            self.finish_event();
            return;
        }
        if self.current_child() != Some(child) {
            self.host.note(&format!(
                "dropped a line of child {child}, which is not the current child"
            ));
            self.finish_event();
            return;
        }
        match parsed {
            Some(Value::Array(items)) => {
                for item in &items {
                    self.settle(item);
                }
                self.host.to_client(line);
            }
            Some(Value::Object(mut object)) => {
                let patched = self.patch_answer(&mut object);
                let message = Value::Object(object);
                self.settle(&message);
                if patched {
                    self.send_client(&message);
                } else {
                    self.host.to_client(line);
                }
            }
            _ => self.host.to_client(line),
        }
        self.try_switch();
        self.finish_event();
    }

    pub(crate) fn child_exited(&mut self, child: u64) {
        let now = self.host.now_ms();
        if self
            .starting
            .as_ref()
            .is_some_and(|starting| starting.slot.id == child)
        {
            self.fail_starting(CandidateProblem::Exited);
        } else if self.current_child() == Some(child) {
            if let Some(old) = self.current.take() {
                self.retire(&old, now, Ended::Exited);
            }
            self.accepted = None;
            self.planned = None;
            self.pending = None;
            self.status_query = None;
            self.handles_clear = false;
            self.deferred = None;
            // A candidate verified against a session that just ended is not used.
            if let Some(starting) = self.starting.take() {
                self.host.stop(starting.slot.id);
            }
            let lost = std::mem::take(&mut self.inflight);
            if !lost.is_empty() {
                self.host.note(&format!(
                    "child {child} ended with {} request(s) in flight; their outcome is unknown and none is sent again",
                    lost.len()
                ));
            }
            for pending in lost.values() {
                self.answer_error(pending, EntryCode::OutcomeUnknown);
            }
            self.dirty = true;
        }
        self.finish_event();
    }

    /// A response ends its request's flight; the status's `calls_in_flight` follows (review L5).
    fn settle(&mut self, message: &Value) {
        if let (None, Some(id)) = (message.get("method"), message.get("id"))
            && self.inflight.remove(&key_of(id)).is_some()
        {
            self.dirty = true;
        }
    }

    /// The answers to the client's `initialize` and `server/discover` advertise
    /// `tools.listChanged` (D5): the entry sends the notice whatever the child says.
    fn patch_answer(&mut self, object: &mut Map<String, Value>) -> bool {
        if object.contains_key("method") {
            return false;
        }
        let Some(key) = object.get("id").map(key_of) else {
            return false;
        };
        let init = self.handshake.init_id.as_deref() == Some(key.as_str());
        let discover = self.discover_ids.iter().position(|id| *id == key);
        if let Some(index) = discover {
            self.discover_ids.remove(index);
        }
        if !init && discover.is_none() {
            return false;
        }
        let Some(result) = object.get_mut("result").and_then(Value::as_object_mut) else {
            return false;
        };
        if init {
            self.handshake.negotiated = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_owned);
            self.dirty = true;
        }
        let capabilities = result
            .entry("capabilities")
            .or_insert_with(|| Value::Object(Map::new()));
        if !capabilities.is_object() {
            *capabilities = Value::Object(Map::new());
        }
        let Some(capabilities) = capabilities.as_object_mut() else {
            return false;
        };
        let tools = capabilities
            .entry("tools")
            .or_insert_with(|| Value::Object(Map::new()));
        if !tools.is_object() {
            *tools = Value::Object(Map::new());
        }
        if let Some(tools) = tools.as_object_mut() {
            tools.insert("listChanged".to_owned(), Value::Bool(true));
        }
        true
    }

    fn internal_answer(&mut self, child: u64, key: &str, message: &Value) {
        let status_answer = self
            .status_query
            .as_ref()
            .is_some_and(|query| query.key == key)
            && self.current_child() == Some(child);
        if status_answer {
            let asked_at_calls = self.status_query.take().map_or(0, |query| query.calls);
            self.status_answer(message, asked_at_calls);
            return;
        }
        let step_answer = self.starting.as_ref().is_some_and(|starting| {
            starting.slot.id == child && starting.request.as_deref() == Some(key)
        });
        if step_answer {
            self.step_answer(message);
            return;
        }
        self.host
            .note(&format!("ignored a late answer {key} of child {child}"));
    }

    /// The current child's answer to `actingcommand/entryStatus` (D4). A `0` counts only when no
    /// call was delivered to the child since the query went out (review L7); otherwise it asks
    /// again.
    fn status_answer(&mut self, message: &Value, asked_at_calls: u64) {
        let now = self.host.now_ms();
        let unchanged = self
            .current
            .as_ref()
            .is_some_and(|slot| slot.calls == asked_at_calls);
        let strings = |member: &str| {
            message
                .get("result")
                .and_then(|result| result.get(member))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        if let Some(result) = message.get("result") {
            let handles = result.get("outstanding_handles").and_then(Value::as_u64);
            let calls = result
                .get("calls_in_flight")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            match handles.map(|handles| handles.saturating_add(calls)) {
                Some(0) if unchanged => self.clear_handles(),
                Some(0) => self.try_switch(),
                Some(outstanding) => self.defer(
                    DeferKind::Handles,
                    Some(outstanding),
                    strings("kinds"),
                    strings("handles"),
                    now,
                ),
                None => self.defer(DeferKind::HandlesUnknown, None, Vec::new(), Vec::new(), now),
            }
            return;
        }
        let not_found = message
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_i64)
            == Some(METHOD_NOT_FOUND);
        if not_found && self.inflight.is_empty() && unchanged {
            // A child before `actingcommand/entryStatus`: none outstanding with no call in
            // flight (D4).
            self.clear_handles();
        } else if not_found && self.inflight.is_empty() {
            self.try_switch();
        } else {
            self.defer(DeferKind::HandlesUnknown, None, Vec::new(), Vec::new(), now);
        }
    }

    fn clear_handles(&mut self) {
        self.handles_clear = true;
        self.deferred = None;
        self.dirty = true;
        self.try_switch();
    }

    fn defer(
        &mut self,
        kind: DeferKind,
        outstanding: Option<u64>,
        kinds: Vec<String>,
        handles: Vec<String>,
        now: u64,
    ) {
        let since = match &self.deferred {
            Some(deferral) if deferral.kind == kind => deferral.since_unix_ms,
            _ => now,
        };
        let past =
            kind == DeferKind::Handles && now.saturating_sub(since) > self.tunables.drain_max_ms;
        let newly_past = past
            && !self
                .deferred
                .as_ref()
                .is_some_and(|deferral| deferral.past_drain_max);
        let deferral = Deferral {
            kind,
            since_unix_ms: since,
            outstanding_handles: outstanding,
            handle_kinds: kinds,
            handles,
            past_drain_max: past,
        };
        if self.deferred.as_ref() != Some(&deferral) {
            self.deferred = Some(deferral);
            self.dirty = true;
        }
        if newly_past {
            self.host.note(
                "the switch has waited past install.mcp_entry.drain_max_ms for outstanding handles; it keeps waiting and never cuts them",
            );
        }
    }

    // ---- watching and planning --------------------------------------------------------------

    pub(crate) fn tick(&mut self) {
        if self.closing {
            return;
        }
        let now = self.host.now_ms();
        if self
            .starting
            .as_ref()
            .is_some_and(|starting| starting.step != Step::Ready && now >= starting.deadline_ms)
        {
            self.fail_starting(CandidateProblem::VerifyTimeout);
        }
        if self
            .status_query
            .as_ref()
            .is_some_and(|query| now >= query.deadline_ms)
        {
            self.status_query = None;
            self.defer(DeferKind::HandlesUnknown, None, Vec::new(), Vec::new(), now);
        }
        self.watch(now);
        self.try_switch();
        self.finish_event();
    }

    fn watch(&mut self, now: u64) {
        if self.current.is_none() || self.plan_running.is_some() || self.starting.is_some() {
            return;
        }
        let stamp = self.host.stamp();
        // #411 P49 (review L10): a planned switch waits (for handles, a call, the writer lock)
        // while the install it was planned for stays put; a newer install drops it, and the
        // change below is planned again once it settles.
        match self.planned.as_ref().map(|planned| planned.stamp == stamp) {
            Some(true) => return,
            Some(false) => self.drop_stale_plan(),
            None => {}
        }
        if self
            .rejected
            .as_ref()
            .is_some_and(|rejected| rejected.stamp == stamp)
        {
            // Review L2: a candidate refused for this stamp is tried once more after
            // `settle_ms`, then not again until the install changes.
            let due = self.rejected.as_ref().is_some_and(|rejected| {
                !rejected.retried && now.saturating_sub(rejected.at_ms) >= self.tunables.settle_ms
            });
            if self.pending.take().is_some() {
                self.dirty = true;
            }
            if due {
                let current = self.current.as_ref().and_then(|slot| slot.build.clone());
                if self.run_plan(stamp, current, now)
                    && let Some(rejected) = self.rejected.as_mut()
                {
                    rejected.retried = true;
                }
            }
            return;
        }
        if self.accepted.is_none() {
            // The current child's build is not known yet: the plan names it.
            self.run_plan(stamp, None, now);
            return;
        }
        if self.accepted.as_ref() == Some(&stamp) {
            if self.pending.take().is_some() {
                self.dirty = true;
            }
            self.clear_deferral_without_plan();
            return;
        }
        let seen = matches!(&self.pending, Some((pending, _)) if *pending == stamp);
        if !seen {
            self.pending = Some((stamp, now));
            self.dirty = true;
            return;
        }
        let settled = matches!(
            &self.pending,
            Some((_, since)) if now.saturating_sub(*since) >= self.tunables.settle_ms
        );
        if settled {
            let current = self.current.as_ref().and_then(|slot| slot.build.clone());
            self.run_plan(stamp, current, now);
        }
    }

    /// Asks the plan, never while an install operation is under way (E-8); true when it runs.
    fn run_plan(&mut self, stamp: Stamp, current: Option<String>, now: u64) -> bool {
        if now < self.plan_retry_at {
            return false;
        }
        if self.host.busy() {
            self.defer(DeferKind::InstallBusy, None, Vec::new(), Vec::new(), now);
            return false;
        }
        self.plan_running = Some(PlanRun {
            initial: current.is_none(),
            stamp,
        });
        self.host
            .start_plan(current.as_deref(), self.tunables.verify_timeout_ms);
        self.dirty = true;
        true
    }

    pub(crate) fn plan_done(&mut self, end: PlanEnd) {
        let Some(run) = self.plan_running.take() else {
            return;
        };
        let now = self.host.now_ms();
        let answer = match end {
            PlanEnd::Answered(answer) => answer,
            PlanEnd::Failed(failure) => {
                self.host
                    .note(&format!("mcp-entry plan did not answer: {failure}"));
                self.last_plan = Some(PlanRecord {
                    at_unix_ms: now,
                    switch: None,
                    build: None,
                    outcome: failure,
                });
                self.plan_refused(now);
                return;
            }
        };
        let plan = parse_plan(&answer);
        self.last_plan = Some(PlanRecord {
            at_unix_ms: now,
            switch: plan.as_ref().map(|plan| plan.switch),
            build: plan.as_ref().map(|plan| plan.build.clone()),
            outcome: answer.get("outcome").cloned().unwrap_or(Value::Null),
        });
        self.dirty = true;
        let Some(plan) = plan else {
            self.host.note(&format!("mcp-entry plan refused: {answer}"));
            // Review M4: a refusal carries the tunables, so the retry policy stays the plan's.
            if let Some(tunables) = parse_tunables(&answer) {
                self.tunables = tunables;
            }
            self.plan_refused(now);
            return;
        };
        self.tunables = plan.tunables;
        self.plan_refusals = 0;
        if self.deferred.as_ref().is_some_and(|deferral| {
            matches!(
                deferral.kind,
                DeferKind::PlanRefused | DeferKind::InstallBusy
            )
        }) {
            self.deferred = None;
        }
        // Review H1: an initial plan asked after the install changed under the current child
        // (its stamp is not the one the child was started at) names a build the child may not
        // run; it is a planned switch, never accepted as the child's build.
        let changed_under_child = run.initial
            && self
                .current
                .as_ref()
                .is_some_and(|slot| slot.spawn_stamp != run.stamp);
        if (plan.switch && !run.initial) || changed_under_child {
            self.planned = Some(Planned {
                build: plan.build,
                exe: plan.exe,
                stamp: run.stamp,
            });
            self.try_switch();
        } else {
            // Review L3: the first plan after a fresh child names a build other than its
            // predecessor's: the client gets the change notice.
            let previous = self
                .current
                .as_mut()
                .and_then(|slot| slot.previous_build.take());
            let differs = previous.is_some_and(|previous| previous != plan.build.commit);
            if let Some(current) = self.current.as_mut() {
                current.build = Some(plan.build.commit);
            }
            self.accepted = Some(run.stamp);
            self.pending = None;
            self.rejected = None;
            self.clear_deferral_without_plan();
            if differs {
                self.send_notices(now);
            }
        }
        self.finish_event();
    }

    /// Review M4: consecutive refusals double the wait before the next plan, from
    /// `settle_ms` (at least `poll_ms`) up to `refused_retry_max_ms`.
    fn plan_refused(&mut self, now: u64) {
        self.defer(DeferKind::PlanRefused, None, Vec::new(), Vec::new(), now);
        let base = self.tunables.settle_ms.max(self.tunables.poll_ms);
        let factor = 1_u64.checked_shl(self.plan_refusals).unwrap_or(u64::MAX);
        let wait = base
            .saturating_mul(factor)
            .min(self.tunables.refused_retry_max_ms);
        self.plan_refusals = self.plan_refusals.saturating_add(1);
        self.plan_retry_at = now.saturating_add(wait);
        self.dirty = true;
        self.finish_event();
    }

    // ---- switching --------------------------------------------------------------------------

    /// Moves a planned switch on, only between calls and with no handle outstanding.
    fn try_switch(&mut self) {
        if self.closing || self.planned.is_none() || self.current.is_none() {
            return;
        }
        let candidate_ready = match &self.starting {
            Some(starting) if starting.purpose == Purpose::Switch => {
                if starting.step != Step::Ready {
                    return;
                }
                true
            }
            Some(_) => return,
            None => false,
        };
        let now = self.host.now_ms();
        if !self.inflight.is_empty() {
            self.defer(DeferKind::CallsInFlight, None, Vec::new(), Vec::new(), now);
            return;
        }
        if self.status_query.is_some() {
            return;
        }
        let calls = self.current.as_ref().map_or(0, |slot| slot.calls);
        if calls > 0 && !self.handles_clear {
            self.query_status(now);
            return;
        }
        if candidate_ready {
            self.commit_switch();
        } else {
            self.start_candidate();
        }
    }

    fn query_status(&mut self, now: u64) {
        let Some(child) = self.current_child() else {
            return;
        };
        let params = match &self.handshake.modern_meta {
            Some(meta) if self.handshake.init_params.is_none() => json!({"_meta": meta}),
            _ => json!({}),
        };
        match self.internal_request(child, ENTRY_STATUS_METHOD, params) {
            Some(key) => {
                let calls = self.current.as_ref().map_or(0, |slot| slot.calls);
                self.status_query = Some(StatusQuery {
                    key,
                    deadline_ms: now.saturating_add(self.tunables.verify_timeout_ms),
                    calls,
                });
            }
            None => self.defer(DeferKind::HandlesUnknown, None, Vec::new(), Vec::new(), now),
        }
    }

    fn start_candidate(&mut self) {
        let Some(planned) = &self.planned else {
            return;
        };
        let exe = planned.exe.clone();
        let build = planned.build.commit.clone();
        // #411 P49 (review L10): the candidate would run the file the install holds now under
        // the plan's build; a plan the install moved past is planned again instead.
        if self.host.stamp() != planned.stamp {
            self.drop_stale_plan();
            return;
        }
        match self.spawn_slot(&exe, Some(build.clone())) {
            Ok(slot) => self.begin(slot, Purpose::Switch),
            Err(error) => {
                self.host
                    .note(&format!("the candidate child {exe} did not start: {error}"));
                self.failure = Some(CandidateFailure {
                    at_unix_ms: self.host.now_ms(),
                    purpose: Purpose::Switch,
                    pid: None,
                    build: Some(build),
                    problem: CandidateProblem::StartFailed,
                });
                self.reject_planned();
                self.deferred = None;
                self.dirty = true;
            }
        }
    }

    fn start_fresh(&mut self) {
        let exe = self.config.default_exe.clone();
        match self.spawn_slot(&exe, None) {
            Ok(slot) => self.begin(slot, Purpose::Fresh),
            Err(error) => {
                self.host
                    .note(&format!("the child {exe} did not start: {error}"));
                self.failure = Some(CandidateFailure {
                    at_unix_ms: self.host.now_ms(),
                    purpose: Purpose::Fresh,
                    pid: None,
                    build: None,
                    problem: CandidateProblem::StartFailed,
                });
                self.fail_held();
                self.dirty = true;
            }
        }
    }

    fn spawn_slot(&mut self, exe: &str, build: Option<String>) -> io::Result<Slot> {
        self.next_child += 1;
        let id = self.next_child;
        let mut command = vec![exe.to_owned()];
        command.extend(self.config.child_args.iter().cloned());
        // Review H1: taken before the start, so a change racing the start errs towards one
        // more switch, never towards a wrong build.
        let spawn_stamp = self.host.stamp();
        let pid = self.host.spawn(id, &command)?;
        Ok(Slot {
            id,
            pid,
            build,
            since_ms: self.host.now_ms(),
            calls: 0,
            spawn_stamp,
            previous_build: None,
        })
    }

    /// #411 P49 (review L10): the install changed after the plan named its build and before a
    /// candidate started, so a candidate would run the newer file under the older build's name
    /// (and one more switch would follow). The plan is dropped; `watch` sees the change, and once
    /// it settles the next plan names the build the install then holds. A deferral it waited in
    /// stays, so its time keeps counting, until no switch follows (see
    /// [`Self::clear_deferral_without_plan`]).
    fn drop_stale_plan(&mut self) {
        if self.planned.take().is_some() {
            self.dirty = true;
            self.host.note(
                "the install changed while a planned switch waited; it is planned again once the change settles",
            );
        }
    }

    /// Fix round 1 (review L1): no switch is planned and none follows (a plan answered no
    /// switch, or the install is back at the accepted build, as after an S2 rollback to the
    /// current version within minutes), so nothing waits: a deferral a dropped plan left is
    /// cleared, and the next deferral counts from its own start.
    fn clear_deferral_without_plan(&mut self) {
        if self.planned.is_none() && self.deferred.take().is_some() {
            self.dirty = true;
        }
    }

    /// The planned switch's candidate failed: its stamp is rejected (review L2 keeps whether
    /// it was already tried again).
    fn reject_planned(&mut self) {
        let now = self.host.now_ms();
        if let Some(planned) = self.planned.take() {
            let retried = self
                .rejected
                .as_ref()
                .is_some_and(|rejected| rejected.stamp == planned.stamp && rejected.retried);
            self.rejected = Some(Rejected {
                stamp: planned.stamp,
                at_ms: now,
                retried,
            });
        }
    }

    fn modern_only(&self) -> bool {
        self.handshake.init_params.is_none() && self.handshake.modern_meta.is_some()
    }

    fn modern_params(&self) -> Value {
        match &self.handshake.modern_meta {
            Some(meta) => json!({"_meta": meta}),
            None => json!({}),
        }
    }

    fn begin(&mut self, slot: Slot, purpose: Purpose) {
        let step = match purpose {
            Purpose::Fresh if self.handshake.init_params.is_some() => Step::Initialize,
            Purpose::Fresh => Step::Ready,
            Purpose::Switch if self.modern_only() => Step::Discover,
            Purpose::Switch => Step::Initialize,
        };
        self.starting = Some(Starting {
            slot,
            purpose,
            step,
            request: None,
            deadline_ms: self
                .host
                .now_ms()
                .saturating_add(self.tunables.verify_timeout_ms),
        });
        self.dirty = true;
        self.send_step();
    }

    fn send_step(&mut self) {
        let Some((child, step)) = self
            .starting
            .as_ref()
            .map(|starting| (starting.slot.id, starting.step))
        else {
            return;
        };
        let (method, params) = match step {
            Step::Initialize => (
                "initialize",
                self.handshake.init_params.clone().unwrap_or_else(|| {
                    json!({
                        "protocolVersion": PROBE_VERSION,
                        "capabilities": {},
                        "clientInfo": {"name": "actingcommand-mcp-entry", "version": GENERATION.to_string()},
                    })
                }),
            ),
            Step::Discover => ("server/discover", self.modern_params()),
            Step::ToolsList if self.modern_only() => ("tools/list", self.modern_params()),
            Step::ToolsList => ("tools/list", json!({})),
            Step::Ready => {
                self.on_ready();
                return;
            }
        };
        match self.internal_request(child, method, params) {
            Some(key) => {
                if let Some(starting) = self.starting.as_mut() {
                    starting.request = Some(key);
                }
            }
            None => self.fail_starting(CandidateProblem::Exited),
        }
    }

    fn step_answer(&mut self, message: &Value) {
        let Some((step, purpose, child)) = self
            .starting
            .as_ref()
            .map(|starting| (starting.step, starting.purpose, starting.slot.id))
        else {
            return;
        };
        let result = message.get("result").filter(|result| result.is_object());
        let next = match step {
            Step::Initialize => {
                let Some(result) = result else {
                    self.fail_starting(CandidateProblem::InitializeFailed);
                    return;
                };
                let version = result.get("protocolVersion").and_then(Value::as_str);
                let changed = self
                    .handshake
                    .negotiated
                    .as_deref()
                    .is_some_and(|expected| version != Some(expected));
                if changed {
                    self.fail_starting(CandidateProblem::ProtocolVersionChanged);
                    return;
                }
                let replay = self.handshake.initialized || self.handshake.init_params.is_none();
                if replay
                    && !self.host.send(
                        child,
                        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                    )
                {
                    self.fail_starting(CandidateProblem::Exited);
                    return;
                }
                if purpose == Purpose::Switch {
                    Step::ToolsList
                } else {
                    Step::Ready
                }
            }
            Step::Discover => {
                let supported = result
                    .and_then(|result| result.get("supportedVersions"))
                    .and_then(Value::as_array)
                    .is_some_and(|versions| {
                        versions
                            .iter()
                            .any(|version| version.as_str() == Some(MODERN_VERSION))
                    });
                if !supported {
                    self.fail_starting(CandidateProblem::ProtocolVersionChanged);
                    return;
                }
                Step::ToolsList
            }
            Step::ToolsList => {
                if !result
                    .and_then(|result| result.get("tools"))
                    .is_some_and(Value::is_array)
                {
                    self.fail_starting(CandidateProblem::ToolsListFailed);
                    return;
                }
                Step::Ready
            }
            Step::Ready => return,
        };
        if let Some(starting) = self.starting.as_mut() {
            starting.step = next;
            starting.request = None;
        }
        self.dirty = true;
        self.send_step();
    }

    fn on_ready(&mut self) {
        let Some(purpose) = self.starting.as_ref().map(|starting| starting.purpose) else {
            return;
        };
        match purpose {
            Purpose::Fresh => {
                let Some(mut starting) = self.starting.take() else {
                    return;
                };
                // Review L3: the build its predecessor ran, to compare with the next plan's.
                starting.slot.previous_build =
                    self.history.front().and_then(|past| past.build.clone());
                self.current = Some(starting.slot);
                // Its build is learnt by the next plan, even when the install's stamp is one
                // whose candidates were refused (re-review N1).
                self.accepted = None;
                self.rejected = None;
                self.handles_clear = false;
                self.dirty = true;
                let held = std::mem::take(&mut self.held);
                for (line, requests) in held {
                    self.deliver(line, requests);
                }
            }
            Purpose::Switch => self.try_switch(),
        }
    }

    fn fail_starting(&mut self, problem: CandidateProblem) {
        let Some(starting) = self.starting.take() else {
            return;
        };
        self.host.stop(starting.slot.id);
        self.host.note(&format!(
            "the {} child pid {} failed ({problem:?}); the current child keeps serving",
            match starting.purpose {
                Purpose::Switch => "candidate",
                Purpose::Fresh => "fresh",
            },
            starting.slot.pid
        ));
        self.failure = Some(CandidateFailure {
            at_unix_ms: self.host.now_ms(),
            purpose: starting.purpose,
            pid: Some(starting.slot.pid),
            build: starting.slot.build,
            problem,
        });
        match starting.purpose {
            Purpose::Switch => {
                self.reject_planned();
                self.deferred = None;
            }
            Purpose::Fresh => self.fail_held(),
        }
        self.dirty = true;
    }

    /// The held requests were never sent: each gets `mcp_entry_child_unavailable`.
    fn fail_held(&mut self) {
        let held = std::mem::take(&mut self.held);
        for (_, requests) in held {
            for (_, pending) in requests {
                self.answer_error(&pending, EntryCode::ChildUnavailable);
            }
        }
    }

    fn commit_switch(&mut self) {
        let (Some(starting), Some(planned)) = (self.starting.take(), self.planned.take()) else {
            return;
        };
        let now = self.host.now_ms();
        let new = starting.slot;
        if let Some(old) = self.current.take() {
            self.host.stop(old.id);
            self.last_switch = Some(SwitchRecord {
                at_unix_ms: now,
                from_pid: old.pid,
                from_build: old.build.clone(),
                to_pid: new.pid,
                to_build: new.build.clone(),
            });
            self.retire(&old, now, Ended::Switched);
        }
        self.host.note(&format!(
            "switched to child pid {} of build {}",
            new.pid, planned.build.commit
        ));
        self.current = Some(new);
        self.accepted = Some(planned.stamp);
        self.pending = None;
        self.rejected = None;
        self.handles_clear = false;
        self.deferred = None;
        self.send_notices(now);
        self.dirty = true;
    }

    /// D5: the change notice of each generation the client speaks.
    fn send_notices(&mut self, now: u64) {
        let legacy = self.handshake.init_params.is_some() && self.handshake.initialized;
        if legacy {
            self.send_client(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed",
            }));
        }
        let subscriptions = self
            .listens
            .values()
            .filter(|(_, tools)| *tools)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in &subscriptions {
            let mut meta = Map::new();
            meta.insert(META_SUBSCRIPTION_ID.to_owned(), id.clone());
            self.send_client(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed",
                "params": {"_meta": meta},
            }));
        }
        self.notice = Some(Notice {
            sent_at_unix_ms: now,
            legacy,
            listen_subscriptions: subscriptions.len(),
            tools_list_after: false,
            tools_list_at_unix_ms: None,
        });
    }

    fn retire(&mut self, old: &Slot, now: u64, ended: Ended) {
        self.history.push_front(PastChild {
            pid: old.pid,
            build: old.build.clone(),
            since_unix_ms: old.since_ms,
            until_unix_ms: now,
            ended,
        });
        self.history.truncate(HISTORY_LIMIT);
    }

    fn internal_request(&mut self, child: u64, method: &str, params: Value) -> Option<String> {
        self.seq += 1;
        let key = format!("{INTERNAL_PREFIX}{}", self.seq);
        let message = json!({"jsonrpc": "2.0", "id": key, "method": method, "params": params});
        self.host
            .send(child, message.to_string().as_bytes())
            .then_some(key)
    }

    // ---- status -----------------------------------------------------------------------------

    fn finish_event(&mut self) {
        if self.dirty {
            self.dirty = false;
            let status = self.status();
            self.host.write_status(&status);
        }
    }

    /// The status file (D6).
    pub(crate) fn status(&self) -> Value {
        let generation = if self.handshake.init_params.is_some() {
            Some("legacy")
        } else if self.handshake.modern_meta.is_some() {
            Some("modern")
        } else {
            None
        };
        let version = if self.handshake.init_params.is_some() {
            self.handshake.negotiated.clone()
        } else {
            self.handshake
                .modern_meta
                .as_ref()
                .and_then(|meta| meta.get(META_PROTOCOL_VERSION))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let child = self.current.as_ref().map(
            |slot| json!({"pid": slot.pid, "build": slot.build, "since_unix_ms": slot.since_ms}),
        );
        let starting = self.starting.as_ref().map(|starting| {
            json!({
                "pid": starting.slot.pid,
                "build": starting.slot.build,
                "purpose": starting.purpose,
                "step": starting.step,
            })
        });
        let refresh_entry = self
            .config
            .client_kind
            .map_or(RefreshEntry::ClientRestart, ClientKind::refresh_entry);
        let refresh_needed = self
            .notice
            .as_ref()
            .is_some_and(|notice| !notice.tools_list_after);
        json!({
            "schema_version": STATUS_SCHEMA,
            "entry_generation": GENERATION,
            "entry_protocol": ENTRY_PROTOCOL,
            "pid": self.config.pid,
            "process_created_at_unix_ms": self.config.process_created_at_unix_ms,
            "started_at_unix_ms": self.started_ms,
            "updated_at_unix_ms": self.host.now_ms(),
            "closing": self.closing,
            "client": {
                "configured": self.config.client_kind,
                "name": self.handshake.client_name,
                "version": self.handshake.client_version,
            },
            "protocol": {"generation": generation, "version": version},
            "child": child,
            "starting": starting,
            "previous_children": self.history,
            "last_switch": self.last_switch,
            "pending_change_since_unix_ms": self.pending.as_ref().map(|(_, since)| *since),
            "deferred": self.deferred,
            "last_candidate_failure": self.failure,
            "last_plan": self.last_plan,
            "plan_refusals": {"consecutive": self.plan_refusals, "next_plan_at_unix_ms": self.plan_retry_at},
            "notice": self.notice,
            "refresh": {"entry": refresh_entry, "needed": refresh_needed},
            "calls_in_flight": self.inflight.len(),
            "listen_subscriptions": self.listens.len(),
            "tunables": self.tunables,
        })
    }
}
