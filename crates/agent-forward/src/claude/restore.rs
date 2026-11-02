// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/bridge/claude.rs and scripts/runtime/src/bridge/user_input/claude_desktop.rs. Copyright (c) 2026 HS7097. MIT: see ../../LICENSE-codex-goal-lifecycle.txt.

//! Restoring a stopped Claude Code session through Claude Desktop before a send (Workflow #432
//! AGT-T1b; `contracts/agent-forward.md`, "Restoring a stopped session"). Alice, 2026-11-02: the
//! component's wake is ported, and only the wake. Nothing here types into a window: the message
//! still goes over the session's inbox, once the session has started and its hook has stored its
//! live record.
//!
//! The route reads Claude Desktop's profile for the one conversation of a session. The helper, a C#
//! program compiled on first use with the .NET Framework's compiler, brings that conversation up.
//! Every failure here is a value of [`ForwardOutcome`] with no write: `Deferred` when nothing could
//! be done now, `Failed` when the route or the helper says it cannot be done.

use super::{failed, session_id, to_hex};
use crate::process::{self, Bounds, Finished, RunFailure};
use crate::{DeferReason, DeliveryProblem, ForwardOutcome};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The helper's source: the component's wake path only, compiled on first use.
pub(super) const SOURCE: &str = include_str!("ClaudeDesktopWake.cs");

/// The longest a restore may take, whatever the send's deadline: the component's bound.
pub(super) const MAX_RESTORE: Duration = Duration::from_secs(25);
/// How often the live record is looked at while the restored session starts.
pub(super) const REGISTRATION_POLL: Duration = Duration::from_millis(100);
/// The longest the compiler may take: the component's bound.
const MAX_COMPILE: Duration = Duration::from_secs(30);
/// The bounds of the component's walk over the session pointers.
const MAX_POINTER_DEPTH: u8 = 8;
const MAX_POINTERS: usize = 20_000;
/// The largest pointer, and the largest `config.json`, read.
const MAX_PROFILE_FILE: u64 = 4 * 1024 * 1024;
/// The most of a helper's or the compiler's words a detail keeps, in characters.
const MAX_HELPER_TEXT: usize = 1024;
const HELPER_BOUNDS: Bounds = Bounds {
    stdout: 64 * 1024,
    stderr: 64 * 1024,
};
const COMPILER_BOUNDS: Bounds = Bounds {
    stdout: 1024 * 1024,
    stderr: 1024 * 1024,
};

/// Why a stopped session was not restored now. Nothing was sent, and the message is tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestoreWait {
    /// A quiet window or the hold covers now.
    Quiet,
    /// The window's prompt holds an unsent draft.
    DraftPresent,
    /// Another native input holds the window.
    InputBusy,
    /// Claude Desktop shows no main window.
    NoWindow,
    /// Claude Desktop shows more than one main window.
    WindowNotUnique,
    /// The conversation came up, and the session stored no live record by the deadline.
    NotRegistered,
}

impl RestoreWait {
    /// The word a send's detail names.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::DraftPresent => "draft_present",
            Self::InputBusy => "input_busy",
            Self::NoWindow => "no_window",
            Self::WindowNotUnique => "window_not_unique",
            Self::NotRegistered => "not_registered",
        }
    }

    /// The neutral reason the row records.
    pub(super) const fn reason(self) -> DeferReason {
        match self {
            Self::Quiet => DeferReason::TargetOffline,
            Self::DraftPresent | Self::InputBusy => DeferReason::TargetAwaitingInput,
            Self::NoWindow | Self::WindowNotUnique => DeferReason::IpcUnavailable,
            Self::NotRegistered => DeferReason::TargetNotLoaded,
        }
    }

    /// A word the helper answers with `queued`.
    fn from_helper(word: &str) -> Option<Self> {
        [
            Self::DraftPresent,
            Self::InputBusy,
            Self::NoWindow,
            Self::WindowNotUnique,
        ]
        .into_iter()
        .find(|wait| wait.as_str() == word)
    }
}

