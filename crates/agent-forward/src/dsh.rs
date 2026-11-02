// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/dsh_notify.rs and scripts/runtime/src/bridge/dsh.rs. Copyright (c) 2026 HS7097. MIT: see ../LICENSE-codex-goal-lifecycle.txt.

//! DSH sessions as channels (Workflow #432; `contracts/agent-forward.md`): one embedded Node
//! helper, run twice for a send. `check` reads the session's durable log and signs in to the local
//! server, and submits nothing. `send` submits the prompt once, with `mode: "queue"`, and reads the
//! exact text back from the durable log under the request id.
//!
//! The helper reads the server's local address and its cookies itself and removes them from its
//! errors, so nothing secret reaches this process. It loads DSH's own modules from the app root,
//! private paths that a DSH update can move.

use crate::channels::{Channel, DshPaths};
use crate::process::{self, Bounds, Finished, RunFailure};
use crate::{
    DeferReason, DeliveryProblem, DeliveryProof, ForwardMessage, ForwardOutcome, IntentFailed,
    SendLimits,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::process::Command;
use std::time::{Duration, Instant};

/// The helper, run as `node --input-type=module -e <it>`.
const HELPER: &str = include_str!("dsh_notify.mjs");
/// The `schema` of every answer of the helper.
const HELPER_SCHEMA: &str = "dsh-native-notification/v1";
/// The most the helper may write on each stream, as in the adapted source.
const HELPER_BOUNDS: Bounds = Bounds {
    stdout: 2 * 1024 * 1024,
    stderr: 1024 * 1024,
};
/// The range of the helper's own time limit, which its input carries; the helper refuses others.
const HELPER_MIN: Duration = Duration::from_millis(1_000);
const HELPER_MAX: Duration = Duration::from_millis(120_000);
/// How much sooner than the send's deadline the helper's own limit ends, so that it answers with
/// its own result before it would be killed.
const HELPER_MARGIN: Duration = Duration::from_millis(1_000);
/// The longest piece of the helper's own words a detail carries, in characters.
const MAX_EXCERPT_CHARS: usize = 1_000;
/// The longest session id, in bytes.
const MAX_SESSION_ID_BYTES: usize = 256;
/// The labels the two request ids of a message are derived with.
const REQUEST_LABEL: &[u8] = b"actingcommand.agent-forward.dsh.request";
const RPC_LABEL: &[u8] = b"actingcommand.agent-forward.dsh.rpc";
const CHECK: &str = "check";
const SEND: &str = "send";

/// Whether `value` is one DSH session id: a letter or a digit, then letters, digits, `-` or `_`,
/// 256 bytes at most. This is the shape the helper accepts, and it names no path.
pub(crate) fn valid_session_id(value: &str) -> bool {
    value.len() <= MAX_SESSION_ID_BYTES
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The text of a UUID of version 8 made of `bytes`.
fn uuid_text(mut bytes: [u8; 16]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
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

/// The request id and the RPC id of a message: SHA-256 of the message's id and a label, as UUIDs of
/// version 8. Every try of one message has the same two, and they differ from each other.
fn request_ids(id: &[u8; 16]) -> (String, String) {
    let derive = |label: &[u8]| {
        let digest = Sha256::new()
            .chain_update(id)
            .chain_update(label)
            .finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        uuid_text(bytes)
    };
    (derive(REQUEST_LABEL), derive(RPC_LABEL))
}

/// The session a helper run addresses, as the helper reads it.
#[derive(Serialize)]
struct Target<'a> {
    app_root: &'a str,
    home_root: &'a str,
    node_executable: &'a str,
    session_id: &'a str,
}

/// A helper run's input, on its stdin.
#[derive(Serialize)]
struct HelperInput<'a> {
    mode: &'static str,
    target: &'a Target<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rpc_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    timeout_ms: u64,
}

/// How the helper runs, so that a test can run a stand-in program.
pub(crate) trait HelperSystem: Send + Sync {
    /// The command that starts the helper for a channel with these paths.
    fn command(&self, paths: &DshPaths) -> Command;

    /// The helper's answer within what it wrote on stdout: all of it, for the real helper.
    fn answer<'a>(&self, stdout: &'a [u8]) -> &'a [u8] {
        stdout
    }

    /// The time now.
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The real helper, under the channel's Node.
struct NodeHelper;

