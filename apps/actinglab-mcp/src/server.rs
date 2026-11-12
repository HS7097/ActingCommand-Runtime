// SPDX-License-Identifier: AGPL-3.0-only

//! The stdio server (#338 §四 线程 / 启动预算 / 关闭 / 双代协议), copied from today's MCP server
//! (`apps/actingctl/src/mcp/server.rs`; Workflow #434 LAB-5G2a: copied, not shared). One thread
//! reads stdin (lines up to 4 MiB), the calling thread dispatches, the workers `--workers` names
//! (eight unless it says otherwise; board #434, the coordinator's ruling 6775192250, points 20 and
//! 22) run `tools/call`, one thread writes stdout in order and one sends progress. stdout carries protocol messages only; diagnostics go
//! to stderr. Startup reads no file and opens no connection, so `initialize` and
//! `server/discover` answer at once. At stdin EOF the server takes no new request, gives answers
//! in flight up to 2 s and exits with 0. Every JSON-RPC error carries its outcome
//! (`protocol::error_response`); a server that ends on a failure returns that failure's outcome,
//! which `main` reports on stderr.
//!
//! What differs from the original: no tiers (every offered row's tool may be called, listed or
//! not); `tools/list` is the table's (`tools::Catalog`); a `tools/call` of an offered row's tool
//! runs on a worker as before and (Workflow #434 LAB-5G2b) is served by `calls::Service` (its
//! Runtime connection and this server's session identity are there), or answers
//! `mcp_operation_unsupported` for a row this build does not serve. The worker count is the start
//! option `--workers` (`ServerConfig::workers`), where the original has a constant.
//!
//! Workflow #434 LAB-5G3 (register §1.287, "Slice LAB-5G3"; the survey's §3): the jobs of the
//! original (`apps/actingctl/src/mcp/server.rs` and `operator.rs`, `job_answer`, `run_pack`,
//! `stop_run`; `runs.rs`, `get_run`), adapted to the generated tools ([`answer`]):
//! - every served call runs as a job (`crate::jobs`); one not done within the call budget (the
//!   start option `--call-budget`, today's 25 s, less today's 2 s for the answer) answers
//!   `{handle, job_phase}` (`mcp_job_started`) and its job goes on; a row this build does not
//!   serve and arguments it refuses answer at once, with no job;
//! - `ac_run_show`'s `id` also takes a job handle, and its `wait_s` waits for the job or reads a
//!   run again every second while it may change (today's `ac_get_run`); `ac_run_stop`'s `wait_s`
//!   is how long its call waits for its job; `ac_run_start` answers at once with the run's
//!   request id, the run submitted in a job;
//! - a line that fits the budget neither way answers `output_budget_exceeded` with its job's
//!   handle, and `ac_run_show` of the handle gives the job's summary (work order point 10);
//! - `actingcommand/entryStatus` counts the jobs as the original counts them; progress and
//!   cancellation are the original's, for the call in flight: a cancelled call's handle is never
//!   issued, and its job goes on; at stdin EOF the jobs are not waited for.
//!
//! Workflow #434 LAB-5G4b (register §1.287, "Slice LAB-5G4b"): a row neither this process's core
//! serving code nor the rescue module serves runs as a job like every served call, its line from a
//! `serve` child (`crate::children`); at stdin EOF every child's input is closed
//! (`calls::Service::end_of_input`) and none is killed.

use crate::budget;
use crate::calls::{self, FrontMembers, Service};
use crate::codes::McpCode;
use crate::jobs::{self, Jobs, Limits};
use crate::lock;
use crate::protocol::{
    self, Era, INVALID_PARAMS, INVALID_REQUEST, Incoming, LegacyVersion, METHOD_NOT_FOUND,
    MetaVersion, PARSE_ERROR,
};
use crate::tools::{self, Catalog, Listing};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_lab_ops::core_ops;
use actingcommand_lab_ops::events;
use actingcommand_lab_ops::table::Operation;
use actingcommand_runtime_client::ContainedRunState;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const PROGRESS_TICK: Duration = Duration::from_millis(500);
/// The MCP entry's private request (Workflow #421 MCP-ENTRY D4).
const ENTRY_STATUS_METHOD: &str = "actingcommand/entryStatus";
/// How long answers in flight may still be written after stdin ends.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const SHUTDOWN_POLL: Duration = Duration::from_millis(20);
/// Workflow #434 LAB-5G3: the rows whose calls the front answers itself, and the members of theirs
/// it reads.
const RUN_SHOW: &str = "run show";
const RUN_START: &str = "run start";
const RUN_STOP: &str = "run stop";
const ID: &str = "id";
const WAIT: &str = "wait";
const CANCEL_ONLY: &str = "cancel_only";
/// The member of a run's line that says its state.
const STATE: &str = "state";
/// The members of the answers about a job.
const HANDLE: &str = "handle";
const JOB_PHASE: &str = "job_phase";
const CORRELATION_ID: &str = "correlation_id";
const WARNINGS: &str = "warnings";
const SUMMARY: &str = "summary";
const JOB: &str = "job";
const KIND: &str = "kind";
const LINE: &str = "line";
/// How often a run is read again while it may change (today's `ac_get_run`).
const RUN_POLL: Duration = Duration::from_secs(1);
/// Kept back from the call budget for the answer after a wait for a job, and for the last read of
/// a run's wait: today's 2 s and 5 s, never more than a tenth and a fifth of a shorter budget.
const ANSWER_RESERVE: Duration = Duration::from_secs(2);
const READ_RESERVE: Duration = Duration::from_secs(5);

