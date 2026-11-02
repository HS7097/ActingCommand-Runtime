// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/bridge/claude.rs and scripts/runtime/src/bridge/security.rs. Copyright (c) 2026 HS7097. MIT: see ../LICENSE-codex-goal-lifecycle.txt.

//! Claude Code sessions as channels (Workflow #432; `contracts/agent-forward.md`): the live record a
//! session's hook or command stores, and one write to the session's inbox.
//!
//! The token lives only in the record's DPAPI blob and, briefly, in a [`Secret`]. No `detail` and no
//! error text here holds the token, the frame, the pipe's name or a native id, because a `detail`
//! goes to a delivery file a person reads.

use crate::Refusal;
use crate::ancestor::{self, ProcessIdentity};
use crate::channels::{ChannelClient, ChannelRegistry, RegistryReadFailure, read_registry};
use crate::cli::{locked, write_atomic};
use crate::dpapi::{self, Secret};
use crate::pipe::{OpenFailure, PipeClient, WriteFailure};
use crate::{
    DeferReason, DeliveryProblem, DeliveryProof, ForwardMessage, ForwardOutcome, IntentFailed,
    SendLimits,
};
use actingcommand_host_metrics::{ProcessProbe, probe_process};
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The `schema` a session's record carries.
const RECORD_SCHEMA: &str = "actingcommand.agent-claude-session.v1";
/// The largest hook input read.
const MAX_HOOK_INPUT: u64 = 64 * 1024;
/// The longest token stored, in bytes.
const MAX_TOKEN_BYTES: usize = 512;
/// The longest part of a pipe's name after its prefix.
const MAX_PIPE_SUFFIX: usize = 128;
/// The longest one write may take, whatever the send's deadline. Claude Code closes a connection
/// with no complete line after 30 seconds.
const MAX_WRITE: Duration = Duration::from_secs(25);

/// What a session exports to its hooks and commands.
pub(crate) const SOCKET_VARIABLE: &str = "CLAUDE_CODE_MESSAGING_SOCKET";
pub(crate) const TOKEN_VARIABLE: &str = "CLAUDE_CODE_MESSAGING_TOKEN";
pub(crate) const SESSION_VARIABLE: &str = "CLAUDE_CODE_SESSION_ID";

/// A session's own variables, read only once the session is known to be a channel.
pub(crate) struct SessionEnvironment {
    pub(crate) socket: Option<String>,
    pub(crate) token: Option<Secret>,
    /// `claude-register` only: a hook has the session id in its input.
    pub(crate) session_id: Option<String>,
}

/// What a hook did. Every one of these exits 0 and prints nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookResult {
    /// A channel's live record was stored.
    Stored,
    /// This session is no channel: nothing was read or stored.
    NotAChannel,
    /// A session's end, with no record to clear.
    NothingToClear,
    /// A session's end from another process than the record's owner: the record stays.
    OtherOwner,
    /// The record was rewritten offline, with no pipe and no token.
    Cleared,
}

/// A session's live record, `<state_root>\agent-forward\claude-sessions\<native id>.json`. A session
/// that has ended has neither `pipe` nor `protected_token`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRecord {
    schema: String,
    native_id: String,
    owner_pid: u32,
    owner_created_at_unix_ms: u64,
    online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pipe: Option<String>,
    /// The DPAPI blob in hexadecimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_token: Option<String>,
    updated_at_unix_ms: u64,
}

#[derive(Deserialize)]
struct HookInput {
    session_id: String,
    #[serde(default)]
    hook_event_name: Option<String>,
}

/// A session id in the one form this crate stores: a UUID in lower case. Anything else is refused,
/// which also makes it safe as a file name.
pub(crate) fn session_id(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 36
        && bytes.iter().enumerate().all(|(at, byte)| {
            if matches!(at, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    shaped.then(|| text.to_ascii_lowercase())
}

/// Whether `value` has the shape of a session's local inbox.
fn validate_pipe(value: &str) -> Result<(), Refusal> {
    let Some(suffix) = value
        .strip_prefix(r"\\.\pipe\LOCAL\cc-msg-")
        .or_else(|| value.strip_prefix(r"\\.\pipe\cc-msg-"))
    else {
        return Err(Refusal(
            "the inbox is not a local cc-msg named pipe".to_owned(),
        ));
    };
    if suffix.is_empty()
        || suffix.len() > MAX_PIPE_SUFFIX
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(Refusal(
            "the inbox's pipe name has another shape".to_owned(),
        ));
    }
    Ok(())
}

fn record_path(state_root: &Path, native_id: &str) -> PathBuf {
    state_root
        .join("agent-forward")
        .join("claude-sessions")
        .join(format!("{native_id}.json"))
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(*byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(*byte & 0x0f)]));
    }
    text
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    let digit = |byte: u8| char::from(byte).to_digit(16).map(|value| value as u8);
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}