impl HelperSystem for NodeHelper {
    fn command(&self, paths: &DshPaths) -> Command {
        let mut command = Command::new(&paths.node_executable);
        // The helper loads nothing relative to its directory. The app root keeps the program out of
        // every directory of this installation.
        command
            .args(["--input-type=module", "-e", HELPER])
            .current_dir(&paths.app_root);
        command
    }
}

/// How one run of the helper ended.
enum Ran {
    /// It answered in its schema, for this session.
    Answered { success: bool, answer: Value },
    /// It could not be started, so it did nothing.
    NotStarted(String),
    /// The send's deadline came first, and it was killed.
    TimedOut,
    /// The Runtime is stopping, and it was killed.
    Stopped,
    /// It wrote more than its bound, an answer that is not its schema or names another session, or
    /// the operating system failed during the run.
    Unreadable(String),
}

/// The helper's own words for its error, its code and its message, cut short.
fn helper_error(answer: &Value) -> String {
    let word = |value: &Value| match value {
        Value::String(text) => text.clone(),
        Value::Null => "none".to_owned(),
        other => other.to_string(),
    };
    let error = &answer["error"];
    excerpt(&format!(
        "{}: {}",
        word(&error["code"]),
        word(&error["message"])
    ))
}

fn excerpt(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MAX_EXCERPT_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The helper's stderr, for a detail that has no answer to show.
fn stderr_note(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    if text.trim().is_empty() {
        String::new()
    } else {
        format!("; its error output: {}", excerpt(&text))
    }
}

fn exit_word(success: bool) -> &'static str {
    if success { "exit 0" } else { "a failing exit" }
}

/// Reads a finished helper's answer: JSON in the helper's schema, for this session.
fn read_answer(answer: &[u8], finished: &Finished, session: &str) -> Ran {
    let Ok(value) = serde_json::from_slice::<Value>(answer) else {
        return Ran::Unreadable(format!(
            "the helper's answer is not JSON ({}){}",
            exit_word(finished.success),
            stderr_note(&finished.stderr)
        ));
    };
    if value["schema"] != HELPER_SCHEMA || value["session_id"] != session {
        return Ran::Unreadable(format!(
            "the helper answered in another schema or for another session ({}){}",
            exit_word(finished.success),
            stderr_note(&finished.stderr)
        ));
    }
    Ran::Answered {
        success: finished.success,
        answer: value,
    }
}

fn failed(problem: DeliveryProblem, detail: impl Into<String>) -> ForwardOutcome {
    ForwardOutcome::Failed {
        problem,
        detail: detail.into(),
    }
}

fn unknown(problem: DeliveryProblem, detail: impl Into<String>) -> ForwardOutcome {
    ForwardOutcome::Unknown {
        problem,
        detail: detail.into(),
    }
}

fn unresponsive(detail: &str) -> ForwardOutcome {
    ForwardOutcome::Deferred {
        reason: DeferReason::TargetUnresponsive,
        detail: detail.to_owned(),
    }
}

/// What the helper's `send` answered, read in the adapted source's order: its ids first, then its
/// proof.
fn send_outcome(success: bool, answer: &Value, request_id: &str, rpc_id: &str) -> ForwardOutcome {
    if answer["request_id"] != request_id || answer["rpc_id"] != rpc_id {
        return unknown(
            DeliveryProblem::ReceiptMismatch,
            "the helper's answer names other request ids; nothing is sent again",
        );
    }
    if success
        && answer["status"] == "recorded"
        && answer["accepted"] == true
        && answer["events"]
            .as_array()
            .is_some_and(|events| !events.is_empty())
    {
        return ForwardOutcome::Delivered {
            proof: DeliveryProof::Recorded,
            detail: "the exact text is in the session's durable log under this request id; this \
                     does not show that the session's turn ran"
                .to_owned(),
        };
    }
    let status = answer["status"].as_str().unwrap_or("none");
    match status {
        "not_submitted" | "rejected" => failed(
            DeliveryProblem::Rejected,
            format!(
                "the helper did not submit the prompt ({status}, {}): {}",
                exit_word(success),
                helper_error(answer)
            ),
        ),
        _ => unknown(
            DeliveryProblem::ReceiptMismatch,
            format!(
                "the helper could not show the prompt in the durable log ({status}, {}): {}; \
                 nothing is sent again",
                exit_word(success),
                helper_error(answer)
            ),
        ),
    }
}

/// Sends to DSH sessions.
pub(crate) struct DshForward {
    system: Box<dyn HelperSystem>,
}