pub(crate) struct ServerConfig {
    /// Every tool this build offers.
    pub(crate) catalog: Catalog,
    /// Which of them `tools/list` lists.
    pub(crate) listing: Listing,
    /// The threads that run `tools/call` (`--workers`, at least one).
    pub(crate) workers: usize,
    /// Workflow #434 LAB-5G2b: what serves the calls.
    pub(crate) service: Service,
    /// Workflow #434 LAB-5G3: the call budget (`--call-budget`) and the jobs' two caps (`--jobs`,
    /// `--kept-jobs`).
    pub(crate) call_budget: Duration,
    pub(crate) limits: Limits,
}

struct Shared {
    catalog: Catalog,
    listing: Listing,
    service: Service,
    /// Workflow #434 LAB-5G3: this server's jobs, its call budget and its instructions, which name
    /// the budget.
    jobs: Jobs,
    call_budget: Duration,
    instructions: String,
    /// The version a legacy `initialize` negotiated; `None` until one arrives.
    legacy: Mutex<Option<LegacyVersion>>,
    /// `tools/call` requests in flight, by their JSON-RPC id.
    calls: Mutex<HashMap<String, Arc<Call>>>,
    out: Sender<Outgoing>,
    output_failed: Arc<AtomicBool>,
    /// The stdout writer's failure, once it failed.
    output_error: Arc<Mutex<Option<Outcome>>>,
}

enum Outgoing {
    Line(String),
    Finish,
}

struct Call {
    cancelled: AtomicBool,
    started: Instant,
    progress_token: Option<Value>,
    phase: Mutex<CallPhase>,
}

/// Guarded together so no progress notification follows the call's answer.
#[derive(Default)]
struct CallPhase {
    progress_sent: u32,
    finished: bool,
}

struct Job {
    id: Value,
    key: String,
    era: Era,
    /// The tool the call names, an offered row's.
    tool: String,
    /// Workflow #434 LAB-5G2b: the call's arguments (none given: empty).
    arguments: Map<String, Value>,
    call: Arc<Call>,
    sink: Sink,
}

/// Where an answer goes: straight to stdout, or into a 2025-03-26 batch reply.
#[derive(Clone)]
enum Sink {
    Direct,
    Batch(Arc<Batch>),
}

#[derive(Default)]
struct Batch {
    state: Mutex<BatchState>,
}

#[derive(Default)]
struct BatchState {
    open: usize,
    sealed: bool,
    responses: Vec<Value>,
}

enum Inbound {
    Line(Vec<u8>),
    TooLong,
    End,
    Failed(io::Error),
}

impl Shared {
    fn emit(&self, message: &Value) {
        if self.out.send(Outgoing::Line(message.to_string())).is_err() {
            self.output_failed.store(true, Ordering::SeqCst);
        }
    }
}

impl Sink {
    fn respond(&self, shared: &Shared, response: Option<Value>) {
        match self {
            Self::Direct => {
                if let Some(response) = response {
                    shared.emit(&response);
                }
            }
            Self::Batch(batch) => batch.complete(shared, response),
        }
    }
}

impl Batch {
    fn open(&self) {
        lock(&self.state).open += 1;
    }

    fn complete(&self, shared: &Shared, response: Option<Value>) {
        let mut state = lock(&self.state);
        if let Some(response) = response {
            state.responses.push(response);
        }
        state.open = state.open.saturating_sub(1);
        Self::flush(shared, &mut state);
    }

    fn seal(&self, shared: &Shared) {
        let mut state = lock(&self.state);
        state.sealed = true;
        Self::flush(shared, &mut state);
    }

    /// One array of every answer once the whole batch is answered; nothing when the batch
    /// held notifications only.
    fn flush(shared: &Shared, state: &mut BatchState) {
        if state.sealed && state.open == 0 && !state.responses.is_empty() {
            shared.emit(&Value::Array(std::mem::take(&mut state.responses)));
        }
    }
}