/// Why a record could not be read.
enum RecordFailure {
    /// The file could not be read now.
    Read(io::Error),
    /// The file holds no record of this shape.
    Shape,
}

fn read_record(state_root: &Path, native_id: &str) -> Result<Option<SessionRecord>, RecordFailure> {
    let text = match std::fs::read_to_string(record_path(state_root, native_id)) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RecordFailure::Read(error)),
    };
    let record: SessionRecord = serde_json::from_str(&text).map_err(|_| RecordFailure::Shape)?;
    if record.schema != RECORD_SCHEMA || record.native_id != native_id {
        return Err(RecordFailure::Shape);
    }
    Ok(Some(record))
}

fn write_record(state_root: &Path, record: &SessionRecord) -> Result<(), Refusal> {
    let text = serde_json::to_vec_pretty(record)
        .map_err(|error| Refusal(format!("cannot encode a session's record: {error}")))?;
    write_atomic(&record_path(state_root, &record.native_id), &text)
}

fn read_hook_input(input: impl Read, expected_event: &str) -> Result<String, Refusal> {
    let mut bytes = Vec::new();
    input
        .take(MAX_HOOK_INPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Refusal(format!("cannot read the hook's input: {error}")))?;
    if bytes.len() as u64 > MAX_HOOK_INPUT {
        return Err(Refusal("the hook's input exceeds 64 KiB".to_owned()));
    }
    let hook: HookInput = serde_json::from_slice(&bytes).map_err(|error| {
        Refusal(format!(
            "the hook's input is not of the expected shape: {error}"
        ))
    })?;
    if hook
        .hook_event_name
        .as_deref()
        .is_some_and(|name| name != expected_event)
    {
        return Err(Refusal(format!(
            "the hook's event is not {expected_event}: check the hook's entry in the settings"
        )));
    }
    session_id(&hook.session_id)
        .ok_or_else(|| Refusal("the hook's session id is not a UUID".to_owned()))
}

fn registry_of(state_root: &Path) -> Result<Option<ChannelRegistry>, Refusal> {
    read_registry(state_root).map_err(|failure| {
        Refusal(match failure {
            RegistryReadFailure::Read(kind) => {
                format!("the channel registry cannot be read: {kind}")
            }
            RegistryReadFailure::Invalid(invalid) => {
                format!("the channel registry is invalid: {:?}", invalid.problem)
            }
        })
    })
}

/// Stores a channel's live record from its session's variables.
fn store_live(
    state_root: &Path,
    native_id: &str,
    environment: SessionEnvironment,
    owner: &dyn Fn() -> Result<ProcessIdentity, Refusal>,
    now_unix_ms: u64,
) -> Result<(), Refusal> {
    let socket = environment.socket.ok_or_else(|| {
        Refusal(format!(
            "this channel's session has no {SOCKET_VARIABLE}: cross-session messaging is off"
        ))
    })?;
    let token = environment.token.ok_or_else(|| {
        Refusal(format!(
            "this channel's session has no {TOKEN_VARIABLE}: cross-session messaging is off"
        ))
    })?;
    validate_pipe(&socket)?;
    let bytes = token.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_TOKEN_BYTES
        || bytes.iter().any(u8::is_ascii_control)
        || std::str::from_utf8(bytes).is_err()
    {
        return Err(Refusal(format!(
            "this channel's session has an invalid {TOKEN_VARIABLE}"
        )));
    }
    let owner = owner()?;
    let blob = dpapi::protect(bytes)
        .map_err(|error| Refusal(format!("cannot protect the session's token: {error}")))?;
    drop(token);
    let record = SessionRecord {
        schema: RECORD_SCHEMA.to_owned(),
        native_id: native_id.to_owned(),
        owner_pid: owner.pid,
        owner_created_at_unix_ms: owner.created_at_unix_ms,
        online: true,
        pipe: Some(socket),
        protected_token: Some(to_hex(&blob)),
        updated_at_unix_ms: now_unix_ms,
    };
    locked(state_root, || write_record(state_root, &record))
}

