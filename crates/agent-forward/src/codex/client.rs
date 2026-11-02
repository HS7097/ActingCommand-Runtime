// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/desktop.rs and scripts/runtime/src/relay.rs. Copyright (c) 2026 HS7097. MIT: see ../../LICENSE-codex-goal-lifecycle.txt.

//! One send to a Codex task through the desktop's local IPC (Workflow #432;
//! `contracts/agent-forward.md`): the length-prefixed frames, the desktop's requests, and the
//! readiness order before the one submission.
//!
//! No credential is involved. A `detail` here never names the task's id: an id in the desktop's own
//! text is replaced before it is quoted.

use super::request::{self, MessageRequest, Submission, Submitter};
use super::state::{self, StateProblem, TaskState};
use crate::pipe::{OpenFailure, PipeClient, ReadFailure, WriteFailure};
use crate::{
    DeferReason, DeliveryProblem, ForwardMessage, ForwardOutcome, IntentFailed, SendLimits,
};
use serde_json::{Value, json};
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::ERROR_PATH_NOT_FOUND;

/// The desktop's IPC pipe. It is fixed: a channel needs only the task's id.
const PIPE_NAME: &str = r"\\.\pipe\codex-ipc";
/// The client type this transport says it is.
const CLIENT_TYPE: &str = "actingcommand-runtime";
/// The largest frame written.
const MAX_OUTGOING_FRAME_BYTES: usize = 64 * 1024 * 1024;
/// The largest frame read. A snapshot holds the task's history; the desktop takes frames up to this.
const MAX_INCOMING_FRAME_BYTES: usize = 256 * 1024 * 1024;
/// The largest message the desktop takes, in bytes.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// The longest `initialize` may take, whatever the send's deadline.
const MAX_INITIALIZE: Duration = Duration::from_secs(5);
/// The one state stream version this build reads.
const STATE_STREAM_VERSION: u64 = 11;
/// The longest piece of the desktop's own text a `detail` quotes, in characters.
const MAX_QUOTED_CHARS: usize = 512;

/// Why bytes did not go or come.
#[derive(Debug)]
pub(super) enum LinkFailure {
    TimedOut,
    Stopped,
    /// The desktop closed its end.
    Closed,
    Failed(io::Error),
}

/// The bytes of one connection to the desktop: its pipe, or a test's script.
pub(super) trait ByteLink {
    fn send_bytes(
        &mut self,
        bytes: &[u8],
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), LinkFailure>;
    fn receive_bytes(
        &mut self,
        buffer: &mut [u8],
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), LinkFailure>;
}

impl ByteLink for PipeClient {
    fn send_bytes(
        &mut self,
        bytes: &[u8],
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), LinkFailure> {
        self.write_all(bytes, deadline, stopped)
            .map_err(|failure| match failure {
                WriteFailure::TimedOut => LinkFailure::TimedOut,
                WriteFailure::Stopped => LinkFailure::Stopped,
                WriteFailure::Failed(error) => LinkFailure::Failed(error),
            })
    }

    fn receive_bytes(
        &mut self,
        buffer: &mut [u8],
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), LinkFailure> {
        self.read_exact(buffer, deadline, stopped)
            .map_err(|failure| match failure {
                ReadFailure::TimedOut => LinkFailure::TimedOut,
                ReadFailure::Stopped => LinkFailure::Stopped,
                ReadFailure::Closed => LinkFailure::Closed,
                ReadFailure::Failed(error) => LinkFailure::Failed(error),
            })
    }
}

/// Why the desktop could not be reached.
#[derive(Debug)]
pub(super) enum ConnectFailure {
    /// The pipe is absent or busy (errors 2, 3 and 231): the desktop is not running.
    Unavailable,
    Failed(io::Error),
}

/// Opens one connection to the desktop.
pub(super) trait Connector: Send + Sync {
    fn connect(&self) -> Result<Box<dyn ByteLink>, ConnectFailure>;
}

/// The pipe's open failure, with error 3 as absent too.
fn open_failure(failure: OpenFailure) -> ConnectFailure {
    match failure {
        OpenFailure::Unavailable => ConnectFailure::Unavailable,
        OpenFailure::Failed(error) if error.raw_os_error() == Some(ERROR_PATH_NOT_FOUND as i32) => {
            ConnectFailure::Unavailable
        }
        OpenFailure::Failed(error) => ConnectFailure::Failed(error),
    }
}

/// The desktop's own pipe.
struct PipeConnector;

impl Connector for PipeConnector {
    fn connect(&self) -> Result<Box<dyn ByteLink>, ConnectFailure> {
        match PipeClient::open(PIPE_NAME) {
            Ok(client) => Ok(Box::new(client)),
            Err(failure) => Err(open_failure(failure)),
        }
    }
}

/// Why a step with the desktop did not give its answer.
#[derive(Debug)]
pub(super) enum Failure {
    /// The deadline passed.
    TimedOut,
    /// The Runtime is stopping.
    Stopped,
    /// The pipe closed or failed.
    Broken(String),
    /// A frame or an answer of a shape this build does not know.
    Protocol(String),
    /// The desktop answered with an error.
    Refused(String),
    /// No desktop client has the task loaded.
    NotLoaded,
    /// The task's owner is still loading it.
    Loading(&'static str),
}

impl Failure {
    fn from_link(failure: LinkFailure) -> Self {
        match failure {
            LinkFailure::TimedOut => Self::TimedOut,
            LinkFailure::Stopped => Self::Stopped,
            LinkFailure::Closed => Self::Broken("the desktop closed the connection".to_owned()),
            LinkFailure::Failed(error) => Self::Broken(error.to_string()),
        }
    }