/// Serves until stdin ends: `Ok` at a clean end; otherwise the outcome that ended the server
/// (`mcp_server_start_failed`, `mcp_input_failed` or `actingctl_output_failed`).
pub(crate) fn serve(config: ServerConfig) -> Result<ExitCode, Outcome> {
    let output_failed = Arc::new(AtomicBool::new(false));
    let output_error = Arc::new(Mutex::new(None));
    let (out_sender, out_receiver) = mpsc::channel::<Outgoing>();
    let writer_failed = Arc::clone(&output_failed);
    let writer_error = Arc::clone(&output_error);
    let writer = spawn("mcp-stdout", move || {
        write_loop(&out_receiver, &writer_failed, &writer_error);
    })?;
    let shared = Arc::new(Shared {
        catalog: config.catalog,
        listing: config.listing,
        service: config.service,
        jobs: Jobs::new(config.limits, events::process_session()),
        call_budget: config.call_budget,
        instructions: tools::instructions(config.call_budget.as_secs()),
        legacy: Mutex::new(None),
        calls: Mutex::new(HashMap::new()),
        out: out_sender,
        output_failed,
        output_error,
    });
    let (job_sender, job_receiver) = mpsc::channel::<Job>();
    let job_receiver = Arc::new(Mutex::new(job_receiver));
    // Workers, the progress sender and the stdin reader run detached; the process ends
    // with this function.
    for index in 0..config.workers {
        let shared = Arc::clone(&shared);
        let jobs = Arc::clone(&job_receiver);
        let _worker = spawn(&format!("mcp-worker-{index}"), move || {
            worker_loop(&shared, &jobs);
        })?;
    }
    // Only the workers hold the job queue: if every one of them is gone, a send fails.
    drop(job_receiver);
    let (stop_progress, progress_stop) = mpsc::channel::<()>();
    let progress_shared = Arc::clone(&shared);
    let _progress = spawn("mcp-progress", move || {
        progress_loop(&progress_shared, &progress_stop);
    })?;
    let (in_sender, in_receiver) = mpsc::channel::<Inbound>();
    let _reader = spawn("mcp-stdin", move || read_loop(&in_sender))?;

    let mut input_error = None;
    for inbound in in_receiver {
        match inbound {
            Inbound::Line(line) => handle_line(&shared, &job_sender, &line),
            Inbound::TooLong => shared.emit(&protocol::error(
                &Value::Null,
                PARSE_ERROR,
                &Outcome::new(ActingctlCode::McpMessageTooLarge).limit_bytes(MAX_LINE_BYTES),
            )),
            Inbound::End => break,
            Inbound::Failed(error) => {
                input_error = Some(error);
                break;
            }
        }
    }

    // Shutdown: no new calls; answers in flight get up to 2 s; background waits are dropped.
    // Workflow #434 LAB-5G4b: every `serve` child's input is closed, an idle one's now and a
    // working one's once its line came; none is killed.
    shared.service.end_of_input();
    drop(job_sender);
    let grace_end = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < grace_end && !lock(&shared.calls).is_empty() {
        thread::sleep(SHUTDOWN_POLL);
    }
    drop(stop_progress);
    if shared.out.send(Outgoing::Finish).is_ok() {
        while !writer.is_finished() && Instant::now() < grace_end + SHUTDOWN_GRACE {
            thread::sleep(SHUTDOWN_POLL);
        }
    }
    if let Some(error) = input_error {
        return Err(Outcome::new(ActingctlCode::McpInputFailed)
            .caused_by(Outcome::from_io_error(&error, IoOp::Read)));
    }
    if shared.output_failed.load(Ordering::SeqCst) {
        let failed = Outcome::new(ActingctlCode::ActingctlOutputFailed);
        return Err(match lock(&shared.output_error).take() {
            Some(cause) => failed.caused_by(cause),
            None => failed,
        });
    }
    Ok(ExitCode::SUCCESS)
}

/// One named thread of the server; a thread the OS refuses is `mcp_server_start_failed` with
/// its name (`thread`).
fn spawn(
    name: &str,
    body: impl FnOnce() + Send + 'static,
) -> Result<thread::JoinHandle<()>, Outcome> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map_err(|error| {
            Outcome::new(ActingctlCode::McpServerStartFailed)
                .thread(name)
                .caused_by(Outcome::from_io_error(&error, IoOp::Spawn))
        })
}

/// Writes every message in order; a failed write keeps its `foreign_os_error` for the end of
/// the server and stops writing.
fn write_loop(messages: &Receiver<Outgoing>, failed: &AtomicBool, error: &Mutex<Option<Outcome>>) {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for message in messages {
        let Outgoing::Line(line) = message else {
            return;
        };
        if let Err(write_error) = write_line(&mut output, &line) {
            *lock(error) = Some(Outcome::from_io_error(&write_error, IoOp::Write));
            failed.store(true, Ordering::SeqCst);
            return;
        }
    }
}