/// A send that waits for `wait`, with no write.
pub(super) fn waiting(wait: RestoreWait, why: &str) -> ForwardOutcome {
    ForwardOutcome::Deferred {
        reason: wait.reason(),
        detail: format!(
            "the session is stopped and the message waits ({}): {why}",
            wait.as_str()
        ),
    }
}

/// A send that ran out of time or was stopped before any write.
pub(super) fn unresponsive(detail: impl Into<String>) -> ForwardOutcome {
    ForwardOutcome::Deferred {
        reason: DeferReason::TargetUnresponsive,
        detail: detail.into(),
    }
}

/// At most [`MAX_HELPER_TEXT`] characters of a program's words.
fn cut(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    match text.char_indices().nth(MAX_HELPER_TEXT) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The one desktop conversation of a session: the active account and the conversation's
/// `local_<uuid>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Route {
    pub(super) account: String,
    pub(super) local_session: String,
}

/// Why there is no route. A send ends `Failed` with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RouteFailure {
    pub(super) problem: DeliveryProblem,
    pub(super) detail: String,
}

impl RouteFailure {
    fn outcome(self) -> ForwardOutcome {
        failed(self.problem, self.detail)
    }
}

fn unreadable(detail: impl Into<String>) -> RouteFailure {
    RouteFailure {
        problem: DeliveryProblem::ProtocolChanged,
        detail: detail.into(),
    }
}

fn not_one(found: usize, detail: &str) -> RouteFailure {
    RouteFailure {
        problem: DeliveryProblem::ChannelUnknown,
        detail: format!(
            "the session maps to {found} conversations of Claude Desktop's active account, not \
             exactly one: {detail}"
        ),
    }
}

/// A profile file, up to `limit` bytes; `None` when it does not exist.
fn read_profile_file(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, RouteFailure> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(unreadable(format!(
                "a file of Claude Desktop's profile cannot be read: {error}"
            )));
        }
    };
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            unreadable(format!(
                "a file of Claude Desktop's profile cannot be read: {error}"
            ))
        })?;
    if bytes.len() as u64 > limit {
        return Err(unreadable(
            "a file of Claude Desktop's profile exceeds four MiB",
        ));
    }
    Ok(Some(bytes))
}

/// The active account of the profile at `profile`; `None` when Claude Desktop keeps no
/// `config.json` there.
pub(super) fn read_account(profile: &Path) -> Result<Option<String>, RouteFailure> {
    let Some(bytes) = read_profile_file(&profile.join("config.json"), MAX_PROFILE_FILE)? else {
        return Ok(None);
    };
    let config: Value = serde_json::from_slice(&bytes)
        .map_err(|_| unreadable("Claude Desktop's config.json is not JSON"))?;
    let account = config
        .get("lastKnownAccountUuid")
        .and_then(Value::as_str)
        .ok_or_else(|| unreadable("Claude Desktop's config.json names no active account"))?;
    if session_id(account).is_none() {
        return Err(unreadable("Claude Desktop's active account is not a UUID"));
    }
    Ok(Some(account.to_owned()))
}

/// The `.json` files under `directory`, a bounded walk that follows no link.
fn pointers(directory: &Path, found: &mut Vec<PathBuf>, depth: u8) -> Result<(), RouteFailure> {
    if depth > MAX_POINTER_DEPTH || found.len() > MAX_POINTERS {
        return Err(unreadable(
            "Claude Desktop's session index exceeds the search bound",
        ));
    }
    let index = |error: std::io::Error| {
        unreadable(format!(
            "Claude Desktop's session index cannot be read: {error}"
        ))
    };
    for entry in fs::read_dir(directory).map_err(index)? {
        let entry = entry.map_err(index)?;
        let kind = entry.file_type().map_err(index)?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            pointers(&path, found, depth + 1)?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            found.push(path);
        }
    }
    Ok(())
}