    /// Text for a person's `detail`.
    pub(super) fn text(&self) -> String {
        match self {
            Self::TimedOut => "the send's deadline passed".to_owned(),
            Self::Stopped => "the Runtime is stopping".to_owned(),
            Self::Broken(text) => format!("the desktop's pipe failed: {text}"),
            Self::Protocol(text) => {
                format!("the desktop answered in a shape this build does not know: {text}")
            }
            Self::Refused(text) => format!("the desktop refused: {text}"),
            Self::NotLoaded => "no desktop client has the task loaded".to_owned(),
            Self::Loading(stage) => format!("the task's owner is still loading it ({stage})"),
        }
    }

    /// The outcome of a failure before the intent: nothing was submitted.
    fn before_intent(self) -> ForwardOutcome {
        let detail = self.text();
        match self {
            Self::TimedOut | Self::Stopped => ForwardOutcome::Deferred {
                reason: DeferReason::TargetUnresponsive,
                detail,
            },
            Self::Broken(_) => ForwardOutcome::Deferred {
                reason: DeferReason::IpcUnavailable,
                detail,
            },
            Self::NotLoaded | Self::Loading(_) => ForwardOutcome::Deferred {
                reason: DeferReason::TargetNotLoaded,
                detail,
            },
            Self::Protocol(_) => ForwardOutcome::Failed {
                problem: DeliveryProblem::ProtocolChanged,
                detail,
            },
            Self::Refused(_) => ForwardOutcome::Failed {
                problem: DeliveryProblem::Rejected,
                detail,
            },
        }
    }