fn write_line(output: &mut impl Write, line: &str) -> io::Result<()> {
    output.write_all(line.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()
}

fn read_loop(sender: &Sender<Inbound>) {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    loop {
        let inbound = read_line(&mut input).unwrap_or_else(Inbound::Failed);
        let last = matches!(inbound, Inbound::End | Inbound::Failed(_));
        if sender.send(inbound).is_err() || last {
            return;
        }
    }
}

/// One newline-delimited message of at most 4 MiB; a longer one is skipped to its end.
fn read_line(input: &mut impl BufRead) -> io::Result<Inbound> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = match input.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(match (too_long, line.is_empty()) {
                (true, _) => Inbound::TooLong,
                (false, true) => Inbound::End,
                (false, false) => Inbound::Line(line),
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let chunk = &available[..newline.unwrap_or(available.len())];
        if !too_long {
            if line.len() + chunk.len() > MAX_LINE_BYTES {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let consumed = chunk.len() + usize::from(newline.is_some());
        input.consume(consumed);
        if newline.is_some() {
            return Ok(if too_long {
                Inbound::TooLong
            } else {
                Inbound::Line(line)
            });
        }
    }
}

fn handle_line(shared: &Shared, jobs: &Sender<Job>, line: &[u8]) {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.iter().all(u8::is_ascii_whitespace) {
        return;
    }
    match serde_json::from_slice::<Value>(line) {
        // The parser's text is foreign: it travels as `raw_text` of a `foreign_data_error`.
        Err(error) => shared.emit(&protocol::error(
            &Value::Null,
            PARSE_ERROR,
            &Outcome::new(ActingctlCode::McpMessageMalformed).caused_by(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(error.to_string()),
            ),
        )),
        Ok(Value::Array(items)) => handle_batch(shared, jobs, items),
        Ok(message) => handle_message(shared, jobs, message, &Sink::Direct),
    }
}

/// 2025-03-26 takes JSON-RPC batches and answers them with one array; every later version
/// and the modern era refuse a batch array with -32600 (`mcp_batch_unsupported`), and an empty
/// batch is refused in every version (`mcp_batch_empty`).
fn handle_batch(shared: &Shared, jobs: &Sender<Job>, items: Vec<Value>) {
    let batching = *lock(&shared.legacy) == Some(LegacyVersion::V20250326);
    if items.is_empty() || !batching {
        let refusal = if items.is_empty() {
            ActingctlCode::McpBatchEmpty
        } else {
            ActingctlCode::McpBatchUnsupported
        };
        shared.emit(&protocol::error(
            &Value::Null,
            INVALID_REQUEST,
            &Outcome::new(refusal),
        ));
        return;
    }
    let batch = Arc::new(Batch::default());
    let sink = Sink::Batch(Arc::clone(&batch));
    for item in items {
        batch.open();
        handle_message(shared, jobs, item, &sink);
    }
    batch.seal(shared);
}

fn handle_message(shared: &Shared, jobs: &Sender<Job>, message: Value, sink: &Sink) {
    match protocol::classify(message) {
        Incoming::Invalid { id } => sink.respond(
            shared,
            Some(protocol::error(
                &id,
                INVALID_REQUEST,
                &Outcome::new(ActingctlCode::McpRequestInvalid),
            )),
        ),
        Incoming::Response => sink.respond(shared, None),
        Incoming::Notification { method, params } => {
            handle_notification(shared, &method, params.as_ref());
            sink.respond(shared, None);
        }
        Incoming::Request { id, method, params } => {
            handle_request(shared, jobs, id, &method, params, sink);
        }
    }
}

/// `notifications/cancelled` stops waiting for that call and suppresses its answer;
/// `notifications/initialized` and any other notification need nothing.
fn handle_notification(shared: &Shared, method: &str, params: Option<&Value>) {
    if method != "notifications/cancelled" {
        return;
    }
    let Some(request_id) = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("requestId"))
    else {
        return;
    };
    if let Some(call) = lock(&shared.calls).get(&request_id.to_string()) {
        call.cancelled.store(true, Ordering::SeqCst);
    }
}

fn handle_request(
    shared: &Shared,
    jobs: &Sender<Job>,
    id: Value,
    method: &str,
    params: Option<Value>,
    sink: &Sink,
) {
    if method == "initialize" {
        let response = initialize(shared, &id, params.as_ref());
        return sink.respond(shared, Some(response));
    }
    // Workflow #421 MCP-ENTRY (D4): the MCP entry's private question before it switches, answered
    // in either era and without a session.
    if method == ENTRY_STATUS_METHOD {
        return sink.respond(shared, Some(protocol::response(&id, entry_status(shared))));
    }
    let era = match protocol::meta_version(params.as_ref()) {
        MetaVersion::Modern => Era::Modern,
        MetaVersion::Unsupported(requested) => {
            return sink.respond(shared, Some(protocol::unsupported_version(&id, &requested)));
        }
        MetaVersion::Malformed(refusal) => {
            return sink.respond(shared, Some(protocol::error(&id, INVALID_PARAMS, &refusal)));
        }
        // A legacy ping is answered with or without a session.
        MetaVersion::Absent if method == "ping" => {
            return sink.respond(shared, Some(protocol::response(&id, json!({}))));
        }
        MetaVersion::Absent => {
            let legacy = *lock(&shared.legacy);
            match legacy {
                Some(version) if method != "server/discover" => Era::Legacy(version),
                // Neither a legacy session nor a `_meta` protocol version: the dual-era
                // server cannot tell the request's protocol.
                _ => {
                    return sink.respond(
                        shared,
                        Some(protocol::error(
                            &id,
                            INVALID_PARAMS,
                            &Outcome::new(ActingctlCode::McpProtocolVersionMissing),
                        )),
                    );
                }
            }
        }
    };
    match method {
        "server/discover" => {
            sink.respond(shared, Some(protocol::discover(&id, &shared.instructions)))
        }
        "tools/list" => sink.respond(shared, Some(list_tools(shared, &id, era, params.as_ref()))),
        "tools/call" => call_tool(shared, jobs, id, era, params, sink),
        _ => sink.respond(
            shared,
            Some(protocol::error(
                &id,
                METHOD_NOT_FOUND,
                &Outcome::new(ActingctlCode::McpMethodUnknown),
            )),
        ),
    }
}

/// `actingcommand/entryStatus` (Workflow #421 MCP-ENTRY D4), today's server's answer: the handles
/// only this process can answer for (`outstanding_handles`, their `kinds`, and the `handles`
/// themselves so a person can collect a stuck one with `ac_run_show`) and the `tools/call`
/// requests still running (`calls_in_flight`, a cancelled one included until it ends). Workflow
/// #434 LAB-5G3: a job counts while it runs and, once it ended, until its line was read; a run's
/// job only while it submits (`jobs::Jobs::outstanding`). The second entry switches only at 0.
fn entry_status(shared: &Shared) -> Value {
    let outstanding = shared.jobs.outstanding();
    let calls = lock(&shared.calls).len();
    json!({
        "outstanding_handles": outstanding.handles.len(),
        "kinds": outstanding.kinds,
        "handles": outstanding.handles,
        "calls_in_flight": calls,
    })
}

fn initialize(shared: &Shared, id: &Value, params: Option<&Value>) -> Value {
    let requested = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("protocolVersion"));
    let requested = match requested {
        Some(Value::String(requested)) => requested,
        Some(_) => {
            return protocol::error(
                id,
                INVALID_PARAMS,
                &protocol::params_invalid("protocolVersion", ArgumentProblem::WrongType),
            );
        }
        None => {
            return protocol::error(
                id,
                INVALID_PARAMS,
                &protocol::params_invalid("protocolVersion", ArgumentProblem::Required),
            );
        }
    };
    let version = LegacyVersion::negotiate(requested);
    *lock(&shared.legacy) = Some(version);
    protocol::response(
        id,
        protocol::initialize_result(version, &shared.instructions),
    )
}

fn list_tools(shared: &Shared, id: &Value, era: Era, params: Option<&Value>) -> Value {
    if params
        .and_then(Value::as_object)
        .and_then(|params| params.get("cursor"))
        .is_some_and(|cursor| !cursor.is_null())
    {
        // tools/list has one page: this server issues no cursor.
        return protocol::error(
            id,
            INVALID_PARAMS,
            &protocol::params_invalid("cursor", ArgumentProblem::Unusable),
        );
    }
    let mut result = shared.catalog.list_result(&shared.listing);
    if era == Era::Modern {
        // 2026-07-28 `ListToolsResult extends CacheableResult` (SEP-2549): both fields required.
        result["ttlMs"] = json!(0);
        result["cacheScope"] = json!("private");
    }
    protocol::response(id, protocol::complete(era, result))
}

/// What a `CallToolRequest` names.
struct CallRequest {
    tool: String,
    /// Workflow #434 LAB-5G2b: its arguments, which `calls` reads.
    arguments: Map<String, Value>,
    progress_token: Option<Value>,
}

/// The tool a `tools/call` names, an offered row's whether listed or not; a refusal is
/// `mcp_params_invalid` with the member at fault, as today's server refuses it (a name that is no
/// offered row's tool `not_recognised`). Its `arguments` must be an object when given (absent or
/// `null`: none); `calls` reads them for a row this build serves.
fn parse_call(catalog: &Catalog, params: Option<Value>) -> Result<CallRequest, Outcome> {
    let mut params = match params {
        Some(Value::Object(params)) => params,
        Some(_) => {
            return Err(protocol::params_invalid(
                "params",
                ArgumentProblem::WrongType,
            ));
        }
        None => {
            return Err(protocol::params_invalid(
                "params",
                ArgumentProblem::Required,
            ));
        }
    };
    let tool = match params.get("name") {
        Some(Value::String(name)) if catalog.offers(name) => name.clone(),
        Some(Value::String(_)) => {
            return Err(protocol::params_invalid(
                "name",
                ArgumentProblem::NotRecognised,
            ));
        }
        Some(_) => return Err(protocol::params_invalid("name", ArgumentProblem::WrongType)),
        None => return Err(protocol::params_invalid("name", ArgumentProblem::Required)),
    };
    let arguments = match params.remove("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(arguments)) => arguments,
        Some(_) => {
            return Err(protocol::params_invalid(
                "arguments",
                ArgumentProblem::WrongType,
            ));
        }
    };
    let progress_token = params
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("progressToken"))
        .filter(|token| token.is_string() || token.is_i64() || token.is_u64())
        .cloned();
    Ok(CallRequest {
        tool,
        arguments,
        progress_token,
    })
}

