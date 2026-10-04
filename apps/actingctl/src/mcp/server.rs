// SPDX-License-Identifier: AGPL-3.0-only

//! The stdio server (#338 §四 线程 / 启动预算 / 关闭 / 双代协议). One thread reads stdin
//! (lines up to 4 MiB), the calling thread dispatches, at most four workers run
//! `tools/call`, one thread writes stdout in order and one sends progress. stdout carries
//! protocol messages only; diagnostics go to stderr. Startup reads no file and opens no
//! connection, so `initialize` and `server/discover` answer at once. At stdin EOF the
//! server takes no new request, gives answers in flight up to 2 s and exits with 0.

use super::lock;
use super::protocol::{
    self, Era, INVALID_PARAMS, INVALID_REQUEST, Incoming, LegacyVersion, METHOD_NOT_FOUND,
    MetaVersion, PARSE_ERROR,
};
use super::runtime::RuntimeAccess;
use super::tools::{self, TierSet, ToolContext, ToolDef, ToolError};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const WORKERS: usize = 4;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const PROGRESS_TICK: Duration = Duration::from_millis(500);
/// How long answers in flight may still be written after stdin ends.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const SHUTDOWN_POLL: Duration = Duration::from_millis(20);

pub(super) struct ServerConfig {
    pub(super) root: Option<PathBuf>,
    pub(super) state_root: Option<PathBuf>,
    pub(super) tiers: TierSet,
}

struct Shared {
    tiers: TierSet,
    runtime: RuntimeAccess,
    /// This process's identity in the cursors it issues.
    session: u64,
    /// The version a legacy `initialize` negotiated; `None` until one arrives.
    legacy: Mutex<Option<LegacyVersion>>,
    /// `tools/call` requests in flight, by their JSON-RPC id.
    calls: Mutex<HashMap<String, Arc<Call>>>,
    out: Sender<Outgoing>,
    output_failed: Arc<AtomicBool>,
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
    tool: &'static ToolDef,
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
    Failed(String),
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

pub(super) fn serve(config: ServerConfig) -> Result<ExitCode, String> {
    let output_failed = Arc::new(AtomicBool::new(false));
    let (out_sender, out_receiver) = mpsc::channel::<Outgoing>();
    let writer_failed = Arc::clone(&output_failed);
    let writer = spawn("mcp-stdout", move || {
        write_loop(&out_receiver, &writer_failed)
    })?;
    let shared = Arc::new(Shared {
        tiers: config.tiers,
        runtime: RuntimeAccess::new(config.root, config.state_root),
        session: session_identity(),
        legacy: Mutex::new(None),
        calls: Mutex::new(HashMap::new()),
        out: out_sender,
        output_failed,
    });
    let (job_sender, job_receiver) = mpsc::channel::<Job>();
    let job_receiver = Arc::new(Mutex::new(job_receiver));
    // Workers, the progress sender and the stdin reader run detached; the process ends
    // with this function.
    for index in 0..WORKERS {
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
            Inbound::TooLong => shared.emit(&protocol::error_response(
                &Value::Null,
                PARSE_ERROR,
                "message exceeds 4 MiB",
                None,
            )),
            Inbound::End => break,
            Inbound::Failed(error) => {
                input_error = Some(error);
                break;
            }
        }
    }

    // Shutdown: no new calls; answers in flight get up to 2 s; background waits are dropped.
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
        return Err(format!("stdin read failed: {error}"));
    }
    if shared.output_failed.load(Ordering::SeqCst) {
        return Err("stdout write failed; the client no longer reads".to_owned());
    }
    Ok(ExitCode::SUCCESS)
}

fn spawn(
    name: &str,
    body: impl FnOnce() + Send + 'static,
) -> Result<thread::JoinHandle<()>, String> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map_err(|error| format!("cannot start thread {name}: {error}"))
}

/// Distinct per process, so a cursor from an earlier server answers `cursor_invalid`.
fn session_identity() -> u64 {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    RandomState::new().hash_one((std::process::id(), started))
}