    /// The problem of a failure after the intent, waiting for the submission's answer.
    pub(super) fn after_intent(&self) -> DeliveryProblem {
        match self {
            Self::TimedOut => DeliveryProblem::WriteTimedOut,
            Self::Protocol(_) => DeliveryProblem::ProtocolChanged,
            Self::Stopped
            | Self::Broken(_)
            | Self::Refused(_)
            | Self::NotLoaded
            | Self::Loading(_) => DeliveryProblem::WriteFailed,
        }
    }
}

/// The 16 bytes of a UUID in any of its text forms: hyphenated, plain, braced or `urn:uuid:`.
fn uuid_bytes(text: &str) -> Option<[u8; 16]> {
    let text = text.strip_prefix("urn:uuid:").unwrap_or(text);
    let text = match text.strip_prefix('{') {
        Some(inner) => inner.strip_suffix('}')?,
        None => text,
    };
    let digits: Vec<u8> = match text.len() {
        32 => text.bytes().collect(),
        36 => {
            let bytes = text.as_bytes();
            if [8, 13, 18, 23].iter().any(|at| bytes[*at] != b'-') {
                return None;
            }
            bytes.iter().copied().filter(|byte| *byte != b'-').collect()
        }
        _ => return None,
    };
    if digits.len() != 32 {
        return None;
    }
    let digit = |byte: u8| char::from(byte).to_digit(16);
    let mut bytes = [0_u8; 16];
    for (at, pair) in digits.chunks_exact(2).enumerate() {
        bytes[at] = u8::try_from((digit(pair[0])? << 4) | digit(pair[1])?).ok()?;
    }
    Some(bytes)
}

/// A UUID's hyphenated text in lower case.
fn uuid_text(bytes: [u8; 16]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(36);
    for (at, byte) in bytes.iter().enumerate() {
        if matches!(at, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        text.push(char::from(DIGITS[usize::from(*byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(*byte & 0x0f)]));
    }
    text
}

/// A UUID the desktop sent, in the one form this transport compares and sends.
pub(super) fn uuid_of(text: &str) -> Option<String> {
    uuid_bytes(text).map(uuid_text)
}

/// A channel's task id: a non-nil UUID, hyphenated, in lower case.
fn task_id(native_id: &str) -> Option<String> {
    if native_id.len() != 36 {
        return None;
    }
    let bytes = uuid_bytes(native_id)?;
    bytes
        .iter()
        .any(|byte| *byte != 0)
        .then(|| uuid_text(bytes))
}

/// The wake's 16 bytes as a UUID of version 8, the form the desktop takes for a message's id.
fn version_8(mut bytes: [u8; 16]) -> [u8; 16] {
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

/// A piece of the desktop's own text for a `detail`: bounded, with the task's id replaced.
pub(super) fn quoted(value: &Value, task: &str) -> String {
    let text = match value.as_str() {
        Some(text) => text.to_owned(),
        None => value.to_string(),
    };
    let text: String = text
        .replace(task, "<task>")
        .chars()
        .take(MAX_QUOTED_CHARS)
        .collect();
    if text.is_empty() {
        "no text".to_owned()
    } else {
        text
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// One connection to the desktop, for one send.
struct Desktop<'a> {
    link: Box<dyn ByteLink>,
    stopped: &'a dyn Fn() -> bool,
    /// The task, as a UUID in lower case.
    task: &'a str,
    /// This client's id, once `initialize` gave it.
    client_id: Option<String>,
    /// The base of this connection's request ids.
    seed: [u8; 16],
    requests: u32,
}

impl<'a> Desktop<'a> {
    fn new(
        link: Box<dyn ByteLink>,
        stopped: &'a dyn Fn() -> bool,
        task: &'a str,
        message_id: [u8; 16],
    ) -> Self {
        // Request ids differ between sends of one message: the message's id mixed with the time.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
            .to_be_bytes();
        let mut seed = message_id;
        for (byte, mix) in seed.iter_mut().zip(nanos) {
            *byte ^= mix;
        }
        Self {
            link,
            stopped,
            task,
            client_id: None,
            seed,
            requests: 0,
        }
    }

    fn source_client_id(&self) -> String {
        self.client_id
            .clone()
            .unwrap_or_else(|| "initializing-client".to_owned())
    }

    fn next_request_id(&mut self) -> String {
        self.requests = self.requests.wrapping_add(1);
        let mut bytes = self.seed;
        for (byte, count) in bytes[12..].iter_mut().zip(self.requests.to_be_bytes()) {
            *byte ^= count;
        }
        uuid_text(version_8(bytes))
    }

    fn write_frame(&mut self, value: &Value, deadline: Instant) -> Result<(), Failure> {
        let payload = serde_json::to_vec(value)
            .map_err(|error| Failure::Protocol(format!("a frame cannot be encoded: {error}")))?;
        let length = u32::try_from(payload.len())
            .ok()
            .filter(|length| *length > 0 && payload.len() <= MAX_OUTGOING_FRAME_BYTES)
            .ok_or_else(|| Failure::Protocol("a frame to write has no valid size".to_owned()))?;
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(&payload);
        self.link
            .send_bytes(&frame, deadline, self.stopped)
            .map_err(Failure::from_link)
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<Value, Failure> {
        if Instant::now() >= deadline {
            return Err(Failure::TimedOut);
        }
        let mut header = [0_u8; 4];
        self.link
            .receive_bytes(&mut header, deadline, self.stopped)
            .map_err(Failure::from_link)?;
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > MAX_INCOMING_FRAME_BYTES {
            return Err(Failure::Protocol(format!(
                "the desktop sent a frame of {length} bytes"
            )));
        }
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(length)
            .map_err(|_| Failure::Protocol(format!("a frame of {length} bytes cannot be held")))?;
        payload.resize(length, 0_u8);
        self.link
            .receive_bytes(&mut payload, deadline, self.stopped)
            .map_err(Failure::from_link)?;
        serde_json::from_slice(&payload)
            .map_err(|_| Failure::Protocol("the desktop sent a frame that is not JSON".to_owned()))
    }

    /// Answers the desktop's question whether this client can handle a request: never. Answers
    /// whether `frame` was such a question.
    fn answer_discovery(&mut self, frame: &Value, deadline: Instant) -> Result<bool, Failure> {
        if frame["type"] != "client-discovery-request" {
            return Ok(false);
        }
        let Some(id) = frame["requestId"].as_str() else {
            return Err(Failure::Protocol(
                "a client-discovery-request has no request id".to_owned(),
            ));
        };
        let answer = json!({
            "type": "client-discovery-response",
            "requestId": id,
            "response": {"canHandle": false}
        });
        self.write_frame(&answer, deadline)?;
        Ok(true)
    }

    /// One request and its answer. Other frames on the way are read and passed over.
    fn request_to(
        &mut self,
        method: &str,
        version: u64,
        params: Value,
        deadline: Instant,
        target: Option<&str>,
    ) -> Result<Value, Failure> {
        let request_id = self.next_request_id();
        let timeout_ms = u64::try_from(
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let mut request = json!({
            "type": "request",
            "requestId": request_id,
            "sourceClientId": self.source_client_id(),
            "version": version,
            "method": method,
            "params": params,
            "timeoutMs": timeout_ms
        });
        if let Some(target) = target {
            request["targetClientId"] = json!(target);
        }
        self.write_frame(&request, deadline)?;
        loop {
            let frame = self.read_frame(deadline)?;
            if self.answer_discovery(&frame, deadline)? {
                continue;
            }
            if frame["type"] == "response"
                && frame["requestId"].as_str() == Some(request_id.as_str())
            {
                return Ok(frame);
            }
        }
    }

    /// Whether an answer says success; an error answer is a refusal, quoted.
    fn ensure_success(&self, response: &Value) -> Result<(), Failure> {
        match response["resultType"].as_str() {
            Some("success") => Ok(()),
            Some("error") => Err(Failure::Refused(quoted(&response["error"], self.task))),
            _ => Err(Failure::Protocol(
                "an answer that is neither success nor error".to_owned(),
            )),
        }
    }

    /// Says this client's type and takes its id.
    fn initialize(&mut self, deadline: Instant) -> Result<(), Failure> {
        let deadline = deadline.min(Instant::now() + MAX_INITIALIZE);
        let response = self.request_to(
            "initialize",
            0,
            json!({"clientType": CLIENT_TYPE}),
            deadline,
            None,
        )?;
        self.ensure_success(&response)?;
        let id = response["result"]["clientId"]
            .as_str()
            .and_then(uuid_of)
            .ok_or_else(|| Failure::Protocol("initialize gave no client id".to_owned()))?;
        self.client_id = Some(id);
        Ok(())
    }

    /// The desktop client that owns the task.
    fn owner(&mut self, deadline: Instant) -> Result<String, Failure> {
        let task = self.task;
        let response = self.request_to(
            "thread-owner-discovery",
            1,
            json!({"hostId": "local", "conversationId": task}),
            deadline,
            None,
        )?;
        if response["resultType"] == "error" && response["error"] == "no-client-found" {
            return Err(Failure::NotLoaded);
        }
        self.ensure_success(&response)?;
        response["handledByClientId"]
            .as_str()
            .and_then(uuid_of)
            .ok_or_else(|| Failure::Protocol("owner discovery gave no client id".to_owned()))
    }

    fn follow(&mut self, following: bool, deadline: Instant) -> Result<(), Failure> {
        let broadcast = json!({
            "type": "broadcast",
            "method": "thread-stream-following-changed",
            "version": 1,
            "sourceClientId": self.source_client_id(),
            "params": {"hostId": "local", "conversationId": self.task, "following": following}
        });
        self.write_frame(&broadcast, deadline)
    }

    /// The task's state: the owner, then one subscription until its snapshot, then its end.
    fn read_state(&mut self, deadline: Instant) -> Result<TaskState, Failure> {
        let owner = self.owner(deadline)?;
        self.follow(true, deadline)?;
        let state = self.read_snapshot(&owner, deadline);
        let ended = self.follow(false, deadline);
        let state = state?;
        ended?;
        Ok(state)
    }

    fn read_snapshot(&mut self, owner: &str, deadline: Instant) -> Result<TaskState, Failure> {
        let own = self.client_id.clone().unwrap_or_default();
        loop {
            let frame = self.read_frame(deadline)?;
            if self.answer_discovery(&frame, deadline)? {
                continue;
            }
            if frame["type"] != "broadcast"
                || frame["sourceClientId"]
                    .as_str()
                    .and_then(uuid_of)
                    .as_deref()
                    != Some(owner)
            {
                continue;
            }
            if let Some(targets) = frame["targetClientIds"].as_array()
                && !targets.iter().any(|id| id.as_str() == Some(own.as_str()))
            {
                continue;
            }
            let params = &frame["params"];
            if params["hostId"] != "local" || params["conversationId"].as_str() != Some(self.task) {
                continue;
            }
            if frame["method"] != "thread-stream-state-changed" {
                continue;
            }
            if frame["version"] != STATE_STREAM_VERSION {
                return Err(Failure::Protocol(
                    "the task's state stream has another version".to_owned(),
                ));
            }
            if params["change"]["type"] == "snapshot" {
                return state::parse(&params["change"]["conversationState"], owner).map_err(
                    |problem| match problem {
                        StateProblem::Loading(stage) => Failure::Loading(stage),
                        StateProblem::Shape(text) => Failure::Protocol(text.to_owned()),
                    },
                );
            }
            // A change that is no snapshot: ask again, as following does.
            self.follow(true, deadline)?;
        }
    }
}

impl Submitter for Desktop<'_> {
    fn task_state(&mut self, task: &str, deadline: Instant) -> Result<TaskState, Failure> {
        if task != self.task {
            return Err(Failure::Protocol(
                "a state read for another task".to_owned(),
            ));
        }
        self.read_state(deadline)
    }

    fn submit(
        &mut self,
        request: &MessageRequest,
        owner: &str,
        deadline: Instant,
    ) -> Result<Value, Failure> {
        let (method, version) = request.method();
        self.request_to(
            method,
            version,
            request.params.clone(),
            deadline,
            Some(owner),
        )
    }
}

/// Sends to Codex tasks through the desktop's IPC.
pub(crate) struct CodexForward {
    connector: Box<dyn Connector>,
}

fn out_of_time(limits: &SendLimits<'_>) -> bool {
    (limits.stopped)() || Instant::now() >= limits.deadline
}

impl CodexForward {
    pub(crate) fn new() -> Self {
        Self {
            connector: Box::new(PipeConnector),
        }
    }

    /// One message to the task `native_id`. The preflight submits nothing, `before_write` commits
    /// the intent, and then there is one submission, with one start after a steer the owner refused
    /// because its turn had just ended. `Accepted` proves the owner returned a turn id, not that the
    /// turn finished.
    pub(crate) fn deliver(
        &self,
        native_id: &str,
        message: &ForwardMessage,
        limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        let Some(task) = task_id(native_id) else {
            return ForwardOutcome::Failed {
                problem: DeliveryProblem::ChannelUnknown,
                detail: "the channel's native id is not a task's UUID".to_owned(),
            };
        };
        if out_of_time(limits) {
            return ForwardOutcome::Deferred {
                reason: DeferReason::TargetUnresponsive,
                detail: "the send was stopped or out of time before it began".to_owned(),
            };
        }
        if message.text.trim().is_empty() || message.text.len() > MAX_MESSAGE_BYTES {
            return ForwardOutcome::Failed {
                problem: DeliveryProblem::WriteFailed,
                detail: "the message is empty or longer than the desktop takes".to_owned(),
            };
        }
        let link = match self.connector.connect() {
            Ok(link) => link,
            Err(ConnectFailure::Unavailable) => {
                return ForwardOutcome::Deferred {
                    reason: DeferReason::IpcUnavailable,
                    detail: "the desktop's IPC pipe is not open: the desktop is not running"
                        .to_owned(),
                };
            }
            Err(ConnectFailure::Failed(error)) => {
                return ForwardOutcome::Failed {
                    problem: DeliveryProblem::WriteFailed,
                    detail: format!("the desktop's IPC pipe cannot be opened: {error}"),
                };
            }
        };
        let mut desktop = Desktop::new(link, limits.stopped, &task, message.id);
        if let Err(failure) = desktop.initialize(limits.deadline) {
            return failure.before_intent();
        }
        let current = match desktop.read_state(limits.deadline) {
            Ok(current) => current,
            Err(failure) => return failure.before_intent(),
        };
        if current.waiting {
            return ForwardOutcome::Deferred {
                reason: DeferReason::TargetAwaitingInput,
                detail: "the task waits for an approval or the person's input".to_owned(),
            };
        }
        if out_of_time(limits) {
            return ForwardOutcome::Deferred {
                reason: DeferReason::TargetUnresponsive,
                detail: "the send was stopped or out of time before its submission".to_owned(),
            };
        }
        let message_id = uuid_text(version_8(message.id));
        let submission = Submission {
            task: &task,
            message_id: &message_id,
            text: &message.text,
            created_at_unix_ms: now_unix_ms(),
        };
        let first = submission.request(&current.cwd, current.running);
        if before_write().is_err() {
            return ForwardOutcome::Failed {
                problem: DeliveryProblem::WriteFailed,
                detail: "the intent could not be recorded; nothing was submitted".to_owned(),
            };
        }
        request::deliver_prepared(&mut desktop, &submission, &current, first, limits.deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeliveryProof;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    // Everything here is invented: no test opens a pipe, and no test reaches a desktop.
    const TASK: &str = "00000000-0000-4000-8000-0000000000c1";
    const OTHER_TASK: &str = "00000000-0000-4000-8000-0000000000c9";
    const OWNER: &str = "00000000-0000-4000-8000-0000000000c2";
    const CLIENT: &str = "00000000-0000-4000-8000-0000000000c3";
    const TURN: &str = "00000000-0000-4000-8000-0000000000c4";
    const CWD: &str = r"C:\neutral\task";
    const WAKE_ID: [u8; 16] = [0x11; 16];
    /// `WAKE_ID` with the version 8 and variant bits set.
    const MESSAGE_ID: &str = "11111111-1111-8111-9111-111111111111";
    const TEXT: &str = "ActingCommand to 频道\n发送测试 正文 C:\\neutral\\w1.json";
    const START: &str = "thread-follower-start-turn";
    const STEER: &str = "thread-follower-steer-turn";

    /// A scripted desktop. It reads each frame this client writes and queues the frames the
    /// desktop's protocol answers with. A read with nothing queued is the deadline passing, at
    /// once and with no sleep.
    #[derive(Default)]
    struct Script {
        /// No owner: discovery answers `no-client-found`.
        not_loaded: bool,
        /// One per subscription; the last one stays.
        snapshots: VecDeque<Value>,
        /// The state stream's version; 11 unless a test says otherwise.
        version: Option<u64>,
        /// One per submission; none left means no answer.
        answers: VecDeque<Value>,
        /// The desktop asks its discovery question before each of its answers.
        ask_discovery: bool,
        /// The desktop closes the connection once it has a submission.
        close_after_submission: bool,
        closed: bool,
        asked: u32,
        written: Vec<Value>,
        pending: Vec<u8>,
        inbound: VecDeque<u8>,
    }

    impl Script {
        fn queue(&mut self, frame: Value) {
            let payload = serde_json::to_vec(&frame).expect("encode a scripted frame");
            let length = u32::try_from(payload.len()).expect("a small frame");
            self.inbound.extend(length.to_le_bytes());
            self.inbound.extend(payload);
        }

        fn question(&mut self) {
            if self.ask_discovery {
                self.asked += 1;
                let id = format!("discovery-{}", self.asked);
                self.queue(json!({"type": "client-discovery-request", "requestId": id}));
            }
        }

        fn answer(&mut self, request: &Value, members: Value) {
            self.question();
            let mut response = json!({"type": "response", "requestId": request["requestId"]});
            if let (Some(response), Some(members)) = (response.as_object_mut(), members.as_object())
            {
                response.extend(members.clone());
            }
            self.queue(response);
        }

        fn snapshot(&mut self) -> Value {
            if self.snapshots.len() > 1 {
                self.snapshots.pop_front().expect("a snapshot")
            } else {
                self.snapshots.front().cloned().expect("a snapshot")
            }
        }

        fn react(&mut self, frame: &Value) {
            match (frame["type"].as_str(), frame["method"].as_str()) {
                (Some("request"), Some("initialize")) => self.answer(
                    frame,
                    json!({"resultType": "success", "result": {"clientId": CLIENT}}),
                ),
                (Some("request"), Some("thread-owner-discovery")) => {
                    let members = if self.not_loaded {
                        json!({"resultType": "error", "error": "no-client-found"})
                    } else {
                        json!({"resultType": "success", "handledByClientId": OWNER})
                    };
                    self.answer(frame, members);
                }
                (Some("request"), Some(START | STEER)) => {
                    if self.close_after_submission {
                        self.closed = true;
                    } else if let Some(members) = self.answers.pop_front() {
                        self.answer(frame, members);
                    }
                }
                (Some("broadcast"), Some("thread-stream-following-changed"))
                    if frame["params"]["following"] == true =>
                {
                    // Frames for another task and from another client come first and are passed
                    // over.
                    let state = self.snapshot();
                    let version = self.version.unwrap_or(11);
                    for (source, task) in [(CLIENT, TASK), (OWNER, OTHER_TASK)] {
                        self.queue(json!({
                            "type": "broadcast", "method": "thread-stream-state-changed",
                            "version": version, "sourceClientId": source,
                            "params": {"hostId": "local", "conversationId": task,
                                       "change": {"type": "snapshot", "conversationState": {}}}
                        }));
                    }
                    self.question();
                    self.queue(json!({
                        "type": "broadcast", "method": "thread-stream-state-changed",
                        "version": version, "sourceClientId": OWNER,
                        "params": {"hostId": "local", "conversationId": TASK,
                                   "change": {"type": "snapshot", "conversationState": state}}
                    }));
                }
                _ => {}
            }
        }

        fn submissions(&self) -> Vec<Value> {
            self.written
                .iter()
                .filter(|frame| matches!(frame["method"].as_str(), Some(START | STEER)))
                .cloned()
                .collect()
        }

        fn subscriptions(&self) -> usize {
            self.written
                .iter()
                .filter(|frame| {
                    frame["method"] == "thread-stream-following-changed"
                        && frame["params"]["following"] == true
                })
                .count()
        }
    }

    struct ScriptLink(Arc<Mutex<Script>>);

    impl ByteLink for ScriptLink {
        fn send_bytes(
            &mut self,
            bytes: &[u8],
            _deadline: Instant,
            _stopped: &dyn Fn() -> bool,
        ) -> Result<(), LinkFailure> {
            let mut script = self.0.lock().expect("the script");
            script.pending.extend_from_slice(bytes);
            while script.pending.len() >= 4 {
                let length =
                    u32::from_le_bytes(script.pending[..4].try_into().expect("a frame's header"))
                        as usize;
                if script.pending.len() < 4 + length {
                    break;
                }
                let frame: Value =
                    serde_json::from_slice(&script.pending[4..4 + length]).expect("a JSON frame");
                script.pending = script.pending.split_off(4 + length);
                script.written.push(frame.clone());
                script.react(&frame);
            }
            Ok(())
        }

        fn receive_bytes(
            &mut self,
            buffer: &mut [u8],
            _deadline: Instant,
            _stopped: &dyn Fn() -> bool,
        ) -> Result<(), LinkFailure> {
            let mut script = self.0.lock().expect("the script");
            if script.inbound.len() < buffer.len() {
                return Err(if script.closed {
                    LinkFailure::Closed
                } else {
                    LinkFailure::TimedOut
                });
            }
            for byte in buffer.iter_mut() {
                *byte = script.inbound.pop_front().expect("a queued byte");
            }
            Ok(())
        }
    }

    enum Reach {
        Script(Arc<Mutex<Script>>),
        Absent,
    }

    struct ScriptConnector(Reach);

    impl Connector for ScriptConnector {
        fn connect(&self) -> Result<Box<dyn ByteLink>, ConnectFailure> {
            match &self.0 {
                Reach::Script(script) => Ok(Box::new(ScriptLink(Arc::clone(script)))),
                Reach::Absent => Err(ConnectFailure::Unavailable),
            }
        }
    }

    fn snapshot(resume: &str, runtime: &str, turns: Value, requests: Value) -> Value {
        json!({
            "cwd": CWD, "requests": requests, "turns": turns, "resumeState": resume,
            "threadRuntimeStatus": {"type": runtime}
        })
    }

    fn idle() -> Value {
        snapshot(
            "resumed",
            "idle",
            json!([{"id": "t0", "status": "completed"}]),
            json!([{"completed": true}]),
        )
    }

    fn running() -> Value {
        snapshot(
            "resumed",
            "active",
            json!([{"id": "t0", "status": "completed"}, {"id": "t1", "status": "inProgress"}]),
            json!([]),
        )
    }

    fn started() -> Value {
        json!({"resultType": "success", "result": {"result": {"turn": {"id": TURN, "status": "inProgress"}}}})
    }

    fn steered() -> Value {
        json!({"resultType": "success", "result": {"result": {"turnId": TURN}}})
    }

    fn ended(task: &str) -> Value {
        json!({"resultType": "error", "error": format!(
            "Cannot steer conversation {task} because its active turn already ended")})
    }

    fn script(snapshots: &[Value], answers: &[Value]) -> Script {
        Script {
            snapshots: snapshots.iter().cloned().collect(),
            answers: answers.iter().cloned().collect(),
            ..Script::default()
        }
    }

    /// Sends once to the scripted desktop and answers the outcome, how often the intent was asked,
    /// and the script.
    fn send_to(script: Script) -> (ForwardOutcome, u32, Arc<Mutex<Script>>) {
        let shared = Arc::new(Mutex::new(script));
        let forward = CodexForward {
            connector: Box::new(ScriptConnector(Reach::Script(Arc::clone(&shared)))),
        };
        let (outcome, intents) = send(&forward, &mut || Ok(()));
        (outcome, intents, shared)
    }

    fn send(
        forward: &CodexForward,
        intent: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> (ForwardOutcome, u32) {
        let stopped = || false;
        let limits = SendLimits {
            deadline: Instant::now() + Duration::from_secs(60),
            stopped: &stopped,
        };
        let message = ForwardMessage {
            id: WAKE_ID,
            text: TEXT.to_owned(),
        };
        let mut intents = 0;
        let outcome = forward.deliver(TASK, &message, &limits, &mut || {
            intents += 1;
            intent()
        });
        (outcome, intents)
    }

    fn detail_of(outcome: &ForwardOutcome) -> &str {
        match outcome {
            ForwardOutcome::Deferred { detail, .. }
            | ForwardOutcome::Delivered { detail, .. }
            | ForwardOutcome::Failed { detail, .. }
            | ForwardOutcome::Unknown { detail, .. } => detail,
        }
    }

    fn assert_deferred(outcome: &ForwardOutcome, expected: DeferReason) {
        assert!(
            matches!(outcome, ForwardOutcome::Deferred { reason, .. } if *reason == expected),
            "{outcome:?}"
        );
    }

    fn assert_failed(outcome: &ForwardOutcome, expected: DeliveryProblem) {
        assert!(
            matches!(outcome, ForwardOutcome::Failed { problem, .. } if *problem == expected),
            "{outcome:?}"
        );
    }

    fn assert_unknown(outcome: &ForwardOutcome, expected: DeliveryProblem) {
        assert!(
            matches!(outcome, ForwardOutcome::Unknown { problem, .. } if *problem == expected),
            "{outcome:?}"
        );
    }

    fn assert_accepted(outcome: &ForwardOutcome) {
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Delivered {
                    proof: DeliveryProof::Accepted,
                    ..
                }
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn idle_starts_a_turn_and_running_steers() {
        let (outcome, intents, shared) = send_to(script(&[idle()], &[started()]));
        assert_accepted(&outcome);
        assert_eq!(intents, 1);
        let script_state = shared.lock().expect("the script");
        let initialize = &script_state.written[0];
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["clientType"], "actingcommand-runtime");
        let submissions = script_state.submissions();
        assert_eq!(submissions.len(), 1);
        let start = &submissions[0];
        assert_eq!(start["method"], START);
        assert_eq!(start["version"], 2);
        assert_eq!(start["sourceClientId"], CLIENT);
        assert_eq!(start["targetClientId"], OWNER);
        let params = &start["params"];
        assert_eq!(params["conversationId"], TASK);
        assert_eq!(params["turnStart"]["request"]["threadId"], TASK);
        assert_eq!(
            params["turnStart"]["request"]["clientUserMessageId"],
            MESSAGE_ID
        );
        assert_eq!(params["turnStart"]["request"]["input"][0]["text"], TEXT);
        assert_eq!(params["turnStart"]["request"]["cwd"], CWD);
        assert_eq!(
            params["turnStart"]["context"]["inheritThreadSettings"],
            true
        );
        // The subscription ended before the submission.
        let unfollow = script_state
            .written
            .iter()
            .position(|frame| {
                frame["method"] == "thread-stream-following-changed"
                    && frame["params"]["following"] == false
            })
            .expect("the subscription ends");
        let submitted = script_state
            .written
            .iter()
            .position(|frame| frame["method"] == START)
            .expect("the submission");
        assert!(unfollow < submitted);
        drop(script_state);

        let (outcome, intents, shared) = send_to(script(&[running()], &[steered()]));
        assert_accepted(&outcome);
        assert_eq!(intents, 1);
        let submissions = shared.lock().expect("the script").submissions();
        assert_eq!(submissions.len(), 1);
        let steer = &submissions[0];
        assert_eq!(steer["method"], STEER);
        assert_eq!(steer["version"], 1);
        assert_eq!(steer["targetClientId"], OWNER);
        let params = &steer["params"];
        assert_eq!(params["conversationId"], TASK);
        assert_eq!(params["clientUserMessageId"], MESSAGE_ID);
        assert_eq!(params["restoreMessage"]["id"], MESSAGE_ID);
        assert_eq!(params["restoreMessage"]["cwd"], CWD);
        assert_eq!(params["input"][0]["text"], TEXT);
    }

    #[test]
    fn not_loaded_absent_pipe_and_loading_are_deferred() {
        // The desktop is not running: its pipe is absent (2, 3) or busy (231).
        for failure in [
            OpenFailure::Unavailable,
            OpenFailure::Failed(io::Error::from_raw_os_error(3)),
        ] {
            assert!(matches!(open_failure(failure), ConnectFailure::Unavailable));
        }
        assert!(matches!(
            open_failure(OpenFailure::Failed(io::Error::from_raw_os_error(5))),
            ConnectFailure::Failed(_)
        ));
        let absent = CodexForward {
            connector: Box::new(ScriptConnector(Reach::Absent)),
        };
        let (outcome, intents) = send(&absent, &mut || Ok(()));
        assert_deferred(&outcome, DeferReason::IpcUnavailable);
        assert_eq!(intents, 0);

        // No desktop client has the task loaded.
        let (outcome, intents, shared) = send_to(Script {
            not_loaded: true,
            ..script(&[idle()], &[started()])
        });
        assert_deferred(&outcome, DeferReason::TargetNotLoaded);
        assert_eq!(intents, 0);
        assert!(shared.lock().expect("the script").submissions().is_empty());

        // The owner is still loading it.
        for resume in ["resuming", "needs_resume"] {
            let loading = snapshot(resume, "idle", json!([]), json!([]));
            let (outcome, intents, shared) = send_to(script(&[loading], &[started()]));
            assert_deferred(&outcome, DeferReason::TargetNotLoaded);
            assert_eq!(intents, 0);
            assert!(shared.lock().expect("the script").submissions().is_empty());
        }
    }

    #[test]
    fn awaiting_input_is_deferred() {
        let mut confirmation = idle();
        confirmation["threadGoalResumeConfirmation"] = json!({"goal": "neutral"});
        let mut busy_and_asking = running();
        busy_and_asking["requests"] = json!([{"completed": true}, {"completed": false}]);
        for state in [
            snapshot(
                "resumed",
                "idle",
                json!([]),
                json!([{"id": "r1", "completed": false}]),
            ),
            confirmation,
            busy_and_asking,
        ] {
            let (outcome, intents, shared) = send_to(script(&[state], &[started(), steered()]));
            assert_deferred(&outcome, DeferReason::TargetAwaitingInput);
            assert_eq!(intents, 0);
            assert!(shared.lock().expect("the script").submissions().is_empty());
        }
    }

    #[test]
    fn an_ended_turn_rejection_gets_one_start_with_the_same_id() {
        // The steer is refused because its turn just ended; the same owner shows the task idle.
        let (outcome, intents, shared) =
            send_to(script(&[running(), idle()], &[ended(TASK), started()]));
        assert_accepted(&outcome);
        assert_eq!(intents, 1);
        assert!(!detail_of(&outcome).contains(TASK), "{outcome:?}");
        let script_state = shared.lock().expect("the script");
        let submissions = script_state.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[0]["method"], STEER);
        assert_eq!(submissions[1]["method"], START);
        assert_eq!(submissions[0]["params"]["clientUserMessageId"], MESSAGE_ID);
        assert_eq!(
            submissions[1]["params"]["turnStart"]["request"]["clientUserMessageId"],
            MESSAGE_ID
        );
        assert_eq!(
            submissions[0]["params"]["input"],
            submissions[1]["params"]["turnStart"]["request"]["input"]
        );
        assert_eq!(submissions[1]["targetClientId"], OWNER);
        assert_eq!(script_state.subscriptions(), 2);
        drop(script_state);

        // The one start is refused too: nothing is tried a third time.
        let refused = json!({"resultType": "error", "error": "second refusal"});
        let (outcome, intents, shared) =
            send_to(script(&[running(), idle()], &[ended(TASK), refused]));
        assert_failed(&outcome, DeliveryProblem::Rejected);
        assert_eq!(intents, 1);
        assert_eq!(shared.lock().expect("the script").submissions().len(), 2);

        // The task is busy again: no start at all.
        let (outcome, intents, shared) =
            send_to(script(&[running(), running()], &[ended(TASK), started()]));
        assert_failed(&outcome, DeliveryProblem::Rejected);
        assert_eq!(intents, 1);
        assert_eq!(shared.lock().expect("the script").submissions().len(), 1);
        assert!(!detail_of(&outcome).contains(TASK), "{outcome:?}");
    }

    #[test]
    fn other_rejections_fail_and_no_answer_is_unknown() {
        // A definite refusal of a start, and a steer refused with another task's ended turn.
        let denied = json!({"resultType": "error", "error": "permission denied"});
        for (state, answer) in [(idle(), denied), (running(), ended(OTHER_TASK))] {
            let (outcome, intents, shared) = send_to(script(&[state], &[answer]));
            assert_failed(&outcome, DeliveryProblem::Rejected);
            assert_eq!(intents, 1);
            let script_state = shared.lock().expect("the script");
            assert_eq!(script_state.submissions().len(), 1);
            assert_eq!(script_state.subscriptions(), 1, "no second read");
        }

        // No answer before the deadline; an answer of another shape; an answer with no turn id.
        let no_turn = json!({"resultType": "success", "result": {}});
        let unexpected = json!({"resultType": "unexpected"});
        for (answer, expected) in [
            (None, DeliveryProblem::WriteTimedOut),
            (Some(unexpected), DeliveryProblem::ProtocolChanged),
            (Some(no_turn), DeliveryProblem::ProtocolChanged),
        ] {
            let (outcome, intents, shared) = send_to(script(&[idle()], answer.as_slice()));
            assert_unknown(&outcome, expected);
            assert_eq!(intents, 1);
            assert_eq!(shared.lock().expect("the script").submissions().len(), 1);
        }

        // The desktop goes away once it has the submission.
        let (outcome, intents, shared) = send_to(Script {
            close_after_submission: true,
            ..script(&[idle()], &[started()])
        });
        assert_unknown(&outcome, DeliveryProblem::WriteFailed);
        assert_eq!(intents, 1);
        assert_eq!(shared.lock().expect("the script").submissions().len(), 1);

        // A refused intent submits nothing.
        let shared = Arc::new(Mutex::new(script(&[idle()], &[started()])));
        let forward = CodexForward {
            connector: Box::new(ScriptConnector(Reach::Script(Arc::clone(&shared)))),
        };
        let (outcome, intents) = send(&forward, &mut || Err(IntentFailed));
        assert_failed(&outcome, DeliveryProblem::WriteFailed);
        assert_eq!(intents, 1);
        assert!(shared.lock().expect("the script").submissions().is_empty());
    }

    #[test]
    fn an_unsupported_stream_version_is_protocol_changed() {
        let (outcome, intents, shared) = send_to(Script {
            version: Some(12),
            ..script(&[idle()], &[started()])
        });
        assert_failed(&outcome, DeliveryProblem::ProtocolChanged);
        assert_eq!(intents, 0);
        assert!(shared.lock().expect("the script").submissions().is_empty());

        // A resume state, a runtime status or a turn status this build does not know.
        for state in [
            snapshot("archived", "idle", json!([]), json!([])),
            snapshot("resumed", "paused", json!([]), json!([])),
            snapshot(
                "resumed",
                "idle",
                json!([{"id": "t0", "status": "queued"}]),
                json!([]),
            ),
        ] {
            let (outcome, intents, shared) = send_to(script(&[state], &[started()]));
            assert_failed(&outcome, DeliveryProblem::ProtocolChanged);
            assert_eq!(intents, 0);
            assert!(shared.lock().expect("the script").submissions().is_empty());
        }
    }

    #[test]
    fn discovery_requests_are_answered_cannot_handle() {
        let (outcome, intents, shared) = send_to(Script {
            ask_discovery: true,
            ..script(&[idle()], &[started()])
        });
        assert_accepted(&outcome);
        assert_eq!(intents, 1);
        let script_state = shared.lock().expect("the script");
        // Before the answers to initialize, discovery and the submission, and before the snapshot.
        assert_eq!(script_state.asked, 4);
        let answers: Vec<&Value> = script_state
            .written
            .iter()
            .filter(|frame| frame["type"] == "client-discovery-response")
            .collect();
        assert_eq!(answers.len(), 4);
        for (at, answer) in answers.iter().enumerate() {
            assert_eq!(answer["requestId"], format!("discovery-{}", at + 1));
            assert_eq!(answer["response"], json!({"canHandle": false}));
        }
    }
}