fn call_tool(
    shared: &Shared,
    jobs: &Sender<Job>,
    id: Value,
    era: Era,
    params: Option<Value>,
    sink: &Sink,
) {
    let CallRequest {
        tool,
        arguments,
        progress_token,
    } = match parse_call(&shared.catalog, params) {
        Ok(request) => request,
        Err(refusal) => {
            return sink.respond(shared, Some(protocol::error(&id, INVALID_PARAMS, &refusal)));
        }
    };
    let key = id.to_string();
    let call = Arc::new(Call {
        cancelled: AtomicBool::new(false),
        started: Instant::now(),
        progress_token,
        phase: Mutex::new(CallPhase::default()),
    });
    let duplicate = {
        let mut calls = lock(&shared.calls);
        if calls.contains_key(&key) {
            true
        } else {
            calls.insert(key.clone(), Arc::clone(&call));
            false
        }
    };
    if duplicate {
        // A request with this id is still in flight.
        return sink.respond(
            shared,
            Some(protocol::error(
                &id,
                INVALID_REQUEST,
                &Outcome::new(ActingctlCode::McpRequestIdInUse),
            )),
        );
    }
    let job = Job {
        id,
        key,
        era,
        tool,
        arguments,
        call,
        sink: sink.clone(),
    };
    if let Err(mpsc::SendError(job)) = jobs.send(job) {
        lock(&shared.calls).remove(&job.key);
        // No tools/call worker is running.
        let failure = Outcome::new(ActingctlCode::McpWorkersUnavailable);
        let result = calls::refusal_result(&failure, job.era);
        job.sink.respond(
            shared,
            Some(protocol::response(
                &job.id,
                protocol::complete(job.era, result),
            )),
        );
    }
}