/// `claude-hook start`. A session that is not a registered Claude channel answers
/// [`HookResult::NotAChannel`] with nothing read from its variables and nothing stored.
pub(crate) fn hook_start(
    state_root: &Path,
    input: impl Read,
    environment: &dyn Fn() -> SessionEnvironment,
    owner: &dyn Fn() -> Result<ProcessIdentity, Refusal>,
    now_unix_ms: u64,
) -> Result<HookResult, Refusal> {
    let native_id = read_hook_input(input, "SessionStart")?;
    let Some(registry) = registry_of(state_root)? else {
        return Ok(HookResult::NotAChannel);
    };
    let is_channel = registry.channels.iter().any(|channel| {
        channel.client == ChannelClient::Claude
            && session_id(&channel.native_id).as_deref() == Some(native_id.as_str())
    });
    if !is_channel {
        return Ok(HookResult::NotAChannel);
    }
    store_live(state_root, &native_id, environment(), owner, now_unix_ms)?;
    Ok(HookResult::Stored)
}

/// `claude-hook end`: rewrites the record with no pipe and no token, unless another process owns it.
/// It looks at the record and not the registry, so a token never outlives a removed channel's end.
pub(crate) fn hook_end(
    state_root: &Path,
    input: impl Read,
    owner: &dyn Fn() -> Result<ProcessIdentity, Refusal>,
    now_unix_ms: u64,
) -> Result<HookResult, Refusal> {
    let native_id = read_hook_input(input, "SessionEnd")?;
    let record = read_record(state_root, &native_id).map_err(|failure| {
        Refusal(match failure {
            RecordFailure::Read(error) => format!("cannot read the session's record: {error}"),
            RecordFailure::Shape => "the session's record is of another shape".to_owned(),
        })
    })?;
    let Some(record) = record else {
        return Ok(HookResult::NothingToClear);
    };
    let owner = owner()?;
    if record.owner_pid != owner.pid || record.owner_created_at_unix_ms != owner.created_at_unix_ms
    {
        return Ok(HookResult::OtherOwner);
    }
    clear_record(state_root, record, now_unix_ms)?;
    Ok(HookResult::Cleared)
}

fn clear_record(state_root: &Path, record: SessionRecord, now_unix_ms: u64) -> Result<(), Refusal> {
    let cleared = SessionRecord {
        online: false,
        pipe: None,
        protected_token: None,
        updated_at_unix_ms: now_unix_ms,
        ..record
    };
    locked(state_root, || write_record(state_root, &cleared))
}

/// A channel was removed: whatever its record held goes with it. The caller holds no lock.
pub(crate) fn clear_removed(
    state_root: &Path,
    native_id: &str,
    now_unix_ms: u64,
) -> Result<(), Refusal> {
    let Some(native_id) = session_id(native_id) else {
        return Ok(());
    };
    match read_record(state_root, &native_id) {
        Ok(Some(record)) => clear_record(state_root, record, now_unix_ms),
        Ok(None) => Ok(()),
        Err(RecordFailure::Read(error)) => Err(Refusal(format!(
            "cannot read the channel's record: {error}"
        ))),
        Err(RecordFailure::Shape) => Err(Refusal(
            "the channel's record is of another shape".to_owned(),
        )),
    }
}

/// `claude-register`: the same record as the hook, for a session already running. The session's own
/// id must be the named channel's, so a credential is never stored under another session's channel.
pub(crate) fn register(
    state_root: &Path,
    name: &str,
    environment: SessionEnvironment,
    owner: &dyn Fn() -> Result<ProcessIdentity, Refusal>,
    now_unix_ms: u64,
) -> Result<(), Refusal> {
    let Some(registry) = registry_of(state_root)? else {
        return Err(Refusal(
            "no channel registry exists: add the channel first".to_owned(),
        ));
    };
    let Some(channel) = registry
        .channel(name)
        .filter(|channel| channel.client == ChannelClient::Claude)
    else {
        return Err(Refusal("no Claude channel has this name".to_owned()));
    };
    let Some(native_id) = session_id(&channel.native_id) else {
        return Err(Refusal("the channel's native id is not a UUID".to_owned()));
    };
    let own = environment.session_id.as_deref().and_then(session_id);
    if own.as_deref() != Some(native_id.as_str()) {
        return Err(Refusal(format!(
            "{SESSION_VARIABLE} is not this channel's session id: run this inside that session"
        )));
    }
    store_live(state_root, &native_id, environment, owner, now_unix_ms)
}

/// What the preflight asks the operating system, so that tests can answer with an invented owner.
pub(crate) trait OwnerSystem: Send + Sync {
    fn probe(&self, pid: u32) -> ProcessProbe;
    fn same_user(&self, pid: u32) -> io::Result<bool>;
}

struct RealSystem;

impl OwnerSystem for RealSystem {
    fn probe(&self, pid: u32) -> ProcessProbe {
        probe_process(pid)
    }

    fn same_user(&self, pid: u32) -> io::Result<bool> {
        ancestor::same_windows_user(pid)
    }
}