/// The desktop conversation of the session `native_id` (lower case), from the profile at
/// `profile`.
pub(super) fn route(profile: &Path, native_id: &str) -> Result<Route, RouteFailure> {
    route_with(profile, native_id, &mut || read_account(profile))
}

/// [`route`], with the reader of the active account given: it is read before the walk and again
/// after it, and the two must agree.
pub(super) fn route_with(
    profile: &Path,
    native_id: &str,
    account: &mut dyn FnMut() -> Result<Option<String>, RouteFailure>,
) -> Result<Route, RouteFailure> {
    let Some(active) = account()? else {
        return Err(not_one(
            0,
            "Claude Desktop keeps no profile for this Windows user",
        ));
    };
    let sessions = profile.join("claude-code-sessions").join(&active);
    let mut paths = Vec::new();
    match fs::metadata(&sessions) {
        Ok(_) => pointers(&sessions, &mut paths, 0)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(unreadable(format!(
                "Claude Desktop's session index cannot be read: {error}"
            )));
        }
    }
    let mut matches = Vec::new();
    for path in paths {
        let Some(bytes) = read_profile_file(&path, MAX_PROFILE_FILE)? else {
            return Err(unreadable(
                "a session pointer of Claude Desktop went away during the lookup",
            ));
        };
        let pointer: Value = serde_json::from_slice(&bytes)
            .map_err(|_| unreadable("a session pointer of Claude Desktop is not JSON"))?;
        if pointer.get("cliSessionId").and_then(Value::as_str) != Some(native_id)
            || pointer.get("isArchived").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let local = pointer
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| unreadable("a pointer of the session names no desktop conversation"))?;
        if local.strip_prefix("local_").and_then(session_id).is_none() {
            return Err(unreadable(
                "a pointer of the session names a conversation of another shape",
            ));
        }
        if path.file_stem().and_then(|stem| stem.to_str()) != Some(local) {
            return Err(unreadable(
                "a pointer's file name differs from its conversation",
            ));
        }
        matches.push(local.to_owned());
    }
    if matches.len() != 1 {
        return Err(not_one(
            matches.len(),
            "none or several unarchived pointers name this session",
        ));
    }
    if account()?.as_deref() != Some(active.as_str()) {
        return Err(RouteFailure {
            problem: DeliveryProblem::OwnerMismatch,
            detail: "Claude Desktop's active account changed during the lookup".to_owned(),
        });
    }
    Ok(Route {
        account: active,
        local_session: matches.swap_remove(0),
    })
}

/// The helper's whole input: no text, ever.
#[derive(Serialize)]
struct WakeRequest<'a> {
    local_session: &'a str,
    account_id: &'a str,
    wake_only: bool,
}

/// Why a helper run gave no answer.
#[derive(Debug)]
pub(super) enum HelperFailure {
    /// The deadline or a stop came first. Nothing was sent, and the message is tried again.
    OutOfTime(String),
    /// The helper or its compiler could not run, or failed.
    Failed(String),
}

impl HelperFailure {
    fn outcome(self) -> ForwardOutcome {
        match self {
            Self::OutOfTime(detail) => unresponsive(detail),
            Self::Failed(detail) => failed(DeliveryProblem::HelperFailed, detail),
        }
    }
}

fn run_failure(program: &str, failure: RunFailure) -> HelperFailure {
    match failure {
        RunFailure::Spawn(error) => {
            HelperFailure::Failed(format!("{program} could not start: {error}"))
        }
        RunFailure::TimedOut => HelperFailure::OutOfTime(format!(
            "{program} did not finish by the deadline and was stopped; nothing was sent"
        )),
        RunFailure::Stopped => HelperFailure::OutOfTime(format!(
            "the Runtime stopped while {program} ran; nothing was sent"
        )),
        RunFailure::OutputTooLarge => {
            HelperFailure::Failed(format!("{program} wrote more than its bound"))
        }
        RunFailure::Failed(error) => {
            HelperFailure::Failed(format!("{program} failed to run: {error}"))
        }
    }
}