fn worker_loop(shared: &Arc<Shared>, jobs: &Mutex<Receiver<Job>>) {
    loop {
        let next = lock(jobs).recv();
        let Ok(job) = next else {
            return;
        };
        run_job(shared, job);
    }
}

fn run_job(shared: &Arc<Shared>, job: Job) {
    let Job {
        id,
        key,
        era,
        tool,
        arguments,
        call,
        sink,
    } = job;
    // Workflow #434 LAB-5G3: the call is answered within its budget; its work runs as a job.
    let answered = panic::catch_unwind(AssertUnwindSafe(|| {
        answer(shared, &tool, arguments, era, &call)
    }))
    .unwrap_or_else(|_| Answered::now(calls::refusal_result(&tool_panicked(&tool), era)));
    let response = protocol::response(&id, protocol::complete(era, answered.result));
    let mut phase = lock(&call.phase);
    phase.finished = true;
    // A cancelled request gets no answer, so the handle it carried was never issued (Workflow
    // #421 MCP-ENTRY fix round 1, review M2). Read once: the same reading decides both.
    let delivered = !call.cancelled.load(Ordering::SeqCst);
    if delivered && let Some(handle) = &answered.handle {
        shared.jobs.issue(handle);
    }
    lock(&shared.calls).remove(&key);
    sink.respond(shared, delivered.then_some(response));
}

/// Workflow #434 LAB-5G3: what a call answers, and the handle of the job its answer names, which
/// is issued when the answer is delivered.
struct Answered {
    result: Value,
    handle: Option<String>,
}

impl Answered {
    fn now(result: Value) -> Self {
        Self {
            result,
            handle: None,
        }
    }
}

/// Workflow #434 LAB-5G3: one call being answered on a worker: the server, the row and the tool
/// it names, its protocol era and its state in flight (its start and its cancellation).
struct Asked<'a> {
    shared: &'a Arc<Shared>,
    operation: &'a Operation,
    tool: &'a str,
    era: Era,
    call: &'a Call,
}

impl Asked<'_> {
    /// The latest a wait for a job ends: the call budget from the call's start, less the room to
    /// answer (today's 2 s, at most a tenth of the budget).
    fn last_wait(&self) -> Instant {
        let budget = self.shared.call_budget;
        self.call.started + budget - ANSWER_RESERVE.min(budget / 10)
    }

    /// The latest a run's wait reads it again: the call budget less the room for that last read
    /// (today's 5 s, at most a fifth of the budget).
    fn last_read(&self) -> Instant {
        let budget = self.shared.call_budget;
        self.call.started + budget - READ_RESERVE.min(budget / 5)
    }

    /// The answer of the line `exit`.
    fn result(&self, exit: &Exit) -> Answered {
        Answered::now(calls::call_result(exit, self.era))
    }

    /// The answer of a refusal, `{outcome}`.
    fn refused(&self, refusal: &Outcome) -> Answered {
        Answered::now(calls::refusal_result(refusal, self.era))
    }

    /// The result of the line of a job that goes on: `members` beside `mcp_job_started`.
    fn started(&self, members: Map<String, Value>) -> Value {
        calls::call_result(
            &cli_line::success(members, &Outcome::new(McpCode::JobStarted)),
            self.era,
        )
    }
}

/// Workflow #434 LAB-5G3: the answer of a call of `tool` with `arguments` in `era`. `parse_call`
/// admits only an offered row's tool; a name the catalog does not hold is refused as unsupported.
/// The members the front adds (`tools::ADDED`) are read and taken out first; then a row this build
/// does not serve, and arguments it refuses, answer at once, with no job (`calls::Service::call`).
/// `ac_run_show` and `ac_run_start` are answered as [`run_show`] and [`run_start`] say; every other
/// served call runs as a job ([`served_as_job`]).
fn answer(
    shared: &Arc<Shared>,
    tool: &str,
    arguments: Map<String, Value>,
    era: Era,
    call: &Call,
) -> Answered {
    let Some(operation) = shared.catalog.operation(tool) else {
        return Answered::now(calls::refusal_result(&tools::unsupported(tool), era));
    };
    let asked = Asked {
        shared,
        operation,
        tool,
        era,
        call,
    };
    let (members, arguments) = match calls::front_members(operation, arguments, shared.call_budget)
    {
        Ok(read) => read,
        Err(refused) => {
            return asked.result(&cli_line::failure(&refused.outcome, refused.text));
        }
    };
    if shared.service.refuses(operation, &arguments) {
        return Answered::now(shared.service.call(operation, tool, &arguments, era));
    }
    match operation.name.as_str() {
        RUN_SHOW => run_show(&asked, &arguments, members.wait),
        RUN_START => run_start(&asked, &arguments),
        _ => served_as_job(&asked, arguments, members),
    }
}