impl DshForward {
    pub(crate) fn new() -> Self {
        Self {
            system: Box::new(NodeHelper),
        }
    }

    /// The helper's own time limit for a run that starts now, when there is time enough for one.
    fn helper_time(&self, limits: &SendLimits<'_>) -> Option<u64> {
        let left = limits
            .deadline
            .saturating_duration_since(self.system.now())
            .checked_sub(HELPER_MARGIN)?;
        if left < HELPER_MIN {
            return None;
        }
        u64::try_from(left.min(HELPER_MAX).as_millis()).ok()
    }

    /// Runs the helper once with `input` and reads how it ended.
    fn run_helper(
        &self,
        paths: &DshPaths,
        session: &str,
        input: Vec<u8>,
        limits: &SendLimits<'_>,
    ) -> Ran {
        let now = || self.system.now();
        match process::run(
            self.system.command(paths),
            input,
            &HELPER_BOUNDS,
            limits.deadline,
            limits.stopped,
            &now,
        ) {
            Ok(finished) => read_answer(self.system.answer(&finished.stdout), &finished, session),
            Err(RunFailure::Spawn(error)) => {
                Ran::NotStarted(format!("the helper could not be started: {error}"))
            }
            Err(RunFailure::TimedOut) => Ran::TimedOut,
            Err(RunFailure::Stopped) => Ran::Stopped,
            Err(RunFailure::OutputTooLarge) => {
                Ran::Unreadable("the helper wrote more than its bound and was killed".to_owned())
            }
            Err(RunFailure::Failed(error)) => {
                Ran::Unreadable(format!("the helper's run failed: {error}"))
            }
        }
    }