fn write_loop(messages: &Receiver<Outgoing>, failed: &AtomicBool) {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for message in messages {
        let Outgoing::Line(line) = message else {
            return;
        };
        if let Err(error) = write_line(&mut output, &line) {
            eprintln!("actingctl mcp-serve: stdout write failed: {error}");
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
        let inbound =
            read_line(&mut input).unwrap_or_else(|error| Inbound::Failed(error.to_string()));
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
        Err(_) => shared.emit(&protocol::error_response(
            &Value::Null,
            PARSE_ERROR,
            "Parse error",
            None,
        )),
        Ok(Value::Array(items)) => handle_batch(shared, jobs, items),
        Ok(message) => handle_message(shared, jobs, message, &Sink::Direct),
    }
}

/// 2025-03-26 takes JSON-RPC batches and answers them with one array; every later version
/// and the modern era refuse a batch array with -32600.
fn handle_batch(shared: &Shared, jobs: &Sender<Job>, items: Vec<Value>) {
    let batching = *lock(&shared.legacy) == Some(LegacyVersion::V20250326);
    if items.is_empty() || !batching {
        let message = if items.is_empty() {
            "empty batch"
        } else {
            "batch arrays are accepted only after initialize with protocol 2025-03-26"
        };
        shared.emit(&protocol::error_response(
            &Value::Null,
            INVALID_REQUEST,
            message,
            None,
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
            Some(protocol::error_response(
                &id,
                INVALID_REQUEST,
                "Invalid Request",
                None,
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
    let era = match protocol::meta_version(params.as_ref()) {
        MetaVersion::Modern => Era::Modern,
        MetaVersion::Unsupported(requested) => {
            return sink.respond(shared, Some(protocol::unsupported_version(&id, &requested)));
        }
        MetaVersion::Malformed(reason) => {
            return sink.respond(
                shared,
                Some(protocol::error_response(&id, INVALID_PARAMS, reason, None)),
            );
        }
        // A legacy ping is answered with or without a session.
        MetaVersion::Absent if method == "ping" => {
            return sink.respond(shared, Some(protocol::response(&id, json!({}))));
        }
        MetaVersion::Absent => {
            let legacy = *lock(&shared.legacy);
            match legacy {
                Some(version) if method != "server/discover" => Era::Legacy(version),
                _ => {
                    return sink.respond(
                        shared,
                        Some(protocol::error_response(
                            &id,
                            INVALID_PARAMS,
                            protocol::DUAL_ERA_MESSAGE,
                            None,
                        )),
                    );
                }
            }
        }
    };
    match method {
        "server/discover" => sink.respond(shared, Some(protocol::discover(&id))),
        "tools/list" => sink.respond(shared, Some(list_tools(shared, &id, era, params.as_ref()))),
        "tools/call" => call_tool(shared, jobs, id, era, params, sink),
        _ => sink.respond(
            shared,
            Some(protocol::error_response(
                &id,
                METHOD_NOT_FOUND,
                "Method not found",
                None,
            )),
        ),
    }
}

fn initialize(shared: &Shared, id: &Value, params: Option<&Value>) -> Value {
    let Some(requested) = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
    else {
        return protocol::error_response(
            id,
            INVALID_PARAMS,
            "initialize requires params.protocolVersion",
            None,
        );
    };
    let version = LegacyVersion::negotiate(requested);
    *lock(&shared.legacy) = Some(version);
    protocol::response(id, protocol::initialize_result(version))
}

fn list_tools(shared: &Shared, id: &Value, era: Era, params: Option<&Value>) -> Value {
    if params
        .and_then(Value::as_object)
        .and_then(|params| params.get("cursor"))
        .is_some_and(|cursor| !cursor.is_null())
    {
        return protocol::error_response(
            id,
            INVALID_PARAMS,
            "tools/list has one page; this server issued no cursor",
            None,
        );
    }
    let listed = tools::TOOLS
        .iter()
        .filter(|tool| shared.tiers.contains(tool.tier))
        .map(|tool| tools::definition(tool, era))
        .collect::<Vec<_>>();
    let mut result = json!({"tools": listed});
    if era == Era::Modern {
        // 2026-07-28 `ListToolsResult extends CacheableResult` (SEP-2549): both fields required.
        result["ttlMs"] = json!(0);
        result["cacheScope"] = json!("private");
    }
    protocol::response(id, protocol::complete(era, result))
}

/// What a `CallToolRequest` names.
struct CallRequest {
    tool: &'static ToolDef,
    arguments: Map<String, Value>,
    progress_token: Option<Value>,
}

fn parse_call(params: Option<Value>) -> Result<CallRequest, String> {
    let Some(Value::Object(mut params)) = params else {
        return Err("tools/call requires params {name, arguments}".to_owned());
    };
    let name = match params.get("name") {
        Some(Value::String(name)) => name.clone(),
        _ => return Err("tools/call params.name must be a tool name".to_owned()),
    };
    let tool = tools::find(&name).ok_or_else(|| format!("unknown tool: {name}"))?;
    let arguments = match params.remove("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(arguments)) => arguments,
        Some(_) => return Err("tools/call params.arguments must be an object".to_owned()),
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
    } = match parse_call(params) {
        Ok(request) => request,
        Err(message) => {
            return sink.respond(
                shared,
                Some(protocol::error_response(
                    &id,
                    INVALID_PARAMS,
                    &message,
                    None,
                )),
            );
        }
    };
    if !shared.tiers.contains(tool.tier) {
        let result = tools::call_result(Err(tools::tier_not_enabled(tool)), era);
        return sink.respond(
            shared,
            Some(protocol::response(&id, protocol::complete(era, result))),
        );
    }
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
        return sink.respond(
            shared,
            Some(protocol::error_response(
                &id,
                INVALID_REQUEST,
                "a request with this id is still in flight",
                None,
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
        let failure = ToolError::new(
            "runtime",
            "mcp_workers_unavailable",
            "no tools/call worker is running; see stderr",
        );
        let result = tools::call_result(Err(failure), job.era);
        job.sink.respond(
            shared,
            Some(protocol::response(
                &job.id,
                protocol::complete(job.era, result),
            )),
        );
    }
}

fn worker_loop(shared: &Shared, jobs: &Mutex<Receiver<Job>>) {
    loop {
        let next = lock(jobs).recv();
        let Ok(job) = next else {
            return;
        };
        run_job(shared, job);
    }
}

fn run_job(shared: &Shared, job: Job) {
    let Job {
        id,
        key,
        era,
        tool,
        arguments,
        call,
        sink,
    } = job;
    let context = ToolContext {
        runtime: &shared.runtime,
        cancelled: &call.cancelled,
        deadline: call.started + tools::CALL_BUDGET,
        session: shared.session,
    };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| (tool.run)(&context, &arguments)))
        .unwrap_or_else(|_| {
            Err(ToolError::new(
                "runtime",
                "mcp_tool_panicked",
                format!("{} stopped on an internal panic; see stderr", tool.name),
            ))
        });
    let response = protocol::response(
        &id,
        protocol::complete(era, tools::call_result(outcome, era)),
    );
    let mut phase = lock(&call.phase);
    phase.finished = true;
    lock(&shared.calls).remove(&key);
    // A cancelled request gets no answer.
    let response = (!call.cancelled.load(Ordering::SeqCst)).then_some(response);
    sink.respond(shared, response);
}

/// From 5 s on, a call whose request carried a progressToken gets a progress notification
/// at most every 5 s until its answer.
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
            let elapsed = call.started.elapsed().as_secs();
            shared.emit(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {
                    "progressToken": token,
                    "progress": next,
                    "message": format!("still running after {elapsed} s"),
                },
            }));
        }
    }
}