/// Workflow #434 LAB-5G3 (work order points 1, 4 and 6): a served call as a job of its own, its
/// line `calls::Service::line`'s (or, for a cursor given back, `calls::Service::taken_back`'s). The
/// call waits for it up to the call budget, `ac_run_stop` up to its `wait_s`, then answers
/// [`job_answer`]. `ac_run_stop` of a run whose job of this process still submits it only asks it
/// to stop (`cancel_only`), as today's `ac_stop_run` does: that job's own client resets after the
/// cancelled run.
fn served_as_job(
    asked: &Asked<'_>,
    mut arguments: Map<String, Value>,
    members: FrontMembers,
) -> Answered {
    let shared = asked.shared;
    if asked.operation.name == RUN_STOP
        && arguments
            .get(ID)
            .and_then(Value::as_str)
            .and_then(|id| shared.jobs.find(id))
            .is_some_and(|job| job.run && !job.finished())
    {
        arguments.insert(CANCEL_ONLY.to_owned(), Value::Bool(true));
    }
    let until = match members.wait {
        Some(wait) => (asked.call.started + wait).min(asked.last_wait()),
        None => asked.last_wait(),
    };
    let body = {
        let shared = Arc::clone(shared);
        let operation = asked.operation.clone();
        let tool = asked.tool.to_owned();
        let era = asked.era;
        move || {
            let served = panic::catch_unwind(AssertUnwindSafe(|| match &members.cursor {
                Some(cursor) => shared
                    .service
                    .taken_back(&operation, &arguments, cursor, era),
                None => shared.service.line(&operation, &tool, &arguments, era),
            }));
            calls::finished(
                &served.unwrap_or_else(|_| cli_line::failure(&tool_panicked(&tool), None)),
            )
        }
    };
    let job = match shared
        .jobs
        .start(shared.jobs.handle(), asked.tool, false, body)
    {
        Ok(job) => job,
        Err(refusal) => return asked.refused(&refusal),
    };
    job.wait_until(until, &asked.call.cancelled);
    job_answer(asked, &job, false)
}

/// Workflow #434 LAB-5G3: the answer about `job`. While it runs: `{handle, job_phase}` with
/// `mcp_job_started`, its handle issued once the answer is delivered. Once it ended: its line, as
/// its call would have been answered, which is then read. Work order point 10: a line that fits
/// the limit neither way is never cut; the answer of its own call (`read` false) is
/// `output_budget_exceeded` with the job's handle, and the job is kept unread, so that it counts
/// for the MCP entry until `ac_run_show` of the handle (`read` true) answers the job's summary
/// beside the same refusal.
fn job_answer(asked: &Asked<'_>, job: &jobs::Job, read: bool) -> Answered {
    let Some(ended) = job.ended() else {
        let mut members = Map::new();
        members.insert(HANDLE.to_owned(), json!(job.handle));
        members.insert(JOB_PHASE.to_owned(), json!(job.phase()));
        return Answered {
            result: asked.started(members),
            handle: Some(job.handle.clone()),
        };
    };
    match calls::sent(&ended, asked.era) {
        Ok(result) => {
            job.collect();
            Answered::now(result)
        }
        Err(sent_bytes) if read => {
            job.collect();
            Answered::now(over_budget(asked, job, sent_bytes, Some(&ended.line)))
        }
        Err(sent_bytes) => Answered {
            result: over_budget(asked, job, sent_bytes, None),
            handle: Some(job.handle.clone()),
        },
    }
}

/// Workflow #434 LAB-5G3 (work order point 10): `output_budget_exceeded` (the bytes the line takes
/// as sent, the limit) beside the job's `handle` and, read with `ac_run_show` (`summarised`, the
/// job's line), its `job_phase` and `summary` (`budget::job_summary`).
fn over_budget(
    asked: &Asked<'_>,
    job: &jobs::Job,
    sent_bytes: usize,
    summarised: Option<&Value>,
) -> Value {
    let mut members = Map::new();
    members.insert(HANDLE.to_owned(), json!(job.handle));
    if let Some(line) = summarised {
        members.insert(JOB_PHASE.to_owned(), json!(job.phase()));
        members.insert(SUMMARY.to_owned(), budget::job_summary(line));
    }
    calls::call_result(
        &cli_line::receipt(members, &budget::exceeded(sent_bytes), true),
        asked.era,
    )
}

/// Workflow #434 LAB-5G3 (work order point 3; ruling 10 of 6775192250): `ac_run_show`. Its `id`
/// is a job handle of this process or a run. A job handle is waited for up to `wait`, within the
/// budget, and answered [`job_answer`]; one this process does not hold (another process's, or one
/// dropped past `--kept-jobs`) is `handle_unknown`. A run is today's `ac_get_run`: read, and read
/// again every second while it is not found, admitted or running, for at most `wait` (and the
/// budget less the room for the last read); a run whose job of this process ended before the run
/// was found is answered at once. A run this process's job submits carries that job as `job`
/// (`kind`, `job_phase` and, once it ended, its `line`).
fn run_show(asked: &Asked<'_>, arguments: &Map<String, Value>, wait: Option<Duration>) -> Answered {
    let shared = asked.shared;
    let wait_until = (Instant::now() + wait.unwrap_or_default()).min(asked.last_read());
    let id = arguments
        .get(ID)
        .and_then(Value::as_str)
        .unwrap_or_default();
    if id.starts_with(jobs::JOB_PREFIX) {
        let Some(job) = shared.jobs.find(id) else {
            return asked.refused(&Outcome::new(ActingctlCode::HandleUnknown));
        };
        job.wait_until(wait_until, &asked.call.cancelled);
        return job_answer(asked, &job, true);
    }
    let job = shared.jobs.find(id).filter(|job| job.run);
    let exit = shared.service.watched(
        asked.operation,
        asked.tool,
        arguments,
        asked.era,
        &mut |exit: &Exit| {
            let state = run_state(exit);
            let may_change = matches!(
                state,
                Some(
                    ContainedRunState::NotFound
                        | ContainedRunState::Admitted
                        | ContainedRunState::Running
                )
            );
            let failed_first = state == Some(ContainedRunState::NotFound)
                && job.as_ref().is_some_and(|job| job.finished());
            if !may_change
                || failed_first
                || asked.call.cancelled.load(Ordering::SeqCst)
                || Instant::now() + RUN_POLL >= wait_until
            {
                return false;
            }
            thread::sleep(RUN_POLL);
            true
        },
    );
    asked.result(&match &job {
        Some(job) => with_job(exit, job),
        None => exit,
    })
}

