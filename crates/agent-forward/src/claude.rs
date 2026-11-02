// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/bridge/claude.rs and scripts/runtime/src/bridge/security.rs. Copyright (c) 2026 HS7097. MIT: see ../LICENSE-codex-goal-lifecycle.txt.

//! Claude Code sessions as channels (Workflow #432; `contracts/agent-forward.md`): the live record a
//! session's hook or command stores, and one write to the session's inbox. A stopped session of a
//! channel that allows it is first restored through Claude Desktop ([`restore`], AGT-T1b).
//!
//! The token lives only in the record's DPAPI blob and, briefly, in a [`Secret`]. No `detail` and no
//! error text here holds the token, the frame, the pipe's name or a native id, because a `detail`
//! goes to a delivery file a person reads.

mod restore;

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
use restore::{
    MAX_RESTORE, REGISTRATION_POLL, RestoreWait, Restorer, Unrestored, reserve_wake, unresponsive,
    waiting,
};
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
    /// AGT-T1b: when a restore of this session got no SessionStart registration by its deadline.
    /// While it is set, no message restores the session. A registration writes a new record
    /// without it, which is what clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_unanswered_at_unix_ms: Option<u64>,
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
        // A registration: any restore mark goes with the old record.
        restore_unanswered_at_unix_ms: None,
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

/// `outcome` with `note` before its detail.
fn noted(outcome: ForwardOutcome, note: &str) -> ForwardOutcome {
    let with = |detail: String| format!("{note}; {detail}");
    match outcome {
        ForwardOutcome::Deferred { reason, detail } => ForwardOutcome::Deferred {
            reason,
            detail: with(detail),
        },
        ForwardOutcome::Delivered { proof, detail } => ForwardOutcome::Delivered {
            proof,
            detail: with(detail),
        },
        ForwardOutcome::Failed { problem, detail } => ForwardOutcome::Failed {
            problem,
            detail: with(detail),
        },
        ForwardOutcome::Unknown { problem, detail } => ForwardOutcome::Unknown {
            problem,
            detail: with(detail),
        },
    }
}

/// Whether a session can take a message now.
enum Liveness {
    /// Its record is online and its owner runs with the record's creation time.
    Live(SessionRecord),
    /// It has no live record: none, an ended one, or one whose process has gone. This is the
    /// stopped session a channel may restore, unless an earlier restore went `unanswered`.
    Stopped { why: &'static str, unanswered: bool },
    /// Its record cannot be read now.
    Unreadable(io::Error),
    /// The send ends here, with no write.
    Ends(ForwardOutcome),
}

/// A session with no live record, as [`ClaudeForward::liveness`] found it.
#[derive(Clone, Copy)]
struct StoppedSession<'a> {
    native_id: &'a str,
    why: &'static str,
    /// An earlier restore of it got no registration, and none has come since.
    unanswered: bool,
}

/// Sends to Claude Code sessions.
pub(crate) struct ClaudeForward {
    state_root: PathBuf,
    system: Box<dyn OwnerSystem>,
    restorer: Restorer,
}

impl ClaudeForward {
    pub(crate) fn new(state_root: PathBuf) -> Self {
        Self {
            restorer: Restorer::new(&state_root),
            state_root,
            system: Box::new(RealSystem),
        }
    }