/// Runs the helper once with `request` on its stdin, by `deadline`.
pub(super) trait WakeHelper: Send + Sync {
    fn run(
        &self,
        request: Vec<u8>,
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Finished, HelperFailure>;
}

/// The helper compiled from [`SOURCE`] into `<state_root>\agent-forward\claude-input\`, named by the
/// source's SHA-256, as the component does.
pub(super) struct CompiledHelper {
    directory: PathBuf,
}

impl CompiledHelper {
    pub(super) fn new(state_root: &Path) -> Self {
        Self {
            directory: state_root.join("agent-forward").join("claude-input"),
        }
    }

    /// The helper's program, compiled first when this build's source has none yet. The compiler
    /// writes beside the program and the result is renamed into place, so a compile cut off at
    /// the deadline never leaves a program that looks finished.
    fn executable(
        &self,
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<PathBuf, HelperFailure> {
        let name = format!("input-{}", to_hex(&Sha256::digest(SOURCE.as_bytes())));
        let executable = self.directory.join(format!("{name}.exe"));
        if executable.is_file() {
            return Ok(executable);
        }
        let local = |what: &str, error: std::io::Error| {
            HelperFailure::Failed(format!("cannot {what} the helper: {error}"))
        };
        fs::create_dir_all(&self.directory).map_err(|error| local("make a folder for", error))?;
        let windows = std::env::var_os("WINDIR").ok_or_else(|| {
            HelperFailure::Failed("WINDIR is not set, so the compiler cannot be found".to_owned())
        })?;
        let framework = PathBuf::from(windows)
            .join("Microsoft.NET")
            .join("Framework64")
            .join("v4.0.30319");
        let source = format!("{name}.cs");
        fs::write(self.directory.join(&source), SOURCE).map_err(|error| local("write", error))?;
        let building = format!("{name}.build.exe");
        let mut command = Command::new(framework.join("csc.exe"));
        command
            .current_dir(&self.directory)
            .args(["/nologo", "/utf8output", "/target:exe"])
            .arg(format!("/out:{building}"))
            .arg(format!(
                "/reference:{}",
                framework.join("System.Web.Extensions.dll").display()
            ))
            .arg(source);
        let compiled = process::run(
            command,
            Vec::new(),
            &COMPILER_BOUNDS,
            deadline.min(Instant::now() + MAX_COMPILE),
            stopped,
            &Instant::now,
        )
        .map_err(|failure| run_failure("the compiler", failure))?;
        if !compiled.success {
            return Err(HelperFailure::Failed(format!(
                "the helper did not compile: {} {}",
                cut(&compiled.stdout),
                cut(&compiled.stderr)
            )));
        }
        fs::rename(self.directory.join(building), &executable)
            .map_err(|error| local("put in place", error))?;
        Ok(executable)
    }
}

impl WakeHelper for CompiledHelper {
    fn run(
        &self,
        request: Vec<u8>,
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Finished, HelperFailure> {
        let executable = self.executable(deadline, stopped)?;
        let mut command = Command::new(executable);
        command.current_dir(&self.directory);
        process::run(
            command,
            request,
            &HELPER_BOUNDS,
            deadline,
            stopped,
            &Instant::now,
        )
        .map_err(|failure| run_failure("the helper", failure))
    }
}

/// The clock of the wait for the restored session's record, so that a test needs no sleep.
pub(super) trait WaitClock: Send + Sync {
    fn now(&self) -> Instant;
    fn pause(&self, length: Duration);
}

pub(super) struct RealClock;

impl WaitClock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn pause(&self, length: Duration) {
        std::thread::sleep(length);
    }
}

/// What the helper answered.
enum Answer {
    Loaded,
    Queued { wait: RestoreWait, detail: String },
}

fn answer(finished: &Finished) -> Result<Answer, ForwardOutcome> {
    let helper_failed = |detail: String| failed(DeliveryProblem::HelperFailed, detail);
    let Ok(answer) = serde_json::from_slice::<Value>(&finished.stdout) else {
        return Err(helper_failed(format!(
            "the helper's answer is not JSON: {} {}",
            cut(&finished.stdout),
            cut(&finished.stderr)
        )));
    };
    let text = |key: &str| answer.get(key).and_then(Value::as_str);
    if !finished.success {
        let problem = if text("problem") == Some("account_changed") {
            DeliveryProblem::OwnerMismatch
        } else {
            DeliveryProblem::HelperFailed
        };
        return Err(failed(
            problem,
            format!(
                "the helper failed and nothing was sent: {}",
                cut(text("error").unwrap_or("it named no error").as_bytes())
            ),
        ));
    }
    match text("state") {
        Some("loaded") => Ok(Answer::Loaded),
        Some("queued") => match text("reason").and_then(RestoreWait::from_helper) {
            Some(wait) => Ok(Answer::Queued {
                wait,
                detail: cut(text("detail").unwrap_or_default().as_bytes()),
            }),
            None => Err(helper_failed(
                "the helper waits for a reason this build does not know".to_owned(),
            )),
        },
        _ => Err(helper_failed(
            "the helper answered a state this build does not know".to_owned(),
        )),
    }
}

/// What a restore uses: Claude Desktop's profile, the helper and the wait's clock.
pub(super) struct Restorer {
    /// `%APPDATA%\Claude`; `None` when the variable is not set.
    pub(super) profile: Option<PathBuf>,
    pub(super) helper: Box<dyn WakeHelper>,
    pub(super) clock: Box<dyn WaitClock>,
}

impl Restorer {
    pub(super) fn new(state_root: &Path) -> Self {
        Self {
            profile: std::env::var_os("APPDATA").map(|data| PathBuf::from(data).join("Claude")),
            helper: Box::new(CompiledHelper::new(state_root)),
            clock: Box::new(RealClock),
        }
    }