#[derive(Serialize)]
struct AuthLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    token: &'a str,
}

#[derive(Serialize)]
struct UserMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct UserLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    message: UserMessage<'a>,
}

/// The two lines of one message. The buffer is sized for JSON's worst case first, so it never moves
/// and leaves no copy of the token behind.
fn frame(token: &str, text: &str) -> Result<Secret, serde_json::Error> {
    let mut frame = Secret::with_capacity((token.len() + text.len()) * 6 + 128);
    serde_json::to_writer(
        frame.buffer(),
        &AuthLine {
            kind: "auth",
            token,
        },
    )?;
    frame.buffer().push(b'\n');
    serde_json::to_writer(
        frame.buffer(),
        &UserLine {
            kind: "user",
            message: UserMessage {
                role: "user",
                content: text,
            },
        },
    )?;
    frame.buffer().push(b'\n');
    Ok(frame)
}

fn offline(detail: &str) -> ForwardOutcome {
    ForwardOutcome::Deferred {
        reason: DeferReason::TargetOffline,
        detail: detail.to_owned(),
    }
}

fn failed(problem: DeliveryProblem, detail: String) -> ForwardOutcome {
    ForwardOutcome::Failed { problem, detail }
}

/// Sends to Claude Code sessions.
pub(crate) struct ClaudeForward {
    state_root: PathBuf,
    system: Box<dyn OwnerSystem>,
}

impl ClaudeForward {
    pub(crate) fn new(state_root: PathBuf) -> Self {
        Self {
            state_root,
            system: Box::new(RealSystem),
        }
    }