    /// One message to the session `native_id`: the preflight writes nothing, `before_write` commits
    /// the intent, and then there is one write. `Written` proves only the write: Claude Code holding
    /// the message, dropping it or refusing the token all look the same from here.
    ///
    /// A session with no live record waits, unless `restore` (the channel's switch) allows a
    /// restore and no quiet time covers now: then Claude Desktop is asked to bring its conversation
    /// up, and the send goes on once the session's hook has stored its live record. Nothing is ever
    /// typed into the window.
    pub(crate) fn deliver(
        &self,
        native_id: &str,
        restore: bool,
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
            return unresponsive("the send was stopped or out of time before it began");
        }
        match self.liveness(&native_id) {
            Liveness::Live(record) => self.write(&record, message, limits, before_write),
            Liveness::Stopped { why, unanswered } => {
                let session = StoppedSession {
                    native_id: &native_id,
                    why,
                    unanswered,
                };
                match self.restore(&session, &message.id, restore, limits) {
                    Ok((record, note)) => {
                        noted(self.write(&record, message, limits, before_write), &note)
                    }
                    Err(outcome) => outcome,
                }
            }
            Liveness::Unreadable(error) => {
                offline(&format!("the session's record cannot be read now: {error}"))
            }
            Liveness::Ends(outcome) => outcome,
        }
    }

    fn liveness(&self, native_id: &str) -> Liveness {
        let record = match read_record(&self.state_root, native_id) {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Liveness::Stopped {
                    why: "the session has no live record",
                    unanswered: false,
                };
            }
            Err(RecordFailure::Read(error)) => return Liveness::Unreadable(error),
            Err(RecordFailure::Shape) => {
                return Liveness::Ends(failed(
                    DeliveryProblem::ProtocolChanged,
                    "the session's record is of another shape".to_owned(),
                ));
            }
        };
        let unanswered = record.restore_unanswered_at_unix_ms.is_some();
        if !record.online || record.pipe.is_none() || record.protected_token.is_none() {
            return Liveness::Stopped {
                why: "the session has ended",
                unanswered,
            };
        }
        match self.system.probe(record.owner_pid) {
            ProcessProbe::Running { created_at_unix_ms }
                if created_at_unix_ms == record.owner_created_at_unix_ms =>
            {
                Liveness::Live(record)
            }
            ProcessProbe::Running { .. } | ProcessProbe::NotRunning => Liveness::Stopped {
                why: "the session's process has gone",
                unanswered,
            },
            ProcessProbe::Unknown(code) => Liveness::Ends(failed(
                DeliveryProblem::OwnerMismatch,
                format!("the session's process cannot be checked: {code}"),
            )),
        }
    }

    /// Restores the stopped session and waits for its live record, within the send's deadline and
    /// at most [`MAX_RESTORE`]. `Ok` holds the record and a note for the detail; `Err` ends the
    /// send, with no write.
    ///
    /// A message wakes at most once (`message_id` is recorded before the helper runs), and a
    /// session whose wake got no registration by its deadline is marked and not woken again,
    /// for any message, until a registration rewrites its record.
    fn restore(
        &self,
        session: &StoppedSession<'_>,
        message_id: &[u8; 16],
        restore: bool,
        limits: &SendLimits<'_>,
    ) -> Result<(SessionRecord, String), ForwardOutcome> {
        let StoppedSession {
            native_id,
            why,
            unanswered,
        } = *session;
        if !restore {
            return Err(offline(&format!(
                "{why}; this channel does not restore a stopped session"
            )));
        }
        if unanswered {
            return Err(waiting(
                RestoreWait::Unanswered,
                &format!(
                    "{why}; an earlier restore of this session got no SessionStart registration \
                     by its deadline, and none has come since"
                ),
            ));
        }
        if (limits.quiet)() {
            return Err(waiting(
                RestoreWait::Quiet,
                &format!("{why}; a quiet window or the hold covers now"),
            ));
        }
        let clock = &*self.restorer.clock;
        let started = clock.now();
        let deadline = limits.deadline.min(started + MAX_RESTORE);
        if started >= deadline {
            return Err(unresponsive(
                "no time was left to restore the stopped session",
            ));
        }
        match reserve_wake(&self.state_root, message_id) {
            Ok(true) => {}
            Ok(false) => {
                return Err(waiting(
                    RestoreWait::WakeSpent,
                    &format!("{why}; this message already had its one restore"),
                ));
            }
            Err(Refusal(text)) => {
                return Err(failed(
                    DeliveryProblem::HelperFailed,
                    format!("the restore could not be recorded, so none was started: {text}"),
                ));
            }
        }
        let note = match self.restorer.wake(native_id, deadline, limits.stopped) {
            Ok(note) => note,
            Err(Unrestored {
                outcome,
                unanswered: true,
            }) => return Err(self.mark_unanswered(native_id, outcome)),
            Err(Unrestored { outcome, .. }) => return Err(outcome),
        };
        loop {
            match self.liveness(native_id) {
                Liveness::Live(record) => {
                    if (limits.stopped)() || Instant::now() >= limits.deadline {
                        return Err(unresponsive(format!(
                            "{note}; no time was left for the message, so nothing was written"
                        )));
                    }
                    return Ok((record, note));
                }
                Liveness::Ends(outcome) => return Err(noted(outcome, &note)),
                // A record being replaced can be unreadable for a moment.
                Liveness::Stopped { .. } | Liveness::Unreadable(_) => {}
            }
            if (limits.stopped)() {
                return Err(unresponsive(format!(
                    "{note}; the Runtime stopped while the session started"
                )));
            }
            let now = clock.now();
            if now >= deadline {
                return Err(self.mark_unanswered(
                    native_id,
                    waiting(
                        RestoreWait::NotRegistered,
                        "Claude Desktop shows the conversation, and the session stored no live \
                         record by the deadline (its SessionStart hook stores it)",
                    ),
                ));
            }
            clock.pause((deadline - now).min(REGISTRATION_POLL));
        }
    }

    /// Marks the session `native_id` as restored with no registration, then answers `outcome`; the
    /// mark goes into its record, under the folder's lock. A record that has turned live meanwhile
    /// is left alone. A mark that cannot be written fails the send loudly.
    fn mark_unanswered(&self, native_id: &str, outcome: ForwardOutcome) -> ForwardOutcome {
        let marked = locked(&self.state_root, || {
            let at = crate::cli::now_unix_ms()?;
            let record = match self.liveness(native_id) {
                Liveness::Live(_) => return Ok(()),
                Liveness::Unreadable(error) => {
                    return Err(Refusal(format!(
                        "the session's record cannot be read: {error}"
                    )));
                }
                Liveness::Ends(_) => {
                    return Err(Refusal("the session's record cannot be checked".to_owned()));
                }
                Liveness::Stopped { .. } => read_record(&self.state_root, native_id)
                    .map_err(|_| Refusal("the session's record cannot be read".to_owned()))?,
            };
            let marked = match record {
                Some(record) => SessionRecord {
                    updated_at_unix_ms: at,
                    restore_unanswered_at_unix_ms: Some(at),
                    ..record
                },
                // No session ever registered: a record that holds only the mark.
                None => SessionRecord {
                    schema: RECORD_SCHEMA.to_owned(),
                    native_id: native_id.to_owned(),
                    owner_pid: 0,
                    owner_created_at_unix_ms: 0,
                    online: false,
                    pipe: None,
                    protected_token: None,
                    updated_at_unix_ms: at,
                    restore_unanswered_at_unix_ms: Some(at),
                },
            };
            write_record(&self.state_root, &marked)
        });
        match marked {
            Ok(()) => outcome,
            Err(Refusal(text)) => failed(
                DeliveryProblem::HelperFailed,
                format!(
                    "the session got no registration after its restore, and the mark that stops \
                     further restores could not be written: {text}"
                ),
            ),
        }
    }

    /// The preflight of a live session, the intent, and the one write.
    fn write(
        &self,
        record: &SessionRecord,
        message: &ForwardMessage,
        limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        let (Some(pipe), Some(protected_token)) =
            (record.pipe.as_deref(), record.protected_token.as_deref())
        else {
            return offline("the session has ended");
        };
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
    use super::restore::{HelperFailure, Route, SOURCE, WaitClock, WakeHelper};
    use super::*;
    use crate::cli::{self, CliAnswer, CliFailure, UsageProblem, add_channel};
    use crate::pipe::tests::TestServer;
    use crate::process::Finished;
    use serde_json::{Value, json};
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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

    /// A forwarder whose restorer may not be used: the tests of a send that restores nothing.
    fn forward(root: &Path, probe: ProcessProbe) -> ClaudeForward {
        ClaudeForward {
            state_root: root.to_path_buf(),
            system: Box::new(FakeSystem { probe }),
            restorer: Restorer {
                profile: None,
                helper: Box::new(NoHelper),
                clock: Box::new(restore::RealClock),
            },
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

    /// A send of a channel that does not restore its stopped session: the path these tests pin.
    fn deliver_until(
        forward: &ClaudeForward,
        text: &str,
        deadline: Instant,
    ) -> (ForwardOutcome, u32) {
        let stopped = || false;
        let limits = SendLimits {
            deadline,
            stopped: &stopped,
            quiet: &stopped,
        };
        let mut intents = 0;
        let outcome = forward.deliver(SESSION, false, &message(text), &limits, &mut || {
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
            restore_unanswered_at_unix_ms: None,
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
            quiet: &stopped,
        };
        // The server goes away between the intent and the write.
        let mut server = Some(server);
        let sender = forward(root.path(), running(77));
        let outcome = sender.deliver(SESSION, false, &message("m"), &limits, &mut || {
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
        let outcome = sender.deliver(SESSION, false, &message("m"), &limits, &mut || {
            Err(IntentFailed)
        });
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

    // ---- AGT-T1b: restoring a stopped session. Claude Desktop is never reached: the profile is
    // an invented folder, the helper a fake that answers as the real one would, and the wait's
    // clock moves only when the wait pauses. ----

    const ACCOUNT: &str = "00000000-0000-4000-8000-0000000000cc";
    const OTHER_ACCOUNT: &str = "00000000-0000-4000-8000-0000000000dd";
    const LOCAL: &str = "local_00000000-0000-4000-8000-0000000000ee";
    const OTHER_LOCAL: &str = "local_00000000-0000-4000-8000-0000000000ff";
    const TEXT: &str = "ActingCommand to 频道\n发送测试";

    fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
        shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The helper of the tests that restore nothing: running it fails the test.
    struct NoHelper;

    impl WakeHelper for NoHelper {
        fn run(
            &self,
            _request: Vec<u8>,
            _deadline: Instant,
            _stopped: &dyn Fn() -> bool,
        ) -> Result<Finished, HelperFailure> {
            panic!("this test restores no session")
        }
    }

    /// The requests a fake helper was given.
    type Seen = Arc<Mutex<Vec<Vec<u8>>>>;

    /// A helper that records its request, does `effect` (what Claude Desktop and the session's
    /// hook would do meanwhile), and answers `answer`, exiting 0 when `success`.
    struct FakeHelper {
        seen: Seen,
        answer: Value,
        success: bool,
        effect: Box<dyn Fn() + Send + Sync>,
    }

    impl WakeHelper for FakeHelper {
        fn run(
            &self,
            request: Vec<u8>,
            _deadline: Instant,
            _stopped: &dyn Fn() -> bool,
        ) -> Result<Finished, HelperFailure> {
            lock(&self.seen).push(request);
            (self.effect)();
            Ok(Finished {
                success: self.success,
                stdout: self.answer.to_string().into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    fn fake_helper(
        answer: Value,
        success: bool,
        effect: impl Fn() + Send + Sync + 'static,
    ) -> (FakeHelper, Seen) {
        let seen = Seen::default();
        let helper = FakeHelper {
            seen: Arc::clone(&seen),
            answer,
            success,
            effect: Box::new(effect),
        };
        (helper, seen)
    }

    fn loaded() -> Value {
        json!({"state": "loaded", "process_id": 1})
    }

    /// A clock that moves only when the wait pauses.
    struct FakeClock {
        start: Instant,
        waited: Arc<Mutex<Duration>>,
    }

    impl WaitClock for FakeClock {
        fn now(&self) -> Instant {
            self.start + *lock(&self.waited)
        }

        fn pause(&self, length: Duration) {
            *lock(&self.waited) += length;
        }
    }

    /// A forwarder that restores through `helper` with the invented profile `profile`, and how
    /// long its wait has paused.
    fn restoring(
        root: &Path,
        profile: Option<PathBuf>,
        helper: FakeHelper,
    ) -> (ClaudeForward, Arc<Mutex<Duration>>) {
        let waited = Arc::new(Mutex::new(Duration::ZERO));
        let forward = ClaudeForward {
            state_root: root.to_path_buf(),
            system: Box::new(FakeSystem { probe: running(77) }),
            restorer: Restorer {
                profile,
                helper: Box::new(helper),
                clock: Box::new(FakeClock {
                    start: Instant::now(),
                    waited: Arc::clone(&waited),
                }),
            },
        };
        (forward, waited)
    }

    /// One send of [`TEXT`] by a channel whose switch is `restore`, while the worker judges
    /// `quiet`; the outcome and how often the intent was asked.
    fn send(forward: &ClaudeForward, restore: bool, quiet: bool) -> (ForwardOutcome, u32) {
        let stopped = || false;
        let quiet = move || quiet;
        let limits = SendLimits {
            deadline: Instant::now() + Duration::from_secs(60),
            stopped: &stopped,
            quiet: &quiet,
        };
        let mut intents = 0;
        let outcome = forward.deliver(SESSION, restore, &message(TEXT), &limits, &mut || {
            intents += 1;
            Ok(())
        });
        (outcome, intents)
    }

    /// Writes an invented Claude Desktop profile: its active account, and one pointer per
    /// `(conversation, session, archived)` under that account.
    fn write_profile(profile: &Path, account: &str, pointers: &[(&str, &str, bool)]) {
        let folder = profile
            .join("claude-code-sessions")
            .join(account)
            .join("neutral");
        std::fs::create_dir_all(&folder).expect("the profile's folders");
        std::fs::write(
            profile.join("config.json"),
            json!({"lastKnownAccountUuid": account}).to_string(),
        )
        .expect("config.json");
        for (local, session, archived) in pointers {
            std::fs::write(
                folder.join(format!("{local}.json")),
                json!({"sessionId": local, "cliSessionId": session, "isArchived": archived})
                    .to_string(),
            )
            .expect("a pointer");
        }
    }

    /// The profile folder under `root`, written as [`write_profile`] does.
    fn profile(root: &Path, account: &str, pointers: &[(&str, &str, bool)]) -> PathBuf {
        let profile = root.join("profile");
        write_profile(&profile, account, pointers);
        profile
    }

    /// The one conversation of the session, under the active account.
    fn one_conversation(root: &Path) -> Option<PathBuf> {
        Some(profile(root, ACCOUNT, &[(LOCAL, SESSION, false)]))
    }

    /// What the session's SessionStart hook does once Claude Desktop has started it.
    fn registers(root: &Path, pipe: &str) -> impl Fn() + Send + Sync + 'static {
        let root = root.to_path_buf();
        let pipe = pipe.to_owned();
        move || {
            let stored = hook_start(
                &root,
                start_input(SESSION).as_bytes(),
                &|| environment(&pipe),
                &|| Ok(own_process()),
                2,
            );
            assert_eq!(stored, Ok(HookResult::Stored));
        }
    }

    #[test]
    fn a_live_session_is_not_woken() {
        let root = state_root();
        let server = TestServer::new(4096);
        live_session(root.path(), &server.name, own_process());
        // No profile exists, so a restore would fail; the helper records any run.
        let (helper, seen) = fake_helper(loaded(), true, || {});
        let (sender, waited) = restoring(root.path(), Some(root.path().join("absent")), helper);
        let (outcome, intents) = send(&sender, true, false);
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
        assert!(!detail_of(&outcome).contains("restored"), "{outcome:?}");
        assert_eq!(intents, 1);
        assert!(lock(&seen).is_empty(), "nothing was restored");
        assert_eq!(*lock(&waited), Duration::ZERO);
        let wire = String::from_utf8(server.read_to_end()).expect("the frame is text");
        assert!(wire.contains("发送测试"), "{wire}");
    }

    #[test]
    fn restore_off_never_wakes() {
        let words = |root: &Path, rest: &[&str]| -> Vec<OsString> {
            let mut words: Vec<OsString> = ["add", "--state-root"]
                .into_iter()
                .map(OsString::from)
                .collect();
            words.push(root.as_os_str().to_owned());
            words.extend(["--name", "频道"].into_iter().map(OsString::from));
            words.extend(rest.iter().map(OsString::from));
            words
        };

        // `add` writes the switch: off when asked.
        let root = state_root();
        let added = cli::run(&words(
            root.path(),
            &[
                "--client",
                "claude",
                "--native-id",
                SESSION,
                "--restore",
                "off",
            ],
        ));
        assert!(
            matches!(&added, Ok(CliAnswer::Members(members)) if members["channel"]["restore"] == json!(false)),
            "{added:?}"
        );
        let registry = read_registry(root.path())
            .expect("a readable registry")
            .expect("a registry");
        let channel = registry.channel("频道").expect("the channel");
        assert!(!channel.restore);

        // On by default; refused for another client, and for a word other than on or off.
        let other = state_root();
        cli::run(&words(
            other.path(),
            &["--client", "claude", "--native-id", SESSION],
        ))
        .expect("the channel is added");
        let by_default = read_registry(other.path())
            .expect("a readable registry")
            .expect("a registry");
        assert!(by_default.channel("频道").expect("the channel").restore);
        let third = state_root();
        let refused = cli::run(&words(
            third.path(),
            &["--client", "codex", "--native-id", "x", "--restore", "on"],
        ));
        assert!(
            matches!(&refused, Err(CliFailure::Failed { text }) if text.contains("--restore")),
            "{refused:?}"
        );
        let refused = cli::run(&words(
            third.path(),
            &[
                "--client",
                "claude",
                "--native-id",
                SESSION,
                "--restore",
                "yes",
            ],
        ));
        assert!(
            matches!(
                &refused,
                Err(CliFailure::Usage {
                    problem: UsageProblem::NotRecognised,
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(read_registry(third.path()), Ok(None), "nothing was written");

        // A stopped session of the channel that is off: no record, then an ended one.
        let (helper, seen) = fake_helper(loaded(), true, || {});
        let (sender, waited) = restoring(root.path(), one_conversation(root.path()), helper);
        let offline = |outcome: &ForwardOutcome| {
            matches!(
                outcome,
                ForwardOutcome::Deferred {
                    reason: DeferReason::TargetOffline,
                    ..
                }
            )
        };
        let (outcome, intents) = send(&sender, channel.restore, false);
        assert!(offline(&outcome), "{outcome:?}");
        assert_eq!(intents, 0);
        registers(root.path(), r"\\.\pipe\cc-msg-00")();
        let ended = hook_end(
            root.path(),
            end_input(SESSION).as_bytes(),
            &|| Ok(own_process()),
            3,
        );
        assert_eq!(ended, Ok(HookResult::Cleared));
        let (outcome, intents) = send(&sender, channel.restore, false);
        assert!(offline(&outcome), "{outcome:?}");
        assert_eq!(intents, 0);
        assert!(lock(&seen).is_empty(), "nothing was restored");
        assert_eq!(*lock(&waited), Duration::ZERO);
    }

    #[test]
    fn quiet_window_defers_without_waking() {
        let root = state_root();
        add_session(root.path());
        let (helper, seen) = fake_helper(loaded(), true, || {});
        let (sender, waited) = restoring(root.path(), one_conversation(root.path()), helper);
        let (outcome, intents) = send(&sender, true, true);
        assert!(
            matches!(
                &outcome,
                ForwardOutcome::Deferred {
                    reason: DeferReason::TargetOffline,
                    detail,
                } if detail.contains("(quiet)")
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 0);
        assert!(lock(&seen).is_empty(), "nothing was restored");
        assert_eq!(*lock(&waited), Duration::ZERO);

        // Once the quiet time is over, the same channel's send restores.
        let (outcome, intents) = send(&sender, true, false);
        assert_eq!(lock(&seen).len(), 1, "{outcome:?}");
        assert_eq!(intents, 0);
    }

    #[test]
    fn a_pointer_set_that_is_not_exactly_one_conversation_fails() {
        // Exactly one is a route.
        let root = state_root();
        let desktop = profile(
            root.path(),
            ACCOUNT,
            &[(LOCAL, SESSION, false), (OTHER_LOCAL, OTHER_SESSION, false)],
        );
        assert_eq!(
            restore::route(&desktop, SESSION),
            Ok(Route {
                account: ACCOUNT.to_owned(),
                local_session: LOCAL.to_owned(),
            })
        );

        // A send to a stopped session of a channel whose profile `desktop` writes.
        fn fails(desktop: impl Fn(&Path) -> PathBuf) {
            let root = state_root();
            add_session(root.path());
            let (helper, seen) = fake_helper(loaded(), true, || {});
            let (sender, _) = restoring(root.path(), Some(desktop(root.path())), helper);
            let (outcome, intents) = send(&sender, true, false);
            assert!(
                matches!(
                    outcome,
                    ForwardOutcome::Failed {
                        problem: DeliveryProblem::ChannelUnknown,
                        ..
                    }
                ),
                "{outcome:?}"
            );
            for hidden in [SESSION, LOCAL, OTHER_LOCAL, ACCOUNT] {
                assert!(!detail_of(&outcome).contains(hidden), "{outcome:?}");
            }
            assert_eq!(intents, 0);
            assert!(lock(&seen).is_empty(), "the helper never ran");
        }
        // None: another session's conversation, and this session's archived one.
        fails(|root| {
            profile(
                root,
                ACCOUNT,
                &[(OTHER_LOCAL, OTHER_SESSION, false), (LOCAL, SESSION, true)],
            )
        });
        // Two.
        fails(|root| {
            profile(
                root,
                ACCOUNT,
                &[(LOCAL, SESSION, false), (OTHER_LOCAL, SESSION, false)],
            )
        });
        // None for the active account: the pointer is another account's.
        fails(|root| {
            let desktop = profile(root, OTHER_ACCOUNT, &[(LOCAL, SESSION, false)]);
            std::fs::write(
                desktop.join("config.json"),
                json!({"lastKnownAccountUuid": ACCOUNT}).to_string(),
            )
            .expect("config.json");
            desktop
        });
        // No profile at all.
        fails(|root| root.join("absent"));
    }

    #[test]
    fn an_account_change_during_route_fails() {
        // Between the lookup's two reads of the active account.
        let root = state_root();
        let desktop = profile(root.path(), ACCOUNT, &[(LOCAL, SESSION, false)]);
        let mut reads = 0;
        let changed = restore::route_with(&desktop, SESSION, &mut || {
            reads += 1;
            let account = if reads == 1 { ACCOUNT } else { OTHER_ACCOUNT };
            Ok(Some(account.to_owned()))
        });
        assert_eq!(
            changed.map_err(|failure| failure.problem),
            Err(DeliveryProblem::OwnerMismatch)
        );
        assert_eq!(reads, 2);

        // While the helper ran: the conversation now belongs to another account.
        let root = state_root();
        add_session(root.path());
        let desktop = profile(root.path(), ACCOUNT, &[(LOCAL, SESSION, false)]);
        let switched = desktop.clone();
        let (helper, seen) = fake_helper(loaded(), true, move || {
            write_profile(&switched, OTHER_ACCOUNT, &[(LOCAL, SESSION, false)]);
        });
        let (sender, waited) = restoring(root.path(), Some(desktop), helper);
        let (outcome, intents) = send(&sender, true, false);
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
        assert_eq!(lock(&seen).len(), 1);
        assert_eq!(*lock(&waited), Duration::ZERO);

        // The helper's own check of the account.
        let root = state_root();
        add_session(root.path());
        let (helper, _) = fake_helper(
            json!({
                "state": "not_submitted",
                "problem": "account_changed",
                "error": "Claude Desktop account changed; the session was not restored",
            }),
            false,
            || {},
        );
        let (sender, _) = restoring(root.path(), one_conversation(root.path()), helper);
        let (outcome, intents) = send(&sender, true, false);
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
    fn queued_draft_is_deferred_with_its_reason() {
        for (word, reason) in [
            ("draft_present", DeferReason::TargetAwaitingInput),
            ("input_busy", DeferReason::TargetAwaitingInput),
            ("no_window", DeferReason::IpcUnavailable),
            ("window_not_unique", DeferReason::IpcUnavailable),
        ] {
            let root = state_root();
            add_session(root.path());
            let (helper, seen) = fake_helper(
                json!({"state": "queued", "reason": word, "detail": "the helper's words"}),
                true,
                || {},
            );
            let (sender, waited) = restoring(root.path(), one_conversation(root.path()), helper);
            let (outcome, intents) = send(&sender, true, false);
            let ForwardOutcome::Deferred {
                reason: waits_for,
                detail,
            } = &outcome
            else {
                panic!("{word}: {outcome:?}");
            };
            assert_eq!(*waits_for, reason, "{word}");
            assert!(
                detail.contains(&format!("({word})")) && detail.contains("the helper's words"),
                "{detail}"
            );
            for hidden in [SESSION, LOCAL, ACCOUNT] {
                assert!(!detail.contains(hidden), "{detail}");
            }
            assert_eq!(intents, 0);
            assert_eq!(lock(&seen).len(), 1);
            assert_eq!(
                *lock(&waited),
                Duration::ZERO,
                "a queued restore does not wait"
            );
        }
    }

    #[test]
    fn loaded_then_registration_delivers_over_the_pipe() {
        let root = state_root();
        let server = TestServer::new(4096);
        add_session(root.path());
        let (helper, seen) = fake_helper(loaded(), true, registers(root.path(), &server.name));
        let (sender, waited) = restoring(root.path(), one_conversation(root.path()), helper);
        let (outcome, intents) = send(&sender, true, false);
        let ForwardOutcome::Delivered {
            proof: DeliveryProof::Written,
            detail,
        } = &outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(
            detail.contains("restored through Claude Desktop"),
            "{detail}"
        );
        assert_eq!(intents, 1);
        assert_eq!(lock(&seen).len(), 1);
        assert_eq!(
            *lock(&waited),
            Duration::ZERO,
            "the record was there at the first look"
        );
        let wire = String::from_utf8(server.read_to_end()).expect("the frame is text");
        assert!(
            wire.starts_with("{\"type\":\"auth\"") && wire.contains("发送测试"),
            "{wire}"
        );
    }

    #[test]
    fn loaded_without_registration_by_the_deadline_is_deferred() {
        let root = state_root();
        add_session(root.path());
        let (helper, seen) = fake_helper(loaded(), true, || {});
        let (sender, waited) = restoring(root.path(), one_conversation(root.path()), helper);
        let (outcome, intents) = send(&sender, true, false);
        let ForwardOutcome::Deferred {
            reason: DeferReason::TargetNotLoaded,
            detail,
        } = &outcome
        else {
            panic!("{outcome:?}");
        };
        assert!(detail.contains("(not_registered)"), "{detail}");
        assert_eq!(intents, 0, "nothing was written, so it is never unknown");
        assert_eq!(*lock(&waited), MAX_RESTORE);
        assert_eq!(lock(&seen).len(), 1, "one restore per send");
        // No session ever registered: a record that holds only the mark.
        let record = record_of(root.path());
        assert!(record.restore_unanswered_at_unix_ms.is_some());
        assert!(!record.online && record.pipe.is_none() && record.protected_token.is_none());
    }

    /// One send of [`TEXT`] as the message whose id bytes are all `id`, by a channel whose switch
    /// is on, with no quiet time.
    fn send_message(forward: &ClaudeForward, id: u8) -> (ForwardOutcome, u32) {
        let stopped = || false;
        let limits = SendLimits {
            deadline: Instant::now() + Duration::from_secs(60),
            stopped: &stopped,
            quiet: &stopped,
        };
        let message = ForwardMessage {
            id: [id; 16],
            text: TEXT.to_owned(),
        };
        let mut intents = 0;
        let outcome = forward.deliver(SESSION, true, &message, &limits, &mut || {
            intents += 1;
            Ok(())
        });
        (outcome, intents)
    }

    /// Whether `outcome` waits with the restore word `word`.
    fn waits_with(outcome: &ForwardOutcome, word: &str) -> bool {
        matches!(outcome, ForwardOutcome::Deferred { detail, .. } if detail.contains(&format!("({word})")))
    }

    fn record_of(root: &Path) -> SessionRecord {
        serde_json::from_str(&record_text(root, SESSION).expect("the record")).expect("a record")
    }

    #[test]
    fn a_retry_of_the_same_message_never_wakes_again() {
        let root = state_root();
        add_session(root.path());
        // The helper ran and found a draft: the message's one restore is used.
        let draft = json!({"state": "queued", "reason": "draft_present", "detail": "a draft"});
        let (helper, seen) = fake_helper(draft.clone(), true, || {});
        let (sender, _) = restoring(root.path(), one_conversation(root.path()), helper);
        let (first, intents) = send_message(&sender, 1);
        assert!(waits_with(&first, "draft_present"), "{first:?}");
        assert_eq!(intents, 0);
        assert_eq!(lock(&seen).len(), 1);

        // Its retry waits with no restore, also from a new forwarder, as after a restart.
        let (again, intents) = send_message(&sender, 1);
        assert!(waits_with(&again, "wake_spent"), "{again:?}");
        assert!(
            matches!(
                again,
                ForwardOutcome::Deferred {
                    reason: DeferReason::TargetOffline,
                    ..
                }
            ),
            "{again:?}"
        );
        assert_eq!(intents, 0);
        let (helper, restarted_seen) = fake_helper(draft, true, || {});
        let (restarted, _) = restoring(root.path(), one_conversation(root.path()), helper);
        let (after_restart, intents) = send_message(&restarted, 1);
        assert!(
            waits_with(&after_restart, "wake_spent"),
            "{after_restart:?}"
        );
        assert_eq!(intents, 0);
        assert!(
            lock(&restarted_seen).is_empty(),
            "no restore after the restart"
        );
        assert_eq!(lock(&seen).len(), 1, "one restore for the message");

        // Another message has its own one.
        let (other, _) = send_message(&sender, 2);
        assert!(waits_with(&other, "draft_present"), "{other:?}");
        assert_eq!(lock(&seen).len(), 2);
    }

    #[test]
    fn a_channel_whose_wake_got_no_registration_is_not_woken_again_until_it_registers() {
        let root = state_root();
        let server = TestServer::new(4096);
        add_session(root.path());
        let (helper, seen) = fake_helper(loaded(), true, || {});
        let (sender, waited) = restoring(root.path(), one_conversation(root.path()), helper);
        // The conversation came up, and no registration by the deadline: the session is marked.
        let (first, intents) = send_message(&sender, 1);
        assert!(waits_with(&first, "not_registered"), "{first:?}");
        assert_eq!(intents, 0);
        assert_eq!(*lock(&waited), MAX_RESTORE);
        assert!(
            record_of(root.path())
                .restore_unanswered_at_unix_ms
                .is_some()
        );

        // Every other message waits with no restore, also from a new forwarder after a restart.
        for id in [2, 3] {
            let (outcome, intents) = send_message(&sender, id);
            assert!(
                matches!(
                    outcome,
                    ForwardOutcome::Deferred {
                        reason: DeferReason::TargetNotLoaded,
                        ..
                    }
                ) && waits_with(&outcome, "restore_unanswered"),
                "{outcome:?}"
            );
            assert_eq!(intents, 0);
        }
        let (helper, restarted_seen) = fake_helper(loaded(), true, || {});
        let (restarted, _) = restoring(root.path(), one_conversation(root.path()), helper);
        let (outcome, _) = send_message(&restarted, 4);
        assert!(waits_with(&outcome, "restore_unanswered"), "{outcome:?}");
        assert!(
            lock(&restarted_seen).is_empty(),
            "no restore after the restart"
        );
        assert_eq!(lock(&seen).len(), 1, "one restore in all");
        assert_eq!(*lock(&waited), MAX_RESTORE, "no further wait");

        // The session registers by itself: its message goes over the inbox, with no restore.
        registers(root.path(), &server.name)();
        let (outcome, intents) = send_message(&sender, 5);
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
        assert_eq!(lock(&seen).len(), 1);
        let wire = String::from_utf8(server.read_to_end()).expect("the frame is text");
        assert!(wire.contains("发送测试"), "{wire}");
    }

    #[test]
    fn a_registration_clears_the_unanswered_mark() {
        let root = state_root();
        add_session(root.path());
        let (helper, seen) = fake_helper(loaded(), true, || {});
        let (sender, _) = restoring(root.path(), one_conversation(root.path()), helper);
        let (first, _) = send_message(&sender, 1);
        assert!(waits_with(&first, "not_registered"), "{first:?}");
        assert!(
            record_of(root.path())
                .restore_unanswered_at_unix_ms
                .is_some()
        );

        // The registration writes a new record without the mark; the session's end keeps it clear.
        registers(root.path(), r"\\.\pipe\cc-msg-00")();
        let text = record_text(root.path(), SESSION).expect("the record");
        assert!(!text.contains("restore_unanswered"), "{text}");
        let ended = hook_end(
            root.path(),
            end_input(SESSION).as_bytes(),
            &|| Ok(own_process()),
            3,
        );
        assert_eq!(ended, Ok(HookResult::Cleared));
        assert!(
            record_of(root.path())
                .restore_unanswered_at_unix_ms
                .is_none()
        );

        // So the stopped session's next message restores it again.
        let (outcome, _) = send_message(&sender, 2);
        assert!(waits_with(&outcome, "not_registered"), "{outcome:?}");
        assert_eq!(
            lock(&seen).len(),
            2,
            "the mark no longer holds the restore back"
        );
        assert!(
            record_of(root.path())
                .restore_unanswered_at_unix_ms
                .is_some()
        );
    }

    #[test]
    fn the_wake_never_sends_text() {
        let root = state_root();
        let server = TestServer::new(4096);
        add_session(root.path());
        let (helper, seen) = fake_helper(loaded(), true, registers(root.path(), &server.name));
        let (sender, _) = restoring(root.path(), one_conversation(root.path()), helper);
        let (outcome, intents) = send(&sender, true, false);
        assert!(
            matches!(outcome, ForwardOutcome::Delivered { .. }),
            "{outcome:?}"
        );
        assert_eq!(intents, 1);

        // The helper's whole input names the conversation and asks for the wake only.
        let requests = lock(&seen).clone();
        assert_eq!(requests.len(), 1);
        let request: Value = serde_json::from_slice(&requests[0]).expect("the request is JSON");
        assert_eq!(
            request,
            json!({"local_session": LOCAL, "account_id": ACCOUNT, "wake_only": true})
        );
        let request_text = String::from_utf8_lossy(&requests[0]);
        assert!(
            !request_text.contains("text") && !request_text.contains("发送测试"),
            "{request_text}"
        );
        // The message went over the inbox, and only there.
        let wire = String::from_utf8(server.read_to_end()).expect("the frame is text");
        assert!(wire.contains("发送测试"), "{wire}");

        // The ported source has no path that fills the prompt or presses a button.
        for token in [
            "SetValue",
            "Invoke",
            "DoDefaultAction",
            "\"Send\"",
            "SendKeys",
            "SendInput",
            "prepare_only",
            "[\"text\"]",
        ] {
            assert!(!SOURCE.contains(token), "the helper's source holds {token}");
        }
        assert!(SOURCE.contains("\"wake_only\""));
    }
}