    /// Brings up the desktop conversation of the stopped session `native_id`, by `deadline`. `Ok`
    /// says it is displayed, with a note for the send's detail; the session's own hook stores its
    /// live record next. `Err` ends the send, with no write.
    pub(super) fn wake(
        &self,
        native_id: &str,
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> Result<String, ForwardOutcome> {
        let Some(profile) = self.profile.as_deref() else {
            return Err(failed(
                DeliveryProblem::ProtocolChanged,
                "APPDATA is not set, so Claude Desktop's profile cannot be found".to_owned(),
            ));
        };
        let selected = route(profile, native_id).map_err(RouteFailure::outcome)?;
        let request = serde_json::to_vec(&WakeRequest {
            local_session: &selected.local_session,
            account_id: &selected.account,
            wake_only: true,
        })
        .map_err(|error| {
            failed(
                DeliveryProblem::HelperFailed,
                format!("the helper's request cannot be encoded: {error}"),
            )
        })?;
        let finished = self
            .helper
            .run(request, deadline, stopped)
            .map_err(HelperFailure::outcome)?;
        match answer(&finished)? {
            Answer::Queued { wait, detail } => Err(waiting(wait, &detail)),
            Answer::Loaded => {
                if route(profile, native_id).map_err(RouteFailure::outcome)? != selected {
                    return Err(failed(
                        DeliveryProblem::OwnerMismatch,
                        "Claude Desktop's account or conversation of the session changed while it \
                         was restored"
                            .to_owned(),
                    ));
                }
                let warnings = cut(&finished.stderr);
                Ok(if warnings.is_empty() {
                    "the stopped session was restored through Claude Desktop".to_owned()
                } else {
                    format!(
                        "the stopped session was restored through Claude Desktop (the helper \
                         said: {warnings})"
                    )
                })
            }
        }
    }
}