    /// One message to the DSH session of `channel`: `check` writes nothing, `before_write` commits
    /// the intent, and then `send` submits the prompt once. `Recorded` proves the exact text is in
    /// the session's durable log; it does not show that the turn ran.
    pub(crate) fn deliver(
        &self,
        channel: &Channel,
        message: &ForwardMessage,
        limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        let Some(paths) = channel.dsh.as_ref() else {
            return failed(
                DeliveryProblem::ChannelUnknown,
                "the channel names no DSH paths",
            );
        };
        let session = channel.native_id.as_str();
        if !valid_session_id(session) {
            return failed(
                DeliveryProblem::ChannelUnknown,
                "the channel's native id is not one DSH session id",
            );
        }
        let target = Target {
            app_root: &paths.app_root,
            home_root: &paths.home_root,
            node_executable: &paths.node_executable,
            session_id: session,
        };

        // The check: it reads and signs in, and submits nothing.
        if (limits.stopped)() {
            return unresponsive("the Runtime is stopping; nothing was started");
        }
        let Some(timeout_ms) = self.helper_time(limits) else {
            return unresponsive(
                "too little time is left before the deadline; nothing was started",
            );
        };
        let check = HelperInput {
            mode: CHECK,
            target: &target,
            request_id: None,
            rpc_id: None,
            text: None,
            timeout_ms,
        };
        let input = match serde_json::to_vec(&check) {
            Ok(input) => input,
            Err(error) => {
                return failed(
                    DeliveryProblem::HelperFailed,
                    format!("the check's input could not be encoded: {error}"),
                );
            }
        };
        match self.run_helper(paths, session, input, limits) {
            Ran::Answered { success, answer } if success && answer["status"] == "ready" => {}
            Ran::Answered { success, answer } => {
                return ForwardOutcome::Deferred {
                    reason: DeferReason::TargetOffline,
                    detail: format!(
                        "the DSH session is not ready ({}): {}",
                        exit_word(success),
                        helper_error(&answer)
                    ),
                };
            }
            Ran::NotStarted(detail) | Ran::Unreadable(detail) => {
                return failed(DeliveryProblem::HelperFailed, detail);
            }
            Ran::TimedOut => {
                return unresponsive("the check reached the deadline and its helper was killed");
            }
            Ran::Stopped => {
                return unresponsive("the Runtime is stopping; the check's helper was killed");
            }
        }

        // The send: the one submission.
        if (limits.stopped)() {
            return unresponsive("the Runtime is stopping; nothing was submitted");
        }
        let Some(timeout_ms) = self.helper_time(limits) else {
            return unresponsive("too little time is left after the check; nothing was submitted");
        };
        let (request_id, rpc_id) = request_ids(&message.id);
        let send = HelperInput {
            mode: SEND,
            target: &target,
            request_id: Some(request_id.as_str()),
            rpc_id: Some(rpc_id.as_str()),
            text: Some(message.text.as_str()),
            timeout_ms,
        };
        let input = match serde_json::to_vec(&send) {
            Ok(input) => input,
            Err(error) => {
                return failed(
                    DeliveryProblem::HelperFailed,
                    format!("the send's input could not be encoded: {error}"),
                );
            }
        };
        if before_write().is_err() {
            return failed(
                DeliveryProblem::WriteFailed,
                "the intent could not be recorded; nothing was submitted",
            );
        }
        match self.run_helper(paths, session, input, limits) {
            Ran::Answered { success, answer } => {
                send_outcome(success, &answer, &request_id, &rpc_id)
            }
            // It never ran, so it submitted nothing.
            Ran::NotStarted(detail) => failed(DeliveryProblem::HelperFailed, detail),
            Ran::TimedOut => unknown(
                DeliveryProblem::WriteTimedOut,
                "the send reached the deadline and its helper was killed; it may have submitted",
            ),
            Ran::Stopped => unknown(
                DeliveryProblem::WriteFailed,
                "the Runtime stopped during the send and its helper was killed; it may have \
                 submitted",
            ),
            Ran::Unreadable(detail) => unknown(
                DeliveryProblem::HelperFailed,
                format!("{detail}; it may have submitted"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{ChannelClient, read_registry};
    use crate::cli::{DshFlags, add_channel};
    use actingcommand_host_metrics::{ProcessProbe, probe_process};
    use serde_json::json;
    use std::io::{ErrorKind, Read, Write};
    use std::path::PathBuf;
    use tempfile::TempDir;

    // Everything here is invented: the session id, the paths and the helper. No test starts Node,
    // reads a DSH home or reaches a DSH server. The helper is this test binary run again as
    // `fake_helper_process`, which answers from the plan its test wrote.
    const SESSION: &str = "neutral-session-0001";
    const PLAN_VARIABLE: &str = "ACTINGCOMMAND_TEST_FAKE_DSH_HELPER";
    const FAKE_HELPER_TEST: &str = "dsh::tests::fake_helper_process";
    /// The stand-in writes this before its answer, since the test harness writes its own lines
    /// first.
    const ANSWER_MARK: &[u8] = b"\n--actingcommand-fake-dsh-helper-answer--\n";
    const TEXT: &str = "ActingCommand to 频道\n发送测试 正文 C:\\neutral\\w1.json";
    const MESSAGE_ID: [u8; 16] = [7; 16];

    /// The stand-in helper. Its tests run this test binary again with `--ignored --exact` for this
    /// one function (precedent: `crates/runtime-host/src/tests/agent_sessions.rs`). Run any other
    /// way, it does nothing.
    ///
    /// It keeps its input as `input-<mode>.json`, then follows the plan's step for the mode: stay
    /// running (`stuck`, after writing `pid-<mode>`), write past a bound (`flood`), write `raw`
    /// text, or answer in the helper's schema with the input's ids and the step's `answer` members.
    #[test]
    #[ignore = "the DSH tests run it as their stand-in helper"]
    fn fake_helper_process() {
        let Some(directory) = std::env::var_os(PLAN_VARIABLE) else {
            return;
        };
        let directory = PathBuf::from(directory);
        let mut input = Vec::new();
        std::io::stdin()
            .lock()
            .read_to_end(&mut input)
            .expect("read the input");
        let input: Value = serde_json::from_slice(&input).expect("the input is JSON");
        let mode = input["mode"].as_str().expect("a mode").to_owned();
        std::fs::write(
            directory.join(format!("input-{mode}.json")),
            input.to_string(),
        )
        .expect("keep the input");
        let plan: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.join("plan.json")).expect("read the plan"),
        )
        .expect("the plan is JSON");
        let step = &plan[mode.as_str()];
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(ANSWER_MARK)
            .and_then(|()| stdout.flush())
            .expect("write the mark");
        if step["stuck"] == true {
            let ProcessProbe::Running { created_at_unix_ms } = probe_process(std::process::id())
            else {
                panic!("this process runs");
            };
            let part = directory.join(format!("pid-{mode}.part"));
            std::fs::write(
                &part,
                format!("{} {created_at_unix_ms}", std::process::id()),
            )
            .expect("write the pid");
            std::fs::rename(&part, directory.join(format!("pid-{mode}"))).expect("show the pid");
            loop {
                std::thread::park();
            }
        }
        if let Some(stream) = step["flood"].as_str() {
            let flood = vec![b'x'; 3 << 20];
            // The reader stops at its bound and the run kills this process: a failed write is
            // expected here.
            let _ = if stream == "stderr" {
                std::io::stderr().lock().write_all(&flood)
            } else {
                stdout.write_all(&flood)
            };
            std::process::exit(1);
        }
        let answer = match step["raw"].as_str() {
            Some(raw) => raw.to_owned(),
            None => {
                let mut answer = json!({
                    "schema": HELPER_SCHEMA,
                    "session_id": input["target"]["session_id"],
                });
                for name in ["request_id", "rpc_id"] {
                    if !input[name].is_null() {
                        answer[name] = input[name].clone();
                    }
                }
                for (name, value) in step["answer"].as_object().into_iter().flatten() {
                    answer[name] = value.clone();
                }
                answer.to_string()
            }
        };
        stdout
            .write_all(answer.as_bytes())
            .and_then(|()| stdout.write_all(b"\n"))
            .and_then(|()| stdout.flush())
            .expect("write the answer");
        let exit = step["exit"].as_i64().unwrap_or(0);
        std::process::exit(i32::try_from(exit).expect("an exit status"));
    }

    /// Runs the stand-in, and moves the clock to `jump`'s deadline once the stuck stand-in of
    /// `jump`'s mode has started.
    struct FakeSystem {
        directory: PathBuf,
        jump: Option<(&'static str, Instant)>,
    }

    impl HelperSystem for FakeSystem {
        fn command(&self, _paths: &DshPaths) -> Command {
            let mut command = Command::new(std::env::current_exe().expect("this test binary"));
            command
                .args(["--exact", FAKE_HELPER_TEST, "--ignored", "--quiet"])
                .env(PLAN_VARIABLE, &self.directory)
                .current_dir(&self.directory);
            command
        }

        fn answer<'a>(&self, stdout: &'a [u8]) -> &'a [u8] {
            stdout
                .windows(ANSWER_MARK.len())
                .position(|window| window == ANSWER_MARK)
                .map_or(stdout, |at| &stdout[at + ANSWER_MARK.len()..])
        }

        fn now(&self) -> Instant {
            match self.jump {
                Some((mode, deadline)) if self.directory.join(format!("pid-{mode}")).exists() => {
                    deadline
                }
                _ => Instant::now(),
            }
        }
    }

    struct Setup {
        directory: TempDir,
        forward: DshForward,
        channel: Channel,
    }

    /// A DSH channel with invented paths under a new directory, whose helper follows `plan`.
    fn with_plan(plan: Value, jump: Option<(&'static str, Instant)>) -> Setup {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join("plan.json"), plan.to_string())
            .expect("write the plan");
        let root = directory.path().to_string_lossy().into_owned();
        let channel = Channel {
            name: "频道".to_owned(),
            client: ChannelClient::Dsh,
            native_id: SESSION.to_owned(),
            dsh: Some(DshPaths {
                app_root: format!(r"{root}\app"),
                home_root: format!(r"{root}\home"),
                node_executable: format!(r"{root}\node.exe"),
            }),
        };
        let forward = DshForward {
            system: Box::new(FakeSystem {
                directory: directory.path().to_path_buf(),
                jump,
            }),
        };
        Setup {
            directory,
            forward,
            channel,
        }
    }

    /// A deadline far enough away that each run's own limit is the helper's longest.
    fn long() -> Instant {
        Instant::now() + Duration::from_secs(600)
    }

    fn never() -> bool {
        false
    }

    fn message() -> ForwardMessage {
        ForwardMessage {
            id: MESSAGE_ID,
            text: TEXT.to_owned(),
        }
    }

    /// Sends once and answers the outcome and how often the intent was asked.
    fn send(setup: &Setup, deadline: Instant, stopped: &dyn Fn() -> bool) -> (ForwardOutcome, u32) {
        let limits = SendLimits { deadline, stopped };
        let mut intents = 0;
        let outcome = setup
            .forward
            .deliver(&setup.channel, &message(), &limits, &mut || {
                intents += 1;
                Ok(())
            });
        (outcome, intents)
    }

    /// The input the stand-in got for `mode`, if it ran for it.
    fn input(setup: &Setup, mode: &str) -> Option<Value> {
        match std::fs::read_to_string(setup.directory.path().join(format!("input-{mode}.json"))) {
            Ok(text) => Some(serde_json::from_str(&text).expect("the kept input is JSON")),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => panic!("read the kept input: {error}"),
        }
    }

    fn ready() -> Value {
        json!({"answer": {"status": "ready", "last_seq": 3}})
    }

    fn events() -> Value {
        json!([{"seq": 4, "type": "user/message", "time": "2026-11-02T00:00:00.000Z"}])
    }

    #[test]
    fn check_not_ready_is_deferred_before_intent() {
        for (step, words) in [
            (
                json!({"answer": {"status": "not_submitted", "error": {"code": "SESSION",
                    "message": "The selected DSH session has no native durable log."}}, "exit": 1}),
                "SESSION: The selected DSH session",
            ),
            // An error code that is not text, as an aborted fetch gives.
            (
                json!({"answer": {"status": "not_submitted", "error": {"code": 20,
                    "message": "This operation was aborted"}}, "exit": 1}),
                "20: This operation was aborted",
            ),
            // `ready` counts only with exit 0.
            (
                json!({"answer": {"status": "ready"}, "exit": 1}),
                "a failing exit",
            ),
        ] {
            let setup = with_plan(json!({"check": step}), None);
            let (outcome, intents) = send(&setup, long(), &never);
            assert!(
                matches!(
                    &outcome,
                    ForwardOutcome::Deferred {
                        reason: DeferReason::TargetOffline,
                        detail,
                    } if detail.contains(words)
                ),
                "{outcome:?}"
            );
            assert_eq!(intents, 0);
            assert_eq!(input(&setup, SEND), None, "no send ran");

            // The check's input: the session and its paths, the helper's time limit, no message.
            let check = input(&setup, CHECK).expect("the check ran");
            let paths = setup.channel.dsh.as_ref().expect("paths");
            assert_eq!(check["mode"], CHECK);
            assert_eq!(
                check["target"],
                json!({
                    "app_root": paths.app_root,
                    "home_root": paths.home_root,
                    "node_executable": paths.node_executable,
                    "session_id": SESSION,
                })
            );
            assert_eq!(check["timeout_ms"], 120_000, "the helper's longest limit");
            for absent in ["request_id", "rpc_id", "text"] {
                assert!(check.get(absent).is_none(), "{absent}");
            }
        }
    }

    #[test]
    fn recorded_with_matching_ids_is_delivered() {
        let (request_id, rpc_id) = request_ids(&MESSAGE_ID);
        // A first submission, and a text the log already holds under this request id.
        for submission in ["submitted_once", "already_recorded"] {
            let setup = with_plan(
                json!({"check": ready(), "send": {"answer": {"status": "recorded",
                    "accepted": true, "submission": submission, "events": events()}}}),
                None,
            );
            let (outcome, intents) = send(&setup, long(), &never);
            assert!(
                matches!(
                    outcome,
                    ForwardOutcome::Delivered {
                        proof: DeliveryProof::Recorded,
                        ..
                    }
                ),
                "{submission}: {outcome:?}"
            );
            assert_eq!(intents, 1);
            let sent = input(&setup, SEND).expect("the send ran");
            assert_eq!(sent["mode"], SEND);
            assert_eq!(sent["text"], TEXT, "the text reaches the helper unchanged");
            assert_eq!(sent["target"]["session_id"], SESSION);
            assert_eq!(sent["request_id"], request_id.as_str());
            assert_eq!(sent["rpc_id"], rpc_id.as_str());
        }

        // The ids: distinct, the same for every try of a message, others for another message, and
        // UUIDs of version 8 in lower case, which the helper accepts.
        assert_ne!(request_id, rpc_id);
        assert_eq!(
            request_ids(&MESSAGE_ID),
            (request_id.clone(), rpc_id.clone())
        );
        let (other_request_id, other_rpc_id) = request_ids(&[8; 16]);
        assert_ne!(other_request_id, request_id);
        assert_ne!(other_rpc_id, rpc_id);
        for id in [&request_id, &rpc_id, &other_request_id, &other_rpc_id] {
            let bytes = id.as_bytes();
            assert_eq!(bytes.len(), 36, "{id}");
            assert!(
                bytes.iter().enumerate().all(|(at, byte)| {
                    if matches!(at, 8 | 13 | 18 | 23) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
                    }
                }),
                "{id}"
            );
            assert_eq!(bytes[14], b'8', "{id}");
            assert!(matches!(bytes[19], b'8' | b'9' | b'a' | b'b'), "{id}");
        }
    }

    #[test]
    fn not_submitted_and_rejected_fail() {
        for (answer, status) in [
            (
                json!({"status": "not_submitted", "accepted": false, "error": {"code": "AUTH",
                    "message": "DSH authentication did not establish a session (HTTP 401)."}}),
                "not_submitted",
            ),
            (
                json!({"status": "rejected", "accepted": false, "error": {"code": "REJECTED",
                    "message": "DSH rejected the prompt (busy)."}}),
                "rejected",
            ),
        ] {
            let setup = with_plan(
                json!({"check": ready(), "send": {"answer": answer, "exit": 1}}),
                None,
            );
            let (outcome, intents) = send(&setup, long(), &never);
            assert!(
                matches!(
                    &outcome,
                    ForwardOutcome::Failed {
                        problem: DeliveryProblem::Rejected,
                        detail,
                    } if detail.contains(status)
                ),
                "{outcome:?}"
            );
            assert_eq!(intents, 1);
        }

        // A refused intent: the check ran, and no send at all.
        let setup = with_plan(json!({"check": ready()}), None);
        let limits = SendLimits {
            deadline: long(),
            stopped: &never,
        };
        let outcome = setup
            .forward
            .deliver(&setup.channel, &message(), &limits, &mut || {
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
        assert!(input(&setup, CHECK).is_some(), "the check ran");
        assert_eq!(input(&setup, SEND), None, "no send ran");
    }

    #[test]
    fn everything_else_after_intent_is_unknown() {
        use crate::DeliveryProblem::{HelperFailed, ReceiptMismatch};
        for (step, problem) in [
            // The helper's own `unknown`: it submitted, then could not show the text in the log.
            (
                json!({"answer": {"status": "unknown", "accepted": true, "error": {"code":
                    "READBACK_PENDING", "message": "exact durable readback did not arrive"}},
                    "exit": 1}),
                ReceiptMismatch,
            ),
            (
                json!({"answer": {"status": "unknown", "error": {"code": "ACK_IDENTITY",
                    "message": "DSH acknowledgement has another RPC identity"}}, "exit": 1}),
                ReceiptMismatch,
            ),
            // `recorded` without its whole proof.
            (
                json!({"answer": {"status": "recorded", "accepted": true, "events": []}}),
                ReceiptMismatch,
            ),
            (
                json!({"answer": {"status": "recorded", "accepted": false, "events": events()}}),
                ReceiptMismatch,
            ),
            (
                json!({"answer": {"status": "recorded", "accepted": true, "events": events()},
                    "exit": 1}),
                ReceiptMismatch,
            ),
            // Another request's ids.
            (
                json!({"answer": {"status": "recorded", "accepted": true, "events": events(),
                    "rpc_id": "00000000-0000-8000-8000-000000000000"}}),
                ReceiptMismatch,
            ),
            // No answer of the helper's.
            (
                json!({"answer": {"status": "recorded", "accepted": true, "events": events(),
                    "schema": "neutral/v1"}}),
                HelperFailed,
            ),
            (
                json!({"answer": {"status": "recorded", "accepted": true, "events": events(),
                    "session_id": "other-session"}}),
                HelperFailed,
            ),
            (json!({"raw": "not an answer", "exit": 1}), HelperFailed),
        ] {
            let setup = with_plan(json!({"check": ready(), "send": step.clone()}), None);
            let (outcome, intents) = send(&setup, long(), &never);
            assert!(
                matches!(
                    &outcome,
                    ForwardOutcome::Unknown { problem: seen, .. } if *seen == problem
                ),
                "{step}: {outcome:?}"
            );
            assert_eq!(intents, 1);
            assert!(input(&setup, SEND).is_some(), "the send ran once");
        }
    }

    #[test]
    fn a_stuck_helper_is_killed_at_its_deadline() {
        let killed = |setup: &Setup, mode: &str| {
            let text = std::fs::read_to_string(setup.directory.path().join(format!("pid-{mode}")))
                .expect("the stuck helper started");
            let (pid, created) = text.split_once(' ').expect("a pid and a creation time");
            let pid: u32 = pid.parse().expect("a pid");
            let created: u64 = created.parse().expect("a creation time");
            assert!(
                !matches!(
                    probe_process(pid),
                    ProcessProbe::Running { created_at_unix_ms } if created_at_unix_ms == created
                ),
                "the stuck {mode} helper was killed"
            );
        };

        // Stuck in the check, before the intent: the message waits.
        let deadline = long();
        let setup = with_plan(json!({"check": {"stuck": true}}), Some((CHECK, deadline)));
        let (outcome, intents) = send(&setup, deadline, &never);
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Deferred {
                    reason: DeferReason::TargetUnresponsive,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 0);
        killed(&setup, CHECK);
        assert_eq!(input(&setup, SEND), None, "no send ran");

        // Stuck in the send, after the intent: it may have submitted.
        let deadline = long();
        let setup = with_plan(
            json!({"check": ready(), "send": {"stuck": true}}),
            Some((SEND, deadline)),
        );
        let (outcome, intents) = send(&setup, deadline, &never);
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Unknown {
                    problem: DeliveryProblem::WriteTimedOut,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 1);
        killed(&setup, SEND);

        // The Runtime stopping kills it the same way.
        let setup = with_plan(json!({"check": ready(), "send": {"stuck": true}}), None);
        let started = setup.directory.path().join(format!("pid-{SEND}"));
        let stopping = || started.exists();
        let (outcome, intents) = send(&setup, long(), &stopping);
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
        assert_eq!(intents, 1);
        killed(&setup, SEND);
    }

    #[test]
    fn helper_output_over_its_bound_is_helper_failed() {
        for stream in ["stdout", "stderr"] {
            let setup = with_plan(json!({"check": {"flood": stream}}), None);
            let (outcome, intents) = send(&setup, long(), &never);
            assert!(
                matches!(
                    outcome,
                    ForwardOutcome::Failed {
                        problem: DeliveryProblem::HelperFailed,
                        ..
                    }
                ),
                "{stream}: {outcome:?}"
            );
            assert_eq!(intents, 0);
            assert_eq!(input(&setup, SEND), None, "no send ran");
        }
        let setup = with_plan(json!({"check": ready(), "send": {"flood": "stdout"}}), None);
        let (outcome, intents) = send(&setup, long(), &never);
        assert!(
            matches!(
                outcome,
                ForwardOutcome::Unknown {
                    problem: DeliveryProblem::HelperFailed,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(intents, 1);
    }

    #[test]
    fn session_ids_outside_the_shape_are_refused() {
        let longest = "s".repeat(MAX_SESSION_ID_BYTES);
        for good in ["a", "Z", "0", SESSION, "a_b-c", longest.as_str()] {
            assert!(valid_session_id(good), "{good}");
        }
        let too_long = "s".repeat(MAX_SESSION_ID_BYTES + 1);
        for bad in [
            "",
            "..",
            "../another",
            "session/path",
            r"session\path",
            "a\nb",
            "-session",
            "_session",
            "a b",
            "a.b",
            "频道",
            too_long.as_str(),
        ] {
            assert!(!valid_session_id(bad), "{bad:?}");
        }

        // `agent-channel add` refuses such an id, and a relative path, and writes no registry.
        let root = tempfile::tempdir().expect("a temporary state root");
        let paths = |app_root: &'static str| {
            Some(DshFlags {
                app_root: Some(app_root),
                home_root: Some(r"C:\neutral\home"),
                node_executable: Some(r"C:\neutral\node.exe"),
            })
        };
        for bad in ["../another", "-session", too_long.as_str()] {
            assert!(
                add_channel(root.path(), "频道", "dsh", bad, paths(r"C:\neutral\app")).is_err(),
                "{bad}"
            );
        }
        assert!(
            add_channel(root.path(), "频道", "dsh", SESSION, paths(r"neutral\app")).is_err(),
            "a relative path"
        );
        assert_eq!(
            read_registry(root.path()),
            Ok(None),
            "no registry was written"
        );
        add_channel(
            root.path(),
            "频道",
            "dsh",
            SESSION,
            paths(r"C:\neutral\app"),
        )
        .expect("a DSH channel");
        let registry = read_registry(root.path())
            .expect("a readable registry")
            .expect("a registry");
        assert_eq!(
            registry
                .channel("频道")
                .map(|channel| channel.native_id.as_str()),
            Some(SESSION)
        );

        // A registry edited by hand with such an id: the send fails before any helper runs.
        let mut setup = with_plan(json!({"check": ready()}), None);
        setup.channel.native_id = "../another".to_owned();
        let (outcome, intents) = send(&setup, long(), &never);
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
        assert_eq!(intents, 0);
        assert_eq!(input(&setup, CHECK), None, "no helper ran");
    }
}