    /// One message to the session `native_id`: the preflight writes nothing, `before_write` commits
    /// the intent, and then there is one write. `Written` proves only the write: Claude Code holding
    /// the message, dropping it or refusing the token all look the same from here.
    pub(crate) fn deliver(
        &self,
        native_id: &str,
        message: &ForwardMessage,
        limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        let Some(native_id) = session_id(native_id) else {
            return failed(
                DeliveryProblem::ChannelUnknown,
                "the channel's native id is not a UUID".to_owned(),
            );
        };
        if (limits.stopped)() || Instant::now() >= limits.deadline {
            return ForwardOutcome::Deferred {
                reason: DeferReason::TargetUnresponsive,
                detail: "the send was stopped or out of time before it began".to_owned(),
            };
        }
        let record = match read_record(&self.state_root, &native_id) {
            Ok(Some(record)) => record,
            Ok(None) => return offline("the session has no live record"),
            Err(RecordFailure::Read(error)) => {
                return offline(&format!("the session's record cannot be read now: {error}"));
            }
            Err(RecordFailure::Shape) => {
                return failed(
                    DeliveryProblem::ProtocolChanged,
                    "the session's record is of another shape".to_owned(),
                );
            }
        };
        let (true, Some(pipe), Some(protected_token)) = (
            record.online,
            record.pipe.as_deref(),
            record.protected_token.as_deref(),
        ) else {
            return offline("the session has ended");
        };
        match self.system.probe(record.owner_pid) {
            ProcessProbe::Running { created_at_unix_ms }
                if created_at_unix_ms == record.owner_created_at_unix_ms => {}
            ProcessProbe::Running { .. } | ProcessProbe::NotRunning => {
                return offline("the session's process has gone");
            }
            ProcessProbe::Unknown(code) => {
                return failed(
                    DeliveryProblem::OwnerMismatch,
                    format!("the session's process cannot be checked: {code}"),
                );
            }
        }
        if let Err(Refusal(text)) = validate_pipe(pipe) {
            return failed(DeliveryProblem::ProtocolChanged, text);
        }
        match self.system.same_user(record.owner_pid) {
            Ok(true) => {}
            Ok(false) => {
                return failed(
                    DeliveryProblem::OwnerMismatch,
                    "the session belongs to another Windows user".to_owned(),
                );
            }
            Err(error) => {
                return failed(
                    DeliveryProblem::OwnerMismatch,
                    format!("the session's Windows user cannot be read: {error}"),
                );
            }
        }
        let client = match PipeClient::open(pipe) {
            Ok(client) => client,
            Err(OpenFailure::Unavailable) => return offline("the session's inbox is not open"),
            Err(OpenFailure::Failed(error)) => {
                return failed(
                    DeliveryProblem::WriteFailed,
                    format!("the session's inbox cannot be opened: {error}"),
                );
            }
        };
        match client.server_process_id() {
            Ok(pid) if pid == record.owner_pid => {}
            Ok(_) => {
                return failed(
                    DeliveryProblem::OwnerMismatch,
                    "another process serves the session's inbox".to_owned(),
                );
            }
            Err(error) => {
                return failed(
                    DeliveryProblem::OwnerMismatch,
                    format!("the inbox's server cannot be identified: {error}"),
                );
            }
        }
        let Some(blob) = from_hex(protected_token) else {
            return failed(
                DeliveryProblem::Rejected,
                "the session's token is not a blob".to_owned(),
            );
        };
        let token = match dpapi::unprotect(&blob) {
            Ok(token) => token,
            Err(error) => {
                return failed(
                    DeliveryProblem::Rejected,
                    format!("the session's token does not decrypt for this Windows user: {error}"),
                );
            }
        };
        let Ok(token_text) = std::str::from_utf8(token.as_bytes()) else {
            return failed(
                DeliveryProblem::Rejected,
                "the session's token is not text".to_owned(),
            );
        };
        let frame = match frame(token_text, &message.text) {
            Ok(frame) => frame,
            Err(_) => {
                return failed(
                    DeliveryProblem::WriteFailed,
                    "the message could not be framed".to_owned(),
                );
            }
        };
        drop(token);
        if before_write().is_err() {
            return failed(
                DeliveryProblem::WriteFailed,
                "the intent could not be recorded; nothing was written".to_owned(),
            );
        }
        let write_deadline = limits.deadline.min(Instant::now() + MAX_WRITE);
        let written = client.write_all(frame.as_bytes(), write_deadline, limits.stopped);
        drop(frame);
        match written {
            Ok(()) => ForwardOutcome::Delivered {
                proof: DeliveryProof::Written,
                detail: "the inbox write finished; this does not show the session accepted it"
                    .to_owned(),
            },
            Err(WriteFailure::TimedOut) => ForwardOutcome::Unknown {
                problem: DeliveryProblem::WriteTimedOut,
                detail: "the inbox write reached its deadline and was cancelled".to_owned(),
            },
            Err(WriteFailure::Stopped) => ForwardOutcome::Unknown {
                problem: DeliveryProblem::WriteFailed,
                detail: "the Runtime stopped during the inbox write".to_owned(),
            },
            Err(WriteFailure::Failed(error)) => ForwardOutcome::Unknown {
                problem: DeliveryProblem::WriteFailed,
                detail: format!("the inbox write failed: {error}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::add_channel;
    use crate::pipe::tests::TestServer;
    use tempfile::TempDir;

    // Everything here is invented: no test reads a variable of its own environment or opens a pipe
    // it did not create itself.
    const SESSION: &str = "00000000-0000-4000-8000-0000000000aa";
    const OTHER_SESSION: &str = "00000000-0000-4000-8000-0000000000bb";
    const TOKEN: &str = "invented-token-0123456789";
    const OWNER: ProcessIdentity = ProcessIdentity {
        pid: 4242,
        created_at_unix_ms: 77,
    };

    /// Answers for an owner that is running, as this process' pipe server or as an invented one.
    struct FakeSystem {
        probe: ProcessProbe,
    }

    impl OwnerSystem for FakeSystem {
        fn probe(&self, _pid: u32) -> ProcessProbe {
            self.probe
        }

        fn same_user(&self, _pid: u32) -> io::Result<bool> {
            Ok(true)
        }
    }

    fn forward(root: &Path, probe: ProcessProbe) -> ClaudeForward {
        ClaudeForward {
            state_root: root.to_path_buf(),
            system: Box::new(FakeSystem { probe }),
        }
    }

    fn running(created_at_unix_ms: u64) -> ProcessProbe {
        ProcessProbe::Running { created_at_unix_ms }
    }

    fn state_root() -> TempDir {
        tempfile::tempdir().expect("a temporary state root")
    }

    fn start_input(session: &str) -> String {
        serde_json::json!({
            "session_id": session,
            "cwd": r"C:\neutral",
            "hook_event_name": "SessionStart",
            "transcript_path": r"C:\neutral\t.jsonl",
        })
        .to_string()
    }

    fn end_input(session: &str) -> String {
        serde_json::json!({"session_id": session, "hook_event_name": "SessionEnd"}).to_string()
    }

    fn environment(pipe: &str) -> SessionEnvironment {
        SessionEnvironment {
            socket: Some(pipe.to_owned()),
            token: Some(Secret::new(TOKEN.as_bytes().to_vec())),
            session_id: None,
        }
    }

    fn add_session(root: &Path) {
        add_channel(root, "频道", "claude", SESSION, None).expect("add the channel");
    }

    /// A channel whose session stored a live record for `pipe`, owned by `owner`.
    fn live_session(root: &Path, pipe: &str, owner: ProcessIdentity) {
        add_session(root);
        let pipe = pipe.to_owned();
        let stored = hook_start(
            root,
            start_input(SESSION).as_bytes(),
            &|| environment(&pipe),
            &|| Ok(owner),
            1,
        );
        assert_eq!(stored, Ok(HookResult::Stored));
    }

    fn own_process() -> ProcessIdentity {
        ProcessIdentity {
            pid: std::process::id(),
            created_at_unix_ms: 77,
        }
    }

    fn message(text: &str) -> ForwardMessage {
        ForwardMessage {
            id: [0; 16],
            text: text.to_owned(),
        }
    }

    /// Delivers with a long deadline and answers the outcome and how often the intent was asked.
    fn deliver(forward: &ClaudeForward, text: &str) -> (ForwardOutcome, u32) {
        deliver_until(forward, text, Instant::now() + Duration::from_secs(60))
    }

    fn deliver_until(
        forward: &ClaudeForward,
        text: &str,
        deadline: Instant,
    ) -> (ForwardOutcome, u32) {
        let stopped = || false;
        let limits = SendLimits {
            deadline,
            stopped: &stopped,
        };
        let mut intents = 0;
        let outcome = forward.deliver(SESSION, &message(text), &limits, &mut || {
            intents += 1;
            Ok(())
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

    fn record_text(root: &Path, session: &str) -> Option<String> {
        match std::fs::read_to_string(record_path(root, session)) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => panic!("read the record: {error}"),
        }
    }

    #[test]
    fn frame_is_auth_line_then_user_line() {
        let root = state_root();
        let server = TestServer::new(4096);
        live_session(root.path(), &server.name, own_process());
        let sender = forward(root.path(), running(77));
        let (outcome, intents) = deliver(&sender, "ActingCommand to 频道\n发送测试 正文 \"x\"");
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Delivered {
                    proof: DeliveryProof::Written,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 1);
        let wire = String::from_utf8(server.read_to_end()).expect("the frame is text");
        assert_eq!(
            wire,
            format!(
                "{{\"type\":\"auth\",\"token\":\"{TOKEN}\"}}\n\
                 {{\"type\":\"user\",\"message\":{{\"role\":\"user\",\
                 \"content\":\"ActingCommand to 频道\\n发送测试 正文 \\\"x\\\"\"}}}}\n"
            )
        );
    }

    #[test]
    fn frame_never_appears_in_a_detail() {
        let root = state_root();
        let hidden = |text: &str, place: &str| {
            for needle in [TOKEN, "\"auth\"", SESSION, r"\\.\pipe"] {
                assert!(!text.contains(needle), "{place} holds {needle:?}: {text}");
            }
        };

        // The write finishes.
        let server = TestServer::new(4096);
        live_session(root.path(), &server.name, own_process());
        let text = record_text(root.path(), SESSION).expect("the record");
        assert!(
            !text.contains(TOKEN),
            "the record holds the token in the clear"
        );
        let (delivered, _) = deliver(&forward(root.path(), running(77)), "m");
        hidden(detail_of(&delivered), "a delivered detail");

        // Every other way a send ends.
        let (gone, _) = deliver(&forward(root.path(), ProcessProbe::NotRunning), "m");
        hidden(detail_of(&gone), "a deferred detail");
        let (other, _) = deliver(&forward(root.path(), running(78)), "m");
        hidden(detail_of(&other), "another owner's detail");

        // The server takes nothing more than its buffer: the write reaches its deadline.
        let root = state_root();
        let server = TestServer::new(16);
        live_session(root.path(), &server.name, own_process());
        let long = "x".repeat(1 << 20);
        let (timed_out, _) = deliver_until(
            &forward(root.path(), running(77)),
            &long,
            Instant::now() + Duration::from_millis(300),
        );
        assert!(
            matches!(timed_out, ForwardOutcome::Unknown { .. }),
            "{timed_out:?}"
        );
        hidden(detail_of(&timed_out), "a timed-out detail");

        // A hook's failure names the variable and never its value.
        let root = state_root();
        add_session(root.path());
        let failure = hook_start(
            root.path(),
            start_input(SESSION).as_bytes(),
            &|| SessionEnvironment {
                socket: Some("not a pipe".to_owned()),
                token: Some(Secret::new(TOKEN.as_bytes().to_vec())),
                session_id: None,
            },
            &|| Ok(OWNER),
            1,
        )
        .expect_err("a bad pipe fails");
        hidden(&failure.0, "a hook's failure");
    }

    #[test]
    fn pipe_name_outside_the_inbox_shape_is_refused() {
        for good in [
            r"\\.\pipe\cc-msg-0123abcdef-99",
            r"\\.\pipe\LOCAL\cc-msg-0123abcdef",
        ] {
            assert_eq!(validate_pipe(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            r"\\.\pipe\cc-msg-",
            r"\\.\pipe\codex-ipc",
            r"\\.\pipe\cc-msg-xyz",
            r"\\.\pipe\cc-msg-00\..\other",
            r"\\remote\pipe\cc-msg-00",
            "/tmp/cc-msg-00",
            "uds:/tmp/cc-msg-00",
        ] {
            assert!(validate_pipe(bad).is_err(), "{bad}");
        }
        assert!(
            validate_pipe(&format!(
                r"\\.\pipe\cc-msg-{}",
                "0".repeat(MAX_PIPE_SUFFIX + 1)
            ))
            .is_err()
        );

        // A record that holds such a name fails before the intent, and no hook stores one.
        let root = state_root();
        add_session(root.path());
        let record = SessionRecord {
            schema: RECORD_SCHEMA.to_owned(),
            native_id: SESSION.to_owned(),
            owner_pid: std::process::id(),
            owner_created_at_unix_ms: 77,
            online: true,
            pipe: Some(r"\\.\pipe\neutral".to_owned()),
            protected_token: Some("00".to_owned()),
            updated_at_unix_ms: 1,
        };
        write_record(root.path(), &record).expect("write the record");
        let (outcome, intents) = deliver(&forward(root.path(), running(77)), "m");
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Failed {
                    problem: DeliveryProblem::ProtocolChanged,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 0);
    }

    #[test]
    fn missing_record_or_dead_owner_is_deferred_before_intent() {
        let deferred = |outcome: &ForwardOutcome| {
            assert!(
                matches!(
                    outcome,
                    ForwardOutcome::Deferred {
                        reason: DeferReason::TargetOffline,
                        ..
                    }
                ),
                "{outcome:?}"
            );
        };
        // No record at all.
        let root = state_root();
        add_session(root.path());
        let (outcome, intents) = deliver(&forward(root.path(), running(77)), "m");
        deferred(&outcome);
        assert_eq!(intents, 0);

        // A live record whose owner has gone, and one whose id now names a younger process.
        let server = TestServer::new(4096);
        let root = state_root();
        live_session(root.path(), &server.name, own_process());
        for probe in [ProcessProbe::NotRunning, running(78)] {
            let (outcome, intents) = deliver(&forward(root.path(), probe), "m");
            deferred(&outcome);
            assert_eq!(intents, 0);
        }

        // A session that has ended.
        let ended = hook_end(
            root.path(),
            end_input(SESSION).as_bytes(),
            &|| Ok(own_process()),
            2,
        );
        assert_eq!(ended, Ok(HookResult::Cleared));
        let (outcome, intents) = deliver(&forward(root.path(), running(77)), "m");
        deferred(&outcome);
        assert_eq!(intents, 0);

        // An inbox nobody serves.
        let root = state_root();
        let name = TestServer::new(4096).name.clone();
        live_session(root.path(), &name, own_process());
        let (outcome, intents) = deliver(&forward(root.path(), running(77)), "m");
        deferred(&outcome);
        assert_eq!(intents, 0);
    }

    #[test]
    fn a_different_server_pid_fails_before_intent() {
        let root = state_root();
        let server = TestServer::new(4096);
        // This process serves the pipe; the record names another owner.
        live_session(
            root.path(),
            &server.name,
            ProcessIdentity {
                pid: std::process::id().wrapping_add(1),
                created_at_unix_ms: 77,
            },
        );
        let (outcome, intents) = deliver(&forward(root.path(), running(77)), "m");
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Failed {
                    problem: DeliveryProblem::OwnerMismatch,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 0);
    }

    #[test]
    fn a_write_error_after_intent_is_unknown() {
        let root = state_root();
        let server = TestServer::new(4096);
        live_session(root.path(), &server.name, own_process());
        let stopped = || false;
        let limits = SendLimits {
            deadline: Instant::now() + Duration::from_secs(60),
            stopped: &stopped,
        };
        // The server goes away between the intent and the write.
        let mut server = Some(server);
        let sender = forward(root.path(), running(77));
        let outcome = sender.deliver(SESSION, &message("m"), &limits, &mut || {
            drop(server.take());
            Ok(())
        });
        assert!(server.is_none(), "the intent ran");
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Unknown {
                    problem: DeliveryProblem::WriteFailed,
                    ..
                }
            ),
            "{outcome:?}"
        );

        // A refused intent, on the other hand, certainly wrote nothing.
        let root = state_root();
        let server = TestServer::new(4096);
        live_session(root.path(), &server.name, own_process());
        let sender = forward(root.path(), running(77));
        let outcome = sender.deliver(SESSION, &message("m"), &limits, &mut || Err(IntentFailed));
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Failed {
                    problem: DeliveryProblem::WriteFailed,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert!(server.read_to_end().is_empty());
    }

    #[test]
    fn hook_stores_nothing_for_a_session_that_is_no_channel() {
        let no_variables = || -> SessionEnvironment {
            panic!("a session that is no channel has its variables left alone")
        };
        let no_owner = || -> Result<ProcessIdentity, Refusal> {
            panic!("a session that is no channel is not looked into")
        };
        let start = |root: &Path| {
            hook_start(
                root,
                start_input(SESSION).as_bytes(),
                &no_variables,
                &no_owner,
                1,
            )
        };
        // No registry file.
        let root = state_root();
        assert_eq!(start(root.path()), Ok(HookResult::NotAChannel));
        assert!(
            !root.path().join("agent-forward").exists(),
            "nothing was created"
        );
        // Another session's channel.
        let root = state_root();
        add_channel(root.path(), "频道", "claude", OTHER_SESSION, None).expect("add");
        assert_eq!(start(root.path()), Ok(HookResult::NotAChannel));
        assert_eq!(record_text(root.path(), SESSION), None);
        // Another client's channel with this id.
        let root = state_root();
        add_channel(root.path(), "频道", "codex", SESSION, None).expect("add");
        assert_eq!(start(root.path()), Ok(HookResult::NotAChannel));
        assert_eq!(record_text(root.path(), SESSION), None);
        // The end of a session that never stored anything.
        let ended = hook_end(root.path(), end_input(SESSION).as_bytes(), &no_owner, 2);
        assert_eq!(ended, Ok(HookResult::NothingToClear));
        assert_eq!(record_text(root.path(), SESSION), None);
    }

    #[test]
    fn hook_for_a_channel_needs_both_variables_and_fails_loud() {
        let pipe = r"\\.\pipe\cc-msg-00";
        let root = state_root();
        add_session(root.path());
        for (socket, token, names) in [
            (None, Some(TOKEN), SOCKET_VARIABLE),
            (Some(pipe), None, TOKEN_VARIABLE),
            (None, None, SOCKET_VARIABLE),
            (Some(pipe), Some(""), TOKEN_VARIABLE),
            (Some(pipe), Some("line\nbreak"), TOKEN_VARIABLE),
        ] {
            let failure = hook_start(
                root.path(),
                start_input(SESSION).as_bytes(),
                &|| SessionEnvironment {
                    socket: socket.map(str::to_owned),
                    token: token.map(|value| Secret::new(value.as_bytes().to_vec())),
                    session_id: None,
                },
                &|| Ok(OWNER),
                1,
            )
            .expect_err("a channel's session without both variables fails");
            assert!(failure.0.contains(names), "{failure:?}");
            assert!(!failure.0.contains(TOKEN), "{failure:?}");
            assert_eq!(record_text(root.path(), SESSION), None, "nothing is stored");
        }
    }

    #[test]
    fn hook_end_clears_the_blob_and_ignores_another_owner() {
        let root = state_root();
        live_session(root.path(), r"\\.\pipe\cc-msg-00", OWNER);
        let stored = record_text(root.path(), SESSION).expect("the record");
        assert!(stored.contains("protected_token"));

        // A stale end from another process, and from a process with the owner's id but not its age.
        for other in [
            ProcessIdentity {
                pid: OWNER.pid + 1,
                ..OWNER
            },
            ProcessIdentity {
                created_at_unix_ms: OWNER.created_at_unix_ms + 1,
                ..OWNER
            },
        ] {
            let ended = hook_end(root.path(), end_input(SESSION).as_bytes(), &|| Ok(other), 2);
            assert_eq!(ended, Ok(HookResult::OtherOwner));
            assert_eq!(record_text(root.path(), SESSION).as_ref(), Some(&stored));
        }

        let ended = hook_end(root.path(), end_input(SESSION).as_bytes(), &|| Ok(OWNER), 3);
        assert_eq!(ended, Ok(HookResult::Cleared));
        let cleared = record_text(root.path(), SESSION).expect("the record stays");
        assert!(!cleared.contains("protected_token"), "{cleared}");
        assert!(!cleared.contains("pipe"), "{cleared}");
        let record: SessionRecord = serde_json::from_str(&cleared).expect("a record");
        assert!(!record.online);
        assert_eq!(record.owner_pid, OWNER.pid);
        assert_eq!(record.updated_at_unix_ms, 3);
    }
}