/// The state a run's line says.
fn run_state(exit: &Exit) -> Option<ContainedRunState> {
    let state = exit.line.as_ref().ok()?.get(STATE)?;
    serde_json::from_value(state.clone()).ok()
}

/// A run's line with the job of this process that submits it, as `job`.
fn with_job(mut exit: Exit, job: &jobs::Job) -> Exit {
    if let Ok(Value::Object(line)) = &mut exit.line {
        let mut member = Map::new();
        member.insert(KIND.to_owned(), json!(job.kind));
        member.insert(JOB_PHASE.to_owned(), json!(job.phase()));
        if let Some(ended) = job.ended() {
            member.insert(LINE.to_owned(), ended.line);
        }
        line.insert(JOB.to_owned(), Value::Object(member));
    }
    exit
}

/// Workflow #434 LAB-5G3 (work order point 5; today's `ac_run_pack`): `ac_run_start`. The jobs'
/// cap first, then the run prepared after today's provenance (`calls::Service::run_start`), then
/// its submission in a job, a run's, whose handle is the run's Runtime request id. Without the
/// row's `wait` the call answers at once `{handle, correlation_id, job_phase}` with the
/// provenance's warnings; with it, it waits for the run's end as long, within the budget, and
/// answers its line when it ended.
fn run_start(asked: &Asked<'_>, arguments: &Map<String, Value>) -> Answered {
    let shared = asked.shared;
    if let Err(refusal) = shared.jobs.has_capacity() {
        return asked.refused(&refusal);
    }
    let started = match shared
        .service
        .run_start(asked.operation, asked.tool, arguments)
    {
        Ok(started) => started,
        Err(exit) => return asked.result(&exit),
    };
    let handle = match started.request_id() {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let mut members = Map::new();
    members.insert(HANDLE.to_owned(), json!(handle));
    members.insert(CORRELATION_ID.to_owned(), started.correlation_id());
    if !started.warnings.is_empty() {
        members.insert(WARNINGS.to_owned(), Value::Array(started.warnings.clone()));
    }
    let tool = asked.tool.to_owned();
    let body = move || {
        let submitted = panic::catch_unwind(AssertUnwindSafe(|| started.submitted()));
        calls::finished(
            &submitted.unwrap_or_else(|_| cli_line::failure(&tool_panicked(&tool), None)),
        )
    };
    let job = match shared.jobs.start(handle, asked.tool, true, body) {
        Ok(job) => job,
        Err(refusal) => return asked.refused(&refusal),
    };
    let wait = arguments
        .get(WAIT)
        .and_then(Value::as_str)
        .and_then(core_ops::parse_duration);
    if let Some(wait) = wait
        && job.wait_until(
            (Instant::now() + wait).min(asked.last_wait()),
            &asked.call.cancelled,
        )
    {
        return job_answer(asked, &job, false);
    }
    members.insert(JOB_PHASE.to_owned(), json!(job.phase()));
    Answered {
        result: asked.started(members),
        handle: Some(job.handle.clone()),
    }
}

/// A panic caught at the tool call boundary: the call fails with `mcp_tool_panicked`
/// (`mcp_tool`) and the server goes on. The default panic hook has already written the panic to
/// stderr.
fn tool_panicked(tool: &str) -> Outcome {
    Outcome::new(ActingctlCode::McpToolPanicked).mcp_tool(tool)
}

/// From 5 s on, a call whose request carried a progressToken gets a progress notification
/// at most every 5 s until its answer. It carries no `message`: that optional MCP member is
/// text, and the server writes none (Workflow #378 A5).
fn progress_loop(shared: &Shared, stop: &Receiver<()>) {
    while let Err(RecvTimeoutError::Timeout) = stop.recv_timeout(PROGRESS_TICK) {
        let watched = lock(&shared.calls)
            .values()
            .filter(|call| call.progress_token.is_some())
            .cloned()
            .collect::<Vec<_>>();
        let now = Instant::now();
        for call in watched {
            let mut phase = lock(&call.phase);
            let Some(token) = &call.progress_token else {
                continue;
            };
            let next = phase.progress_sent + 1;
            let due = PROGRESS_INTERVAL
                .checked_mul(next)
                .and_then(|after| call.started.checked_add(after));
            if phase.finished
                || call.cancelled.load(Ordering::SeqCst)
                || due.is_none_or(|due| now < due)
            {
                continue;
            }
            phase.progress_sent = next;
            shared.emit(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {"progressToken": token, "progress": next},
            }));
        }
    }
}
