// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl devices list|bind|unbind` (Workflow #398 RD-1c; model-398 v1.1 §3.3, §10.1;
//! `contracts/device-binding.md`, "Choosing a device"), the CLI original of the MCP tools
//! `ac_devices`, `ac_device_bind` and `ac_device_unbind` (`mcp::devices`). A person or an agent
//! picks the device of a handset instance from what adb discovers, and the choice goes into the
//! state root's private binding file, the one place a device serial lives.
//!
//! - `list` runs the install root's adb once (`adb devices -l`; Workflow #337, the adb of
//!   `<install root>\tools\platform-tools\`) and answers one row per device: its `handle`
//!   (`<transport>:<transport_id>`, the adb server's session number, never a device value), its
//!   `transport` (`usb` when the row has a `usb:` segment, else `tcp`), `state`, `model`, the
//!   alias the private file binds it to (`bound_alias`) and, for a loopback `tcp` row (an
//!   emulator), its `endpoint`.
//! - `bind <alias> <handle>` checks that the selected configuration has the alias as a handset,
//!   reads the list again, and writes the device of `handle` for the alias into the private
//!   file, with the time and the channel (`cli` here, `mcp` for the tool).
//! - `unbind <alias>` removes the alias's binding.
//! - #398 DB-1 (`contracts/device-binding.md`, "Choosing a device"): `bind <alias> <handle>
//!   --config <file>` checks the alias in the candidate configuration `file` instead, by the same
//!   rules and codes, and writes into the private file of the state root `file` names, the one a
//!   check-config of `file` reads, so that a new installation binds its handset before its
//!   configuration passes check-config and is switched in: no `--state-root` is taken, and no
//!   install selection is read, of the process or of a root (fix round 1): the install root's adb
//!   is that of `--root`, else of the root the program runs from by its place
//!   (`locate::root_of_place`); the answer names `config`. `--bound-via ui` writes the channel
//!   `ui` (the installer's); without it the channel is the caller's, as before.
//! - #398 DB-2 (`contracts/device-binding.md`, "Choosing a device"; register §1.467): `list` also
//!   lists for a root that has nothing selected: with neither `--state-root` nor `--config` it
//!   reads no binding, takes its adb as a bind against a candidate does (`--root`, else the root
//!   the program runs from by its place) and says so (`bindings_unread`); `list --config <file>`
//!   reads the bindings of the state root a candidate names. A row may carry `device_key`, the
//!   contract's fingerprint of the device's serial ([`Keyer`]) under the caller's salt
//!   (`--key-salt`, [`KeySalt`]), else under the installation's own secret (the selected or
//!   candidate configuration's `secret_fingerprint_salt`), else none. `bind <alias> --device-key
//!   <key>` names the device by its key in place of the handle, found in a fresh listing
//!   (`device_key_unknown` when no row carries it). Every bind refuses a row that cannot be a
//!   handset's device (`device_row_unbindable`): adb reports it in another state than `device`,
//!   or its transport is not the one the handset declares.
//! - #398 DB-2 fix round 1: a listing never refuses for want of a key. Where the selected
//!   configuration cannot be read for the installation's secret, its rows carry no `device_key`
//!   and the answer says so (`device_keys_unmade` `config_unreadable`, the reader's refusal its
//!   `cause`); `bindings_unread` carries the place's refusal as its `cause` where the Lab's row
//!   located no state root ([`dispatch_unlocated`]); a row refused for its state carries the
//!   state's word (`device_link_state`).
//!
//! - Workflow #479 F-2 (`contracts/device-binding.md`, "An iOS handset"; register §1.482): `list`
//!   lists the phones `acib devices` sees beside adb's rows, each `{device_key?, system: "ios",
//!   transport, bound_alias?}`, its key made by `acib` under the caller's salt, else the
//!   installation's own secret, by the same function; `acib` is the install root's
//!   `tools\acib.exe`, started as adb is (standard input from nowhere, pipes for its output and
//!   errors), and a salt reaches it only as a variable of that one child's environment under the
//!   Lab's fixed names ([`ACIB_KEY_SALT_VARIABLE`], [`ACIB_BINDING_SALT_VARIABLE`]), never an
//!   argument. Fix round 1: `acib` is given its own deadline ([`ACIB_TIMEOUT_MS`]) and the Lab
//!   waits for it a little longer, reading a bounded amount of its output ([`wait_acib`]); past
//!   either it ends its child. When `acib` is not installed and no iOS handset is configured the
//!   listing is as before; otherwise an iOS listing that fails is the member
//!   `ios_devices_unlisted` `{why}` (`ios_list_problem`) beside the Android rows, never the
//!   listing's failure. `bind` of an iOS
//!   handset names its phone by the listing's `device_key`, has `acib` find it and answer its key
//!   under the installation's secret, and writes the iOS row; a key or handle of the other device
//!   system is `device_row_unbindable` `system_mismatch`.
//!
//! None of them runs the Runtime or records anything in the ledger: the next start reads the
//! file and records the binding by fingerprint, so a change answers `restart_required`. No
//! answer, outcome value or developer line names a device serial: adb's own text has every
//! serial the file binds replaced by the device's label `[device:handset:<alias>]`
//! (`raw_text_redacted`, model-398 §3.5 rule 4). Nothing of this module ever holds an
//! identifier of an iOS phone: `acib` keeps it and answers only salted keys.

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, DeviceRowProblem, IosListProblem};
use actingcommand_contract::codes::{ContractCode, SanitizationCode, SharedCode};
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{
    DeviceBinding, DeviceBindings, DeviceBindingsFile, DeviceBoundVia, DeviceLinkState,
    DeviceSystem, InstalledProcess, InstanceDeviceKind, RuntimeErrorCode, SecretField,
    SecretFingerprinter, device_bindings_path, device_bindings_schema, device_key_valid,
    parse_device_bindings, read_device_bindings,
};
use actingcommand_ledger::Sha256SecretFingerprinter;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const COMMAND: &str = "devices";
pub const LIST: &str = "list";
pub const BIND: &str = "bind";
pub const UNBIND: &str = "unbind";
/// #398 DB-1: `bind`'s candidate configuration, a file, in place of `--state-root`; #398 DB-2:
/// `list`'s too.
pub const CONFIG_FLAG: &str = "--config";
/// #398 DB-1: the channel `bind` writes; absent, the caller's own.
pub const BOUND_VIA_FLAG: &str = "--bound-via";
/// #398 DB-2: the salt the caller makes the device keys with ([`KeySalt`]), as text on the command
/// line (for scripts and tests: a command line is visible to the user's other processes).
pub const KEY_SALT_FLAG: &str = "--key-salt";
/// #398 DB-2 (the coordinator's addition to ruling 2): the same salt read from the environment
/// variable this flag names, of the Lab's own process: the form for a salt no other process may
/// see (the installer's).
pub const KEY_SALT_ENV_FLAG: &str = "--key-salt-env";
/// #398 DB-2: `bind`'s device key, in place of its handle.
pub const DEVICE_KEY_FLAG: &str = "--device-key";
/// #398 DB-2: the shortest salt, in bytes: the bound actingd holds the installation's own secret
/// (`secret_fingerprint_salt`) to.
pub const MIN_KEY_SALT_BYTES: usize = 16;
/// #398 DB-2: the longest salt, in bytes, the installation's own secret's bound too.
pub const MAX_KEY_SALT_BYTES: usize = 1024;
/// #398 DB-1: the one word [`BOUND_VIA_FLAG`] takes.
const UI: &str = "ui";
/// #398 DB-1: the member of `bind`'s answer that names the candidate configuration the alias was
/// checked in.
const CONFIG_MEMBER: &str = "config";
/// #398 DB-2: the member of a row, and of a bind by key's answer, that carries the device key.
const DEVICE_KEY_MEMBER: &str = "device_key";
/// #398 DB-2: the member of a listing that read no binding, and the one word it says why with: no
/// state root is known (none was given or located, and no candidate configuration named one).
/// Fix round 1: `{why, cause?}` ([`not_done`]), the place's refusal its cause where a place was
/// asked ([`dispatch_unlocated`]).
const BINDINGS_UNREAD_MEMBER: &str = "bindings_unread";
const STATE_ROOT_UNKNOWN: &str = "state_root_unknown";
/// #398 DB-2 fix round 1: the member of a listing whose rows carry no `device_key` because the
/// selected configuration, which provides the installation's secret, cannot be read or decoded,
/// and the one word it says why with; `{why, cause}`, the reader's refusal its cause.
const DEVICE_KEYS_UNMADE_MEMBER: &str = "device_keys_unmade";
const CONFIG_UNREADABLE: &str = "config_unreadable";
/// #398 DB-2 fix round 1: the members of [`BINDINGS_UNREAD_MEMBER`] and
/// [`DEVICE_KEYS_UNMADE_MEMBER`]: the word, and the refusal that caused it as an outcome envelope.
const WHY_MEMBER: &str = "why";
const CAUSE_MEMBER: &str = "cause";
/// #398 DB-2: the configuration member that holds the installation's own secret.
const SECRET_MEMBER: &str = "secret_fingerprint_salt";
/// #398 DB-2: the state adb reports for an authorized device that is online, the one state a
/// handset's device is bound in.
const STATE_DEVICE: &str = "device";
/// The transports of a row; a handset that declares none is on USB (actingd's reading).
const USB: &str = "usb";
const TCP: &str = "tcp";
/// Workflow #479 F-2: the environment variables the Lab gives `acib` its salts under, in that one
/// child's environment alone: the salt of the rows' keys (`acib devices --key-salt-env`) and the
/// installation's own secret (`--binding-salt-env`), whose keys are the private file's.
pub const ACIB_KEY_SALT_VARIABLE: &str = "ACTINGCOMMAND_ACIB_KEY_SALT";
pub const ACIB_BINDING_SALT_VARIABLE: &str = "ACTINGCOMMAND_ACIB_BINDING_SALT";
/// Workflow #479 F-2: the member of a listing that has no iOS rows because `acib` gave none, and
/// the member of a row, and of an iOS bind's answer, that names its device system.
const IOS_DEVICES_UNLISTED_MEMBER: &str = "ios_devices_unlisted";
const SYSTEM_MEMBER: &str = "system";
/// Workflow #479 F-2: the connections `acib devices` names a phone's row by, the iOS rows'
/// `transport`.
const ACIB_CONNECTIONS: [&str; 3] = ["usb", "network", "unknown"];
const USAGE: &str = "usage: actingctl devices list [--state-root <dir> | --config <file>] [--root <install root>] [--key-salt <text> | --key-salt-env <NAME>]\n       actingctl devices bind <alias> (<handle> | --device-key <key> [--key-salt <text> | --key-salt-env <NAME>]) (--state-root <dir> | --config <file>) [--root <install root>] [--bound-via ui]\n       actingctl devices unbind <alias> --state-root <dir>";

/// One run of `adb devices -l`: what it printed, or why it printed nothing.
pub enum AdbRun {
    Printed(String),
    /// adb did not start.
    NotStarted(std::io::Error),
    /// adb ended with a failure: its exit code and its error text, verbatim.
    Failed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

/// Runs `adb devices -l` with the adb at `adb`.
pub fn run_adb(adb: &Path) -> AdbRun {
    match Command::new(adb).args(["devices", "-l"]).output() {
        Err(error) => AdbRun::NotStarted(error),
        Ok(output) if output.status.success() => {
            AdbRun::Printed(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => AdbRun::Failed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
    }
}

/// Workflow #479 F-2: one run of `acib`: its arguments and the variables of that one child's
/// environment. A salt is only ever a value of `environment`, under the Lab's own names
/// ([`ACIB_KEY_SALT_VARIABLE`], [`ACIB_BINDING_SALT_VARIABLE`]); `removed` names the variable of
/// the Lab's own process a caller's salt was read from (`--key-salt-env`), which the child does not
/// inherit. Its `Debug` shows the variables' names and none of their values.
pub struct AcibCall {
    pub arguments: Vec<String>,
    pub environment: Vec<(&'static str, String)>,
    pub removed: Vec<String>,
}

impl fmt::Debug for AcibCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcibCall")
            .field("arguments", &self.arguments)
            .field(
                "environment",
                &self
                    .environment
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>(),
            )
            .field("removed", &self.removed)
            .finish()
    }
}

/// Workflow #479 F-2: what one run of `acib` gave.
pub enum AcibRun {
    /// It ended with status 0: its standard output, one JSON line.
    Answered(Vec<u8>),
    /// It ended with another status: its standard output, its problem line.
    Refused(Vec<u8>),
    /// It did not start.
    NotStarted(std::io::Error),
    /// Workflow #479 F-2 fix round 1: it had not ended with its answer when the Lab's wait
    /// ([`ACIB_WAIT_MS`]) passed, and the Lab ended it.
    Overdue,
    /// Workflow #479 F-2 fix round 1: it printed more than [`ACIB_OUTPUT_LIMIT_BYTES`] on one of
    /// its two streams, and the Lab ended it.
    TooLong,
    /// Workflow #479 F-2 fix round 1: the Lab asked it to end (past its wait, past its cap, or
    /// after a stream or its state could not be read) and ending it failed: it may still run.
    /// Fix round 2: this cause alone.
    NotEnded,
    /// Workflow #479 F-2 fix round 2: the Lab could not read whether it had ended, so it ended it,
    /// and ending it succeeded: it runs no more, and what it answered is not known.
    StateUnread,
}

/// Workflow #479 F-2 fix round 1: the deadline the Lab gives `acib` for one call (`--timeout-ms`):
/// 10 s, well under the 30 s the installer waits for a Lab call (register §1.475), so a listing
/// whose wait for Apple's device service runs out still answers adb's rows in time.
pub const ACIB_TIMEOUT_MS: u64 = 10_000;
/// Workflow #479 F-2 fix round 1: how long the Lab waits for `acib`, a little above its own
/// deadline: a child still running then is ended (it is the Lab's own child) and the call answers
/// `no_answer_in_time`.
pub const ACIB_WAIT_MS: u64 = 12_000;
/// Workflow #479 F-2 fix round 1: the most the Lab reads of each of `acib`'s two streams (its
/// output, and its errors, which are read and dropped); a child that prints more is ended and the
/// call answers `answer_too_long`. A `devices` line of many phones is a few kilobytes.
pub const ACIB_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
/// How often the Lab looks whether `acib` ended.
const ACIB_POLL_MS: u64 = 10;

/// Workflow #479 F-2 fix round 1: the bounds of one wait for `acib` ([`wait_acib`]).
#[derive(Debug, Clone, Copy)]
pub struct AcibBounds {
    /// The longest the Lab waits for the child to end with its answer.
    pub wait: Duration,
    /// The most read of each of its two streams.
    pub output_limit: usize,
    /// The time between two looks.
    pub poll: Duration,
}

impl AcibBounds {
    /// The bounds of [`run_acib`]: [`ACIB_WAIT_MS`], [`ACIB_OUTPUT_LIMIT_BYTES`] and a look every
    /// 10 ms.
    pub const fn lab() -> Self {
        Self {
            wait: Duration::from_millis(ACIB_WAIT_MS),
            output_limit: ACIB_OUTPUT_LIMIT_BYTES,
            poll: Duration::from_millis(ACIB_POLL_MS),
        }
    }
}

/// Workflow #479 F-2 fix round 1: a started `acib` as [`wait_acib`] follows it: the Lab's own
/// child process, or a test's scripted one.
pub trait AcibChild {
    /// Whether it ended, `Some(true)` with status 0; `None` while it runs.
    fn ended(&mut self) -> std::io::Result<Option<bool>>;
    /// Ends it and waits for its end.
    fn end(&mut self) -> std::io::Result<()>;
}

impl AcibChild for Child {
    fn ended(&mut self) -> std::io::Result<Option<bool>> {
        self.try_wait()
            .map(|status| status.map(|status| status.success()))
    }

    fn end(&mut self) -> std::io::Result<()> {
        self.kill()?;
        self.wait().map(|_| ())
    }
}

/// Workflow #479 F-2 fix round 1: the clock a wait for `acib` reads: the wall clock in
/// [`run_acib`], a test's own in its tests.
pub trait AcibClock {
    /// The time since the wait began.
    fn elapsed(&self) -> Duration;
    /// Waits `span` before the next look.
    fn pause(&self, span: Duration);
}

/// The wall clock of one wait, from its start.
struct WallClock(Instant);

impl AcibClock for WallClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn pause(&self, span: Duration) {
        std::thread::sleep(span);
    }
}

/// Workflow #479 F-2: runs `acib` at `acib` as `call` says, started as adb is: standard input from
/// nowhere, its output and errors through pipes (register §1.475), and the salts in its own
/// environment alone. Fix round 1: the call carries `acib`'s own deadline ([`ACIB_TIMEOUT_MS`],
/// [`AcibSalts::call`]), and the Lab waits for it within [`AcibBounds::lab`] ([`wait_acib`]).
pub fn run_acib(acib: &Path, call: &AcibCall) -> AcibRun {
    let mut command = Command::new(acib);
    command
        .args(&call.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in &call.removed {
        command.env_remove(name);
    }
    for (name, value) in &call.environment {
        command.env(name, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return AcibRun::NotStarted(error),
    };
    let clock = WallClock(Instant::now());
    match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => wait_acib(
            &mut child,
            Box::new(stdout),
            Box::new(stderr),
            AcibBounds::lab(),
            &clock,
        ),
        // Both were asked for as pipes, so both are there; without them no line can be read.
        _ => ended_as(&mut child, AcibRun::Refused(Vec::new())),
    }
}

/// Workflow #479 F-2 fix round 1: what one reader of a stream of `acib` read to the stream's end.
enum StreamRead {
    /// Its bytes (none for the errors, which are dropped).
    Ended(Vec<u8>),
    /// More than the limit.
    TooLong,
    /// The read failed.
    Failed,
}

/// Which of `acib`'s two streams a [`StreamRead`] is of.
#[derive(Clone, Copy)]
enum AcibStream {
    Output,
    Errors,
}

/// Reads `stream` to its end, at most `limit` bytes, keeping them when `keep` is set.
fn read_bounded(mut stream: Box<dyn Read + Send>, limit: usize, keep: bool) -> StreamRead {
    let mut kept = Vec::new();
    let mut read = 0_usize;
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return StreamRead::Ended(kept),
            Ok(count) => {
                read = read.saturating_add(count);
                if read > limit {
                    return StreamRead::TooLong;
                }
                if keep {
                    kept.extend_from_slice(&buffer[..count]);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return StreamRead::Failed,
        }
    }
}

/// `child`'s run as `run` once it is ended; [`AcibRun::NotEnded`] when ending it fails.
fn ended_as(child: &mut dyn AcibChild, run: AcibRun) -> AcibRun {
    match child.end() {
        Ok(()) => run,
        Err(_) => AcibRun::NotEnded,
    }
}

/// Workflow #479 F-2 fix round 1: follows the started `acib` `child`, whose standard output and
/// errors are `output` and `errors`, within `bounds` on `clock`. Each stream is read on a thread
/// of its own, at most `bounds.output_limit` bytes (the errors read and dropped), so a child that
/// fills a pipe never blocks. The run is the child's answer once it ended and both streams
/// reached their end; a stream past the limit ends the child, [`AcibRun::TooLong`]; a child (or
/// a stream) not at its end when `bounds.wait` has passed is ended, [`AcibRun::Overdue`]; a
/// stream that cannot be read ends the child, as a line the Lab cannot read
/// ([`AcibRun::Refused`] with nothing). Fix round 2, one word each: a state that cannot be read
/// ends the child, [`AcibRun::StateUnread`]; ending the child failing, whatever asked for it, is
/// [`AcibRun::NotEnded`].
pub fn wait_acib(
    child: &mut dyn AcibChild,
    output: Box<dyn Read + Send>,
    errors: Box<dyn Read + Send>,
    bounds: AcibBounds,
    clock: &dyn AcibClock,
) -> AcibRun {
    let (sender, receiver) = mpsc::channel();
    for (stream, which) in [(output, AcibStream::Output), (errors, AcibStream::Errors)] {
        let sender = sender.clone();
        let limit = bounds.output_limit;
        let keep = matches!(which, AcibStream::Output);
        // A reader the wait no longer listens to ends with its stream (the ended child's pipe);
        // nothing joins it, and what it would send has nobody left to read it.
        let _reader = std::thread::spawn(move || {
            let read = read_bounded(stream, limit, keep);
            sender.send((which, read)).ok();
        });
    }
    drop(sender);
    let mut answer = None;
    let mut errors_read = false;
    let mut status = None;
    loop {
        while let Ok((which, read)) = receiver.try_recv() {
            match (which, read) {
                (_, StreamRead::TooLong) => return end_unless(child, status, AcibRun::TooLong),
                (_, StreamRead::Failed) => {
                    return end_unless(child, status, AcibRun::Refused(Vec::new()));
                }
                (AcibStream::Output, StreamRead::Ended(bytes)) => answer = Some(bytes),
                (AcibStream::Errors, StreamRead::Ended(_)) => errors_read = true,
            }
        }
        if status.is_none() {
            match child.ended() {
                Ok(ended) => status = ended,
                // Fix round 2: whether it ended is not known here, so the Lab ends it; ended, the
                // state that could not be read is the word, else `not_ended` (`ended_as`).
                Err(_) => return ended_as(child, AcibRun::StateUnread),
            }
        }
        if let (Some(success), true) = (status, errors_read)
            && let Some(answer) = answer.take()
        {
            return if success {
                AcibRun::Answered(answer)
            } else {
                AcibRun::Refused(answer)
            };
        }
        if clock.elapsed() >= bounds.wait {
            return end_unless(child, status, AcibRun::Overdue);
        }
        clock.pause(bounds.poll);
    }
}

/// `run` for `child`, ended first unless it already ended (`status` is some).
fn end_unless(child: &mut dyn AcibChild, status: Option<bool>, run: AcibRun) -> AcibRun {
    if status.is_some() {
        run
    } else {
        ended_as(child, run)
    }
}

/// Workflow #479 F-2: the salts of one `acib devices` call: the rows' keys under `key`, and the
/// keys under the installation's own secret beside them (`binding`); `removed`, the variable of
/// the Lab's process a caller's salt came from.
struct AcibSalts<'a> {
    key: Option<&'a [u8]>,
    binding: Option<&'a [u8]>,
    removed: Option<&'a str>,
}

impl AcibSalts<'_> {
    /// The call: `devices`, `--key-salt-env` naming the Lab's variable when there is a key salt,
    /// and `--binding-salt-env` when there is a secret too; the salts as those variables' values.
    /// Fix round 1: and `--timeout-ms` [`ACIB_TIMEOUT_MS`], `acib`'s own deadline.
    fn call(&self) -> AcibCall {
        let mut arguments = vec!["devices".to_owned()];
        let mut environment = Vec::new();
        if let Some(key) = self.key {
            arguments.push("--key-salt-env".to_owned());
            arguments.push(ACIB_KEY_SALT_VARIABLE.to_owned());
            environment.push((
                ACIB_KEY_SALT_VARIABLE,
                String::from_utf8_lossy(key).into_owned(),
            ));
            if let Some(binding) = self.binding {
                arguments.push("--binding-salt-env".to_owned());
                arguments.push(ACIB_BINDING_SALT_VARIABLE.to_owned());
                environment.push((
                    ACIB_BINDING_SALT_VARIABLE,
                    String::from_utf8_lossy(binding).into_owned(),
                ));
            }
        }
        arguments.push("--timeout-ms".to_owned());
        arguments.push(ACIB_TIMEOUT_MS.to_string());
        AcibCall {
            arguments,
            environment,
            removed: self.removed.map(str::to_owned).into_iter().collect(),
        }
    }
}

/// Workflow #479 F-2: one phone `acib devices` lists: its connection, its key under the call's key
/// salt and its key under the installation's secret, each when the call gave its salt. No
/// identifier of the phone.
struct IosListed {
    connection: String,
    device_key: Option<String>,
    binding_key: Option<String>,
}

/// Workflow #479 F-2: the line `acib devices` answers.
#[derive(Deserialize)]
struct AcibDevicesLine {
    devices: Vec<AcibDevice>,
}

#[derive(Deserialize)]
struct AcibDevice {
    connection: String,
    #[serde(default)]
    device_key: Option<String>,
    #[serde(default)]
    binding_key: Option<String>,
}

/// Workflow #479 F-2: the line `acib` answers when it refuses: its problem word.
#[derive(Deserialize)]
struct AcibProblemLine {
    problem: String,
}

/// Workflow #479 F-2: the phones `acib` at `acib` lists under `salts`, in its order; `None` when no
/// iOS phone is to be listed (`acib` is not installed and `ios_configured` is false: the listing
/// is as before). Otherwise [`ios_phones`].
fn ios_listing(
    acib: &Path,
    ios_configured: bool,
    salts: &AcibSalts<'_>,
    run: &dyn Fn(&Path, &AcibCall) -> AcibRun,
) -> Option<Result<Vec<IosListed>, IosListProblem>> {
    (ios_configured || acib.is_file()).then(|| ios_phones(acib, salts, run))
}

/// Workflow #479 F-2: the phones `acib` at `acib` lists under `salts`, in its order. `acib` not
/// installed (no file there; nothing is started), not started, refusing or answering a line this
/// does not read is its word of `ios_list_problem`.
fn ios_phones(
    acib: &Path,
    salts: &AcibSalts<'_>,
    run: &dyn Fn(&Path, &AcibCall) -> AcibRun,
) -> Result<Vec<IosListed>, IosListProblem> {
    if !acib.is_file() {
        return Err(IosListProblem::NotInstalled);
    }
    ios_rows(acib, salts, run)
}

/// Workflow #479 F-2: one `acib devices` run under `salts` and its rows; a key not of the
/// contract's form, or a connection `acib` does not name, is a line this does not read.
fn ios_rows(
    acib: &Path,
    salts: &AcibSalts<'_>,
    run: &dyn Fn(&Path, &AcibCall) -> AcibRun,
) -> Result<Vec<IosListed>, IosListProblem> {
    let line = match run(acib, &salts.call()) {
        AcibRun::NotStarted(_) => return Err(IosListProblem::NotStarted),
        // Workflow #479 F-2 fix round 1: past the Lab's wait, past its cap, or not ended; fix
        // round 2: or ended because its state could not be read.
        AcibRun::Overdue => return Err(IosListProblem::NoAnswerInTime),
        AcibRun::TooLong => return Err(IosListProblem::AnswerTooLong),
        AcibRun::NotEnded => return Err(IosListProblem::NotEnded),
        AcibRun::StateUnread => return Err(IosListProblem::StateUnreadable),
        AcibRun::Refused(stdout) => {
            return Err(serde_json::from_slice::<AcibProblemLine>(&stdout)
                .ok()
                .and_then(|refused| {
                    IosListProblem::ALL
                        .iter()
                        .copied()
                        .find(|word| word.as_str() == refused.problem)
                })
                .unwrap_or(IosListProblem::AnswerUnreadable));
        }
        AcibRun::Answered(stdout) => serde_json::from_slice::<AcibDevicesLine>(&stdout)
            .map_err(|_| IosListProblem::AnswerUnreadable)?,
    };
    let key_of = |key: Option<String>, given: bool| match (key, given) {
        (Some(key), true) if device_key_valid(&key) => Ok(Some(key)),
        (None, false) => Ok(None),
        _ => Err(IosListProblem::AnswerUnreadable),
    };
    line.devices
        .into_iter()
        .map(|device| {
            if !ACIB_CONNECTIONS.contains(&device.connection.as_str()) {
                return Err(IosListProblem::AnswerUnreadable);
            }
            Ok(IosListed {
                connection: device.connection,
                device_key: key_of(device.device_key, salts.key.is_some())?,
                binding_key: key_of(
                    device.binding_key,
                    salts.key.is_some() && salts.binding.is_some(),
                )?,
            })
        })
        .collect()
}

/// One device row of `adb devices -l`.
struct Listed {
    serial: String,
    state: String,
    usb: bool,
    transport_id: Option<String>,
    model: Option<String>,
}

impl Listed {
    fn transport(&self) -> &'static str {
        if self.usb { USB } else { TCP }
    }

    /// `<transport>:<transport_id>`; none when adb gave no transport id.
    fn handle(&self) -> Option<String> {
        self.transport_id
            .as_ref()
            .map(|id| format!("{}:{id}", self.transport()))
    }
}

/// The device rows of `adb devices -l`: after the header, one line per device, its serial, its
/// state and `key:value` words; the adb server's own `*` lines are not devices.
fn parse_listing(text: &str) -> Vec<Listed> {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with("List of devices") && !line.starts_with('*')
        })
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let serial = words.next()?.to_owned();
            let state = words.next()?.to_owned();
            let mut listed = Listed {
                serial,
                state,
                usb: false,
                transport_id: None,
                model: None,
            };
            for word in words {
                if word.starts_with("usb:") {
                    listed.usb = true;
                } else if let Some(id) = word.strip_prefix("transport_id:") {
                    listed.transport_id = Some(id.to_owned());
                } else if let Some(model) = word.strip_prefix("model:") {
                    listed.model = Some(model.to_owned());
                }
            }
            Some(listed)
        })
        .collect()
}

/// The parts of `actingd.config.json` a binding checks.
#[derive(Deserialize)]
struct ConfigFile {
    instances: Vec<ConfigInstance>,
}

#[derive(Deserialize)]
struct ConfigInstance {
    alias: String,
    #[serde(default)]
    device: Option<ConfigDevice>,
}

#[derive(Deserialize)]
struct ConfigDevice {
    kind: InstanceDeviceKind,
    /// #398 DB-2: a handset's transport; absent, [`USB`], as actingd reads it.
    #[serde(default)]
    transport: Option<String>,
    /// Workflow #479 F-2: a handset's device system; absent, Android, as actingd reads it.
    #[serde(default)]
    system: Option<String>,
}

impl ConfigInstance {
    /// Workflow #479 F-2: whether the entry is an iOS handset (`device.kind` `handset`,
    /// `device.system` `ios`); actingd refuses any other use of `system`.
    fn ios_handset(&self) -> bool {
        self.device.as_ref().is_some_and(|device| {
            device.kind == InstanceDeviceKind::Handset
                && device.system.as_deref() == Some(DeviceSystem::Ios.as_str())
        })
    }
}

/// #398 DB-2: a salt the caller gives for the device keys (`--key-salt <text>`, or
/// `--key-salt-env <NAME>`, the variable of the Lab's own process): [`MIN_KEY_SALT_BYTES`] to
/// [`MAX_KEY_SALT_BYTES`] bytes of text, the bounds of the installation's own secret. The
/// installer makes a random one for each run and gives it to the listing and to the bind. It is
/// never printed, logged or written: its `Debug` shows none of it, and no answer, outcome value
/// or developer line carries it. Workflow #479 F-2: it remembers the variable it was read from,
/// which `acib`'s environment does not inherit (the salt reaches `acib` under the Lab's own name).
pub struct KeySalt {
    bytes: Vec<u8>,
    variable: Option<String>,
}

impl KeySalt {
    /// The salt `text`; one outside the bounds is `actingctl_usage_invalid` naming
    /// [`KEY_SALT_FLAG`], `out_of_range` with the bounds (never the text).
    pub fn new(text: &str) -> Result<Self, Outcome> {
        Self::given(text, KEY_SALT_FLAG)
    }

    /// The salt `text`, given by `flag`; one outside the bounds is refused naming `flag`.
    fn given(text: &str, flag: &str) -> Result<Self, Outcome> {
        if !(MIN_KEY_SALT_BYTES..=MAX_KEY_SALT_BYTES).contains(&text.len()) {
            return Err(
                cli_line::usage_invalid(Some(flag), ArgumentProblem::OutOfRange)
                    .range_min(MIN_KEY_SALT_BYTES)
                    .range_max(MAX_KEY_SALT_BYTES),
            );
        }
        Ok(Self {
            bytes: text.as_bytes().to_vec(),
            variable: None,
        })
    }

    /// #398 DB-2: the salt of the environment variable `name` of this process, as `environment`
    /// reads it (`std::env::var_os` in [`dispatch`]). A name that no variable can have (empty, or
    /// holding `=` or NUL) is `actingctl_usage_invalid` [`KEY_SALT_ENV_FLAG`] `not_recognised`; a
    /// variable that is unset, empty or not UTF-8 `unusable`; one outside the bounds
    /// `out_of_range`. No refusal carries the value; the flag names the variable.
    fn from_environment(
        name: &str,
        environment: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<Self, Outcome> {
        let refused = |problem| cli_line::usage_invalid(Some(KEY_SALT_ENV_FLAG), problem);
        if name.is_empty() || name.contains(['=', '\0']) {
            return Err(refused(ArgumentProblem::NotRecognised));
        }
        let value = environment(name).ok_or_else(|| refused(ArgumentProblem::Unusable))?;
        let text = value
            .to_str()
            .filter(|text| !text.is_empty())
            .ok_or_else(|| refused(ArgumentProblem::Unusable))?;
        let mut salt = Self::given(text, KEY_SALT_ENV_FLAG)?;
        salt.variable = Some(name.to_owned());
        Ok(salt)
    }
}

impl fmt::Debug for KeySalt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeySalt(<redacted>)")
    }
}

/// #398 DB-2 (`contracts/device-binding.md`, "Identity, label and fingerprint"): what a row's
/// device key is made with. The key is the contract's fingerprint of the device's serial, made by
/// the one function the ledger fingerprints a handset's serial with (`Sha256SecretFingerprinter`,
/// the field `device_serial`), keyed by the caller's salt or by the installation's own secret:
/// `sha256:` and 64 lowercase hexadecimal digits, the same at every listing while its key is the
/// same, never the serial. Under the installation's secret it is the fingerprint the ledger
/// records for the device (`runtime.instance_bound`'s `device.fingerprint`).
struct Keyer(Sha256SecretFingerprinter);

impl Keyer {
    /// The keys of `salt`, a salt or a secret of [`MIN_KEY_SALT_BYTES`] bytes at least, so never
    /// empty, the one thing the fingerprinter refuses; were it refused, `fingerprinter_failed`.
    fn new(salt: &[u8]) -> Result<Self, Outcome> {
        Sha256SecretFingerprinter::new(salt)
            .map(Self)
            .map_err(|_| Outcome::new(SanitizationCode::FingerprinterFailed))
    }

    /// The device key of `serial`; a fingerprint the contract refuses is its refusal's code.
    fn device_key(&self, serial: &str) -> Result<String, Outcome> {
        self.0
            .fingerprint(SecretField::DeviceSerial, serial)
            .map(|fingerprint| fingerprint.as_str().to_owned())
            .map_err(|error| Outcome::new(error.code()))
    }
}

/// #398 DB-2: the installation's own secret the configuration `document` provides: its
/// `secret_fingerprint_salt` when that is text of [`MIN_KEY_SALT_BYTES`] to
/// [`MAX_KEY_SALT_BYTES`] bytes, the bounds actingd holds it to; none otherwise (a configuration
/// without one, or with one actingd refuses, provides no secret).
fn secret_of(document: &Value) -> Option<Vec<u8>> {
    document
        .get(SECRET_MEMBER)
        .and_then(Value::as_str)
        .filter(|secret| (MIN_KEY_SALT_BYTES..=MAX_KEY_SALT_BYTES).contains(&secret.len()))
        .map(|secret| secret.as_bytes().to_vec())
}

/// #398 DB-2: the secret of the selected configuration `config`, read as [`configured_instances`]
/// reads it (`config_read_failed`, `config_decode_failed`, each with its `path`). Workflow #479
/// F-2: and whether it configures an iOS handset (a configuration whose instances do not decode
/// configures none here).
fn installation_secret(
    installation: Option<&InstalledProcess>,
    config: &Path,
) -> Result<(Option<Vec<u8>>, bool), Outcome> {
    let bytes = configuration_bytes(installation, config)?;
    let document =
        serde_json::from_slice::<Value>(&bytes).map_err(|error| decode_failed(config, &error))?;
    Ok((secret_of(&document), ios_configured(&document)))
}

/// Workflow #479 F-2: whether the configuration `document` has an iOS handset.
fn ios_configured(document: &Value) -> bool {
    ConfigFile::deserialize(document)
        .is_ok_and(|file| file.instances.iter().any(ConfigInstance::ios_handset))
}

/// The install root's programs a `devices` call runs: its adb and (Workflow #479 F-2) its acib.
struct Tools {
    adb: PathBuf,
    acib: PathBuf,
}

/// The tools of the install root `install_root`: `<install root>\tools\platform-tools\adb.exe`
/// and `<install root>\tools\acib.exe`.
fn tools_of(install_root: &Path) -> Tools {
    let tools = install_root.join("tools");
    Tools {
        adb: tools.join("platform-tools").join("adb.exe"),
        acib: tools.join("acib.exe"),
    }
}

/// The selected configuration of the install root and its tools, found as `actingctl packs` finds
/// them (command table D5): this process's install selection, else `root`'s, else the install
/// root this executable runs from. Its adb is `<install root>\tools\platform-tools\adb.exe`, its
/// acib `<install root>\tools\acib.exe`.
fn install(root: Option<&Path>) -> Result<(Option<InstalledProcess>, PathBuf, Tools), Outcome> {
    let (installation, config) =
        crate::packs::selected_config(root).map_err(|error| error.outcome())?;
    let install_root = installation
        .as_ref()
        .map(|installation| installation.root().to_path_buf())
        .or_else(|| root.map(Path::to_path_buf))
        .or_else(crate::packs::executable_install_root)
        .ok_or_else(|| Outcome::new(ActingctlCode::InstallRootUnresolved))?;
    Ok((installation, config, tools_of(&install_root)))
}

/// #398 DB-1 fix round 1: the adb a bind against a candidate configuration lists the devices
/// with: that of `root` (`--root`) when it is given, else that of the install root the program
/// at `program` runs from by its place alone (`locate::root_of_place`). No install selection is
/// read, of this process (one it inherited from another installation included) or of a root, so
/// a root whose files are laid and whose configuration is not selected yet binds, from a slot too.
/// Neither root: `install_root_unresolved`. Workflow #479 F-2: and the acib of that root.
fn candidate_tools(root: Option<&Path>, program: Option<&Path>) -> Result<Tools, Outcome> {
    root.map(Path::to_path_buf)
        .or_else(|| program.and_then(crate::locate::root_of_place))
        .map(|install_root| tools_of(&install_root))
        .ok_or_else(|| Outcome::new(ActingctlCode::InstallRootUnresolved))
}

/// The private binding file of `state_root`; one that breaks its rules is
/// `device_bindings_invalid` with the file's `path` and its `device_binding_problem`.
fn bindings(state_root: &Path) -> Result<DeviceBindings, Outcome> {
    read_device_bindings(state_root).map_err(|problem| {
        Outcome::new(ActingctlCode::DeviceBindingsInvalid)
            .path(device_bindings_path(state_root))
            .device_binding_problem(problem)
    })
}

/// adb's text with every serial `bindings` binds replaced by its device's label, and whether one
/// was (model-398 §3.5 rule 4).
fn redacted(text: &str, bindings: &DeviceBindings) -> (String, bool) {
    let mut text = text.to_owned();
    let mut replaced = false;
    // Workflow #479 F-2: an iOS row holds no serial, so adb's text cannot name it.
    for (serial, alias) in bindings
        .iter()
        .filter_map(|binding| binding.serial().map(|serial| (serial, &binding.alias)))
    {
        if text.contains(serial) {
            text = text.replace(serial, &format!("[device:handset:{alias}]"));
            replaced = true;
        }
    }
    (text, replaced)
}

/// The device rows adb lists now; a run that gives none is `device_list_failed` with the adb
/// (`adb_path`), caused by the OS's failure to start it or by adb's own (`foreign_adb_error`,
/// its exit code and its error text with the bound serials replaced).
fn listing(
    adb: &Path,
    bindings: &DeviceBindings,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Vec<Listed>, Outcome> {
    let failed = Outcome::new(ActingctlCode::DeviceListFailed).adb_path(adb);
    match run(adb) {
        AdbRun::Printed(text) => Ok(parse_listing(&text)),
        AdbRun::NotStarted(error) => {
            Err(failed.caused_by(Outcome::from_io_error(&error, IoOp::Spawn).path(adb)))
        }
        AdbRun::Failed { exit_code, stderr } => {
            let (text, replaced) = redacted(&stderr, bindings);
            let mut cause = Outcome::new(ContractCode::ForeignAdbError)
                .raw_source(RawSource::Adb)
                .raw_text(text);
            if replaced {
                cause = cause.raw_text_redacted(true);
            }
            if let Some(exit_code) = exit_code {
                cause = cause.exit_code(i64::from(exit_code));
            }
            Err(failed.caused_by(cause))
        }
    }
}

/// `{devices: [{handle?, device_key?, transport, state, model?, bound_alias?, endpoint?}]}`: every
/// device adb lists now, in its order, with the alias the private file of `state_root` binds it
/// to. No row names a serial; a loopback `tcp` row (an emulator) names its `endpoint`. #398 DB-2:
/// each row carries its `device_key` when the selected configuration provides the installation's
/// own secret ([`list_from`], the state root given); fix round 1: one that cannot be read provides
/// none, and the answer says so (`device_keys_unmade`).
pub fn list(
    state_root: &Path,
    root: Option<&Path>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    let against = Against::StateRoot(state_root.to_path_buf());
    let request = ListRequest {
        against: &against,
        root,
        key_salt: None,
        unlocated: None,
    };
    list_from(&request, None, run)
}

/// #398 DB-2: one listing: whose private binding file it reads (`against`), the install root a
/// call names (`--root`) and the caller's salt (`--key-salt`). Fix round 1: and, where a place was
/// asked for the state root and located none, its refusal (`unlocated`), the cause of
/// `bindings_unread`.
pub struct ListRequest<'a> {
    pub against: &'a Against,
    pub root: Option<&'a Path>,
    pub key_salt: Option<&'a KeySalt>,
    pub unlocated: Option<&'a Outcome>,
}

/// #398 DB-2 (`contracts/device-binding.md`, "Choosing a device"): [`list`] for the program at
/// `program`, whose place names the install root where neither the state root nor `root` does.
/// - Against a state root: its bindings, and the adb of the install root found as before
///   ([`install`]: the install selection, else `root`, else the root the executable runs from);
///   the device keys under the salt, else under the selected configuration's secret (read as
///   `bind` reads it). Fix round 1: a configuration that cannot be read or decoded provides no
///   secret, so the rows carry no key and the answer says so, the reader's refusal
///   (`config_read_failed`, `config_decode_failed`) its cause (`device_keys_unmade`
///   `config_unreadable`); the listing is not refused for it.
/// - Against a candidate configuration: the bindings of the state root it names, the adb of
///   `root`, else of the root the program runs from by its place ([`candidate_tools`]; no install
///   selection is read), the keys under the salt, else under the candidate's secret.
/// - Against neither: no binding is read, the adb as against a candidate, the keys under the salt
///   alone; the answer says that the bindings were not read, and why (`bindings_unread`
///   `state_root_unknown`), so that no row's missing `bound_alias` reads as "nothing is bound";
///   fix round 1: the request's `unlocated` its cause.
pub fn list_from(
    request: &ListRequest<'_>,
    program: Option<&Path>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    list_with_acib(request, program, run, &run_acib)
}

/// Workflow #479 F-2 (`contracts/device-binding.md`, "An iOS handset"): [`list_from`] with `acib`
/// for its run of `acib devices` ([`run_acib`] in [`list_from`]; a test's scripted `acib`). After
/// adb's rows come the phones `acib` lists, each `{device_key?, system: "ios", transport,
/// bound_alias?}`: its key under the caller's salt, else under the installation's own secret (the
/// rule of adb's rows), and the alias of the iOS row of the private file whose key is the phone's
/// key under that secret. A salt reaches `acib` only as a variable of its environment
/// ([`AcibSalts`]). When `acib` is not installed and the configuration this listing reads has no
/// iOS handset nothing is asked and the answer is as before; otherwise a listing `acib` did not
/// give is the member `ios_devices_unlisted` `{why}` beside adb's rows, and the listing answers.
pub fn list_with_acib(
    request: &ListRequest<'_>,
    program: Option<&Path>,
    run: &dyn Fn(&Path) -> AdbRun,
    acib: &dyn Fn(&Path, &AcibCall) -> AcibRun,
) -> Result<Value, Outcome> {
    let salt = request.key_salt.map(|salt| salt.bytes.as_slice());
    let (bindings, tools, secret, unmade, ios_configured) = match request.against {
        Against::StateRoot(state_root) => {
            let bindings = bindings(state_root)?;
            let (installation, config, tools) = install(request.root)?;
            // Workflow #479 F-2: the secret is read beside a caller's salt too, for the iOS rows'
            // keys under it (the binding keys); its refusal is said only where the rows' keys
            // needed it, as before.
            let (secret, unmade, ios_configured) =
                match (salt, installation_secret(installation.as_ref(), &config)) {
                    (_, Ok((secret, ios))) => (secret, None, ios),
                    (Some(_), Err(_)) => (None, None, false),
                    (None, Err(refused)) => (None, Some(refused), false),
                };
            (Some(bindings), tools, secret, unmade, ios_configured)
        }
        Against::Candidate(file) => {
            let tools = candidate_tools(request.root, program)?;
            let candidate = read_candidate(file)?;
            let ios_configured = candidate.instances.iter().any(ConfigInstance::ios_handset);
            (
                Some(bindings(&candidate.state_root)?),
                tools,
                candidate.secret,
                None,
                ios_configured,
            )
        }
        Against::Unnamed => (
            None,
            candidate_tools(request.root, program)?,
            None,
            None,
            false,
        ),
    };
    let key_salt = salt.or(secret.as_deref());
    let keyer = key_salt.map(Keyer::new).transpose()?;
    let unread = DeviceBindings::default();
    let listed = listing(&tools.adb, bindings.as_ref().unwrap_or(&unread), run)?;
    let detail = cli_line::detail().unwrap_or(Detail::Full);
    let mut answer = Map::new();
    let mut devices = rows(&listed, bindings.as_ref(), keyer.as_ref())?;
    let salts = AcibSalts {
        key: key_salt,
        binding: secret.as_deref(),
        removed: request.key_salt.and_then(|salt| salt.variable.as_deref()),
    };
    let unlisted = match ios_listing(&tools.acib, ios_configured, &salts, acib) {
        None => None,
        Some(Ok(phones)) => {
            devices.extend(ios_device_rows(&phones, bindings.as_ref()));
            None
        }
        Some(Err(problem)) => Some(problem),
    };
    answer.insert("devices".to_owned(), Value::Array(devices));
    if bindings.is_none() {
        answer.insert(
            BINDINGS_UNREAD_MEMBER.to_owned(),
            not_done(STATE_ROOT_UNKNOWN, request.unlocated, detail),
        );
    }
    if let Some(refused) = &unmade {
        answer.insert(
            DEVICE_KEYS_UNMADE_MEMBER.to_owned(),
            not_done(CONFIG_UNREADABLE, Some(refused), detail),
        );
    }
    if let Some(problem) = unlisted {
        answer.insert(
            IOS_DEVICES_UNLISTED_MEMBER.to_owned(),
            not_done(problem.as_str(), None, detail),
        );
    }
    Ok(Value::Object(answer))
}

/// Workflow #479 F-2: the rows of the phones `acib` listed, in its order: each with its key under
/// the listing's salt when it was given one, `system` `ios`, its connection as its `transport`,
/// and the alias of the private file's iOS row whose key is its key under the installation's
/// secret. No identifier of a phone: `acib` answered none.
fn ios_device_rows(phones: &[IosListed], bindings: Option<&DeviceBindings>) -> Vec<Value> {
    phones
        .iter()
        .map(|phone| {
            let mut row = Map::new();
            if let Some(device_key) = &phone.device_key {
                row.insert(DEVICE_KEY_MEMBER.to_owned(), json!(device_key));
            }
            row.insert(SYSTEM_MEMBER.to_owned(), json!(DeviceSystem::Ios.as_str()));
            row.insert("transport".to_owned(), json!(phone.connection));
            if let Some(binding) = phone.binding_key.as_deref().and_then(|binding_key| {
                bindings.and_then(|bindings| {
                    bindings
                        .iter()
                        .find(|binding| binding.device_key() == Some(binding_key))
                })
            }) {
                row.insert("bound_alias".to_owned(), json!(binding.alias));
            }
            Value::Object(row)
        })
        .collect()
}

/// #398 DB-2 fix round 1: what a listing did not do, as its member says it: `{why, cause?}`, the
/// word `why` and, when a refusal kept it from it, that refusal as an outcome envelope in `detail`
/// (`cause`). No serial: the refusals a listing keeps name paths and codes.
fn not_done(why: &str, cause: Option<&Outcome>, detail: Detail) -> Value {
    let mut member = Map::new();
    member.insert(WHY_MEMBER.to_owned(), json!(why));
    if let Some(cause) = cause {
        member.insert(
            CAUSE_MEMBER.to_owned(),
            cli_line::outcome_value(cause, detail),
        );
    }
    Value::Object(member)
}

/// The rows of `listed`, in adb's order: each with its handle, its device key when `keyer` makes
/// them, its transport, state and model, the alias `bindings` binds it to and, for a loopback
/// `tcp` row (an emulator), its `endpoint`.
fn rows(
    listed: &[Listed],
    bindings: Option<&DeviceBindings>,
    keyer: Option<&Keyer>,
) -> Result<Vec<Value>, Outcome> {
    listed
        .iter()
        .map(|listed| -> Result<Value, Outcome> {
            let mut row = Map::new();
            if let Some(handle) = listed.handle() {
                row.insert("handle".to_owned(), json!(handle));
            }
            if let Some(keyer) = keyer {
                row.insert(
                    DEVICE_KEY_MEMBER.to_owned(),
                    json!(keyer.device_key(&listed.serial)?),
                );
            }
            row.insert("transport".to_owned(), json!(listed.transport()));
            row.insert("state".to_owned(), json!(listed.state));
            if let Some(model) = &listed.model {
                row.insert("model".to_owned(), json!(model));
            }
            if let Some(binding) = bindings.and_then(|bindings| {
                bindings
                    .iter()
                    .find(|binding| binding.serial() == Some(listed.serial.as_str()))
            }) {
                row.insert("bound_alias".to_owned(), json!(binding.alias));
            }
            let loopback = listed
                .serial
                .parse::<SocketAddr>()
                .is_ok_and(|address| address.ip().is_loopback());
            if !listed.usb && loopback {
                row.insert("endpoint".to_owned(), json!(listed.serial));
            }
            Ok(Value::Object(row))
        })
        .collect()
}

/// One binding request: the handset's alias and the handle of the device `list` showed.
pub struct Bind<'a> {
    pub alias: &'a str,
    pub handle: &'a str,
    pub via: DeviceBoundVia,
}

impl<'a> Bind<'a> {
    /// #398 DB-2: the request [`bound`] writes.
    fn by_handle(&self) -> Request<'a> {
        Request {
            alias: self.alias,
            chosen: Chosen::Handle(self.handle),
            via: self.via,
        }
    }
}

/// #398 DB-2: how a bind names its device.
#[derive(Clone, Copy)]
enum Chosen<'a> {
    /// The row of this handle.
    Handle(&'a str),
    /// The row of this device key, made with the salt (absent: the installation's secret).
    Key {
        device_key: &'a str,
        salt: Option<&'a KeySalt>,
    },
}

/// #398 DB-2: one binding [`bound`] writes: the alias, how its device is named, the channel.
struct Request<'a> {
    alias: &'a str,
    chosen: Chosen<'a>,
    via: DeviceBoundVia,
}

/// #398 DB-2: how [`bound`] finds the device in its listing. Workflow #479 F-2: a key keeps the
/// salt it was made with, which `acib` is asked under when no adb row carries the key.
enum Finder<'a> {
    Handle(&'a str),
    Key {
        device_key: &'a str,
        keyer: Keyer,
        key_salt: Vec<u8>,
    },
}

/// Binds the device of `handle` to the handset `alias`: `{alias, handle, model?,
/// restart_required: true}`. An alias the selected configuration does not have is
/// `instance_unknown`; one that is not a handset `device_kind_unsupported`; a handle adb does not
/// list now `device_handle_unknown`; a binding the file's rules refuse (the device already bound
/// to another alias) `device_bindings_invalid`; a file that cannot be written
/// `device_bindings_write_failed`.
/// #398 DB-2: a row that cannot be a handset's device is `device_row_unbindable` ([`bindable`]).
pub fn bind(
    state_root: &Path,
    root: Option<&Path>,
    request: &Bind<'_>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    let against = Against::StateRoot(state_root.to_path_buf());
    bind_against(&against, root, None, &request.by_handle(), run, &run_acib)
}

/// #398 DB-2 (`contracts/device-binding.md`, "Choosing a device"): the binding `request` against
/// the state root (its configuration the selected one, its adb found by [`install`]), against the
/// candidate configuration (its adb by [`candidate_tools`] for the program at `program`, the answer
/// naming the file as `config`), or against neither (`--state-root` `required`). [`bind`],
/// [`bind_candidate_from`] and [`dispatch_in`] call it. A device named by its key is found in a
/// fresh listing by the key under the request's salt, else under the installation's own secret
/// (the selected configuration's, read as `bind` reads it, or the candidate's); neither is
/// `actingctl_usage_invalid` [`KEY_SALT_FLAG`] `required`, no row with the key
/// `device_key_unknown`. The binding written is the one a bind by the row's handle writes; the
/// answer by key is `{alias, device_key, handle?, model?, restart_required: true}` and, against a
/// candidate, `config`. Workflow #479 F-2: `acib` runs `acib devices` ([`run_acib`] but in a
/// test); an iOS handset is bound by [`bound_ios`].
fn bind_against(
    against: &Against,
    root: Option<&Path>,
    program: Option<&Path>,
    request: &Request<'_>,
    run: &dyn Fn(&Path) -> AdbRun,
    acib: &dyn Fn(&Path, &AcibCall) -> AcibRun,
) -> Result<Value, Outcome> {
    match against {
        Against::StateRoot(state_root) => {
            let (installation, config, tools) = install(root)?;
            let instances = configured_instances(installation.as_ref(), &config)?;
            let secret =
                || installation_secret(installation.as_ref(), &config).map(|(secret, _)| secret);
            let devices = Devices {
                tools: &tools,
                run,
                acib,
            };
            bound(state_root, &devices, &instances, request, &secret).map(Value::Object)
        }
        Against::Candidate(file) => {
            let tools = candidate_tools(root, program)?;
            let candidate = read_candidate(file)?;
            let secret = || -> Result<Option<Vec<u8>>, Outcome> { Ok(candidate.secret.clone()) };
            let devices = Devices {
                tools: &tools,
                run,
                acib,
            };
            let mut answer = bound(
                &candidate.state_root,
                &devices,
                &candidate.instances,
                request,
                &secret,
            )?;
            answer.insert(
                CONFIG_MEMBER.to_owned(),
                Value::String(file.to_string_lossy().into_owned()),
            );
            Ok(Value::Object(answer))
        }
        Against::Unnamed => Err(cli_line::usage_invalid(
            Some("--state-root"),
            ArgumentProblem::Required,
        )),
    }
}

/// #398 DB-1 (`contracts/device-binding.md`, "Choosing a device"): [`bind`] against the candidate
/// configuration `file` in place of the selected one. The alias is checked in `file` by the same
/// rules and codes, and the binding goes into the private file of the state root `file` names,
/// which a check-config of `file` reads; no install selection is read, and the install root is
/// asked for its adb alone ([`candidate_tools`]: `root`, else the root this program runs from by its
/// place). A file that cannot be read is `config_read_failed`, one that does not decode
/// `config_decode_failed`, each with its `path`; one without an absolute `state_root` is
/// `install_state_root_unresolved` with `config_path`. The answer is [`bind`]'s and `config`, the
/// file as given.
pub fn bind_candidate(
    file: &Path,
    root: Option<&Path>,
    request: &Bind<'_>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    let program = std::env::current_exe().and_then(fs::canonicalize).ok();
    bind_candidate_from(file, root, program.as_deref(), request, run)
}

/// #398 DB-1 fix round 1: [`bind_candidate`] for the program at `program` (this process's own,
/// canonical, in [`bind_candidate`]), whose place names the install root when `root` is not
/// given, so that a test binds as a program in a given place does.
pub fn bind_candidate_from(
    file: &Path,
    root: Option<&Path>,
    program: Option<&Path>,
    request: &Bind<'_>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    let against = Against::Candidate(file.to_path_buf());
    bind_against(
        &against,
        root,
        program,
        &request.by_handle(),
        run,
        &run_acib,
    )
}

/// Workflow #479 F-2: where a bind lists the devices: the install root's tools and the runs of its
/// adb and its acib.
struct Devices<'a> {
    tools: &'a Tools,
    run: &'a dyn Fn(&Path) -> AdbRun,
    acib: &'a dyn Fn(&Path, &AcibCall) -> AcibRun,
}

/// The binding of `request` into the private file of `state_root`: its alias checked among
/// `instances` (the configuration's), the device read from adb at `adb`, by its handle or by its
/// key (#398 DB-2: under the request's salt, else under `secret`, the installation's, read only
/// for a bind by key), and refused before anything is written when it cannot be the handset's
/// ([`bindable`]). The answer's members. Workflow #479 F-2: an iOS handset is bound by
/// [`bound_ios`]; a key no adb row carries that `acib` lists under the same salt is an iOS phone's,
/// `device_row_unbindable` `system_mismatch` ([`unknown_key`]).
fn bound(
    state_root: &Path,
    devices: &Devices<'_>,
    instances: &[ConfigInstance],
    request: &Request<'_>,
    secret: &dyn Fn() -> Result<Option<Vec<u8>>, Outcome>,
) -> Result<Map<String, Value>, Outcome> {
    let instance = instances
        .iter()
        .find(|instance| instance.alias == request.alias)
        .ok_or_else(|| {
            Outcome::new(RuntimeErrorCode::InstanceUnknown).instance_selector(request.alias)
        })?;
    let declared = match &instance.device {
        Some(device) if device.kind == InstanceDeviceKind::Handset => {
            device.transport.as_deref().unwrap_or(USB)
        }
        _ => return Err(Outcome::new(ActingctlCode::DeviceKindUnsupported).alias(request.alias)),
    };
    if instance.ios_handset() {
        return bound_ios(state_root, devices, request, secret);
    }
    let finder = match request.chosen {
        Chosen::Handle(handle) => Finder::Handle(handle),
        Chosen::Key { device_key, salt } => {
            let (keyer, key_salt) = key_maker(salt, secret)?;
            Finder::Key {
                device_key,
                keyer,
                key_salt,
            }
        }
    };
    let bindings = bindings(state_root)?;
    let rows = listing(&devices.tools.adb, &bindings, devices.run)?;
    let listed = match &finder {
        Finder::Handle(handle) => rows
            .into_iter()
            .find(|listed| listed.handle().as_deref() == Some(*handle))
            .ok_or_else(|| Outcome::new(ActingctlCode::DeviceHandleUnknown).handle(*handle))?,
        Finder::Key {
            device_key,
            keyer,
            key_salt,
        } => match keyed(rows, keyer, device_key)? {
            Some(listed) => listed,
            None => {
                let removed = match request.chosen {
                    Chosen::Key { salt, .. } => salt.and_then(|salt| salt.variable.as_deref()),
                    Chosen::Handle(_) => None,
                };
                return Err(unknown_key(
                    devices,
                    key_salt,
                    removed,
                    device_key,
                    request.alias,
                ));
            }
        },
    };
    bindable(&listed, declared, request.alias)?;
    let mut kept = bindings
        .iter()
        .filter(|binding| binding.alias != request.alias)
        .cloned()
        .collect::<Vec<_>>();
    kept.push(DeviceBinding::new(
        request.alias,
        listed.serial.clone(),
        now_unix_ms()?,
        request.via,
    ));
    write(state_root, kept)?;
    let mut answer = Map::new();
    answer.insert("alias".to_owned(), json!(request.alias));
    match request.chosen {
        Chosen::Handle(handle) => {
            answer.insert("handle".to_owned(), json!(handle));
        }
        Chosen::Key { device_key, .. } => {
            answer.insert(DEVICE_KEY_MEMBER.to_owned(), json!(device_key));
            if let Some(handle) = listed.handle() {
                answer.insert("handle".to_owned(), json!(handle));
            }
        }
    }
    if let Some(model) = &listed.model {
        answer.insert("model".to_owned(), json!(model));
    }
    answer.insert("restart_required".to_owned(), json!(true));
    Ok(answer)
}

/// Workflow #479 F-2: the refusal of a bind of an Android handset by `device_key`, which no adb
/// row carries under `key_salt`: when the install root's `acib` lists a phone with that key under
/// the same salt, the key is an iOS phone's, `device_row_unbindable` `system_mismatch` (no handle:
/// a phone has none); otherwise, `acib` not installed or not answering included,
/// `device_key_unknown` as before.
fn unknown_key(
    devices: &Devices<'_>,
    key_salt: &[u8],
    removed: Option<&str>,
    device_key: &str,
    alias: &str,
) -> Outcome {
    let salts = AcibSalts {
        key: Some(key_salt),
        binding: None,
        removed,
    };
    let ios = devices.tools.acib.is_file()
        && ios_rows(&devices.tools.acib, &salts, devices.acib).is_ok_and(|phones| {
            phones
                .iter()
                .any(|phone| phone.device_key.as_deref() == Some(device_key))
        });
    if ios {
        Outcome::new(ActingctlCode::DeviceRowUnbindable)
            .alias(alias)
            .device_row_problem(DeviceRowProblem::SystemMismatch)
    } else {
        Outcome::new(ActingctlCode::DeviceKeyUnknown)
    }
}

/// Workflow #479 F-2 (`contracts/device-binding.md`, "An iOS handset"): the binding of the iOS
/// handset `request.alias` into the private file of `state_root`. Its phone is named by the
/// listing's `device_key`, under the request's salt, else under the installation's own secret;
/// `acib` lists the phones under that salt and under the secret, and the row written holds the
/// phone's key under the secret (what the Runtime records as its fingerprint), never an
/// identifier. Before anything is written: a handle is adb's, an Android device's
/// (`device_row_unbindable` `system_mismatch` when adb lists it, `device_handle_unknown`
/// otherwise); a
/// configuration without the secret is `device_binding_secret_missing`; a listing `acib` did not
/// give is `device_ios_list_failed` with its `ios_list_problem`; a key no phone carries is an
/// Android device's when adb lists one with it under the same salt (`system_mismatch`), else
/// `device_key_unknown`; a phone bound to another alias is `device_bindings_invalid`
/// `duplicate_serial`, as an Android device is. The answer is `{alias, device_key, system: "ios",
/// restart_required: true}`.
fn bound_ios(
    state_root: &Path,
    devices: &Devices<'_>,
    request: &Request<'_>,
    secret: &dyn Fn() -> Result<Option<Vec<u8>>, Outcome>,
) -> Result<Map<String, Value>, Outcome> {
    let bindings = bindings(state_root)?;
    let mismatch = |handle: Option<String>| {
        let refused = Outcome::new(ActingctlCode::DeviceRowUnbindable)
            .alias(request.alias)
            .device_row_problem(DeviceRowProblem::SystemMismatch);
        match handle {
            Some(handle) => refused.handle(handle),
            None => refused,
        }
    };
    let (device_key, salt) = match request.chosen {
        // A handle is adb's: an Android device's, never an iOS phone's.
        Chosen::Handle(handle) => {
            let listed = listing(&devices.tools.adb, &bindings, devices.run)?
                .into_iter()
                .find(|listed| listed.handle().as_deref() == Some(handle))
                .ok_or_else(|| Outcome::new(ActingctlCode::DeviceHandleUnknown).handle(handle))?;
            return Err(mismatch(listed.handle()));
        }
        Chosen::Key { device_key, salt } => (device_key, salt),
    };
    let secret = secret()?.ok_or_else(|| {
        Outcome::new(ActingctlCode::DeviceBindingSecretMissing).alias(request.alias)
    })?;
    let key_salt = salt.map_or_else(|| secret.clone(), |salt| salt.bytes.clone());
    let salts = AcibSalts {
        key: Some(&key_salt),
        binding: Some(&secret),
        removed: salt.and_then(|salt| salt.variable.as_deref()),
    };
    let list_failed = |problem: IosListProblem| {
        Outcome::new(ActingctlCode::DeviceIosListFailed)
            .alias(request.alias)
            .ios_list_problem(problem)
    };
    let phones = ios_phones(&devices.tools.acib, &salts, devices.acib).map_err(list_failed)?;
    let Some(phone) = phones
        .into_iter()
        .find(|phone| phone.device_key.as_deref() == Some(device_key))
    else {
        // A key of an Android device under the same salt is the other system's.
        let android = Keyer::new(&key_salt).ok().and_then(|keyer| {
            listing(&devices.tools.adb, &bindings, devices.run)
                .ok()
                .and_then(|rows| keyed(rows, &keyer, device_key).ok().flatten())
        });
        return Err(match android {
            Some(listed) => mismatch(listed.handle()),
            None => Outcome::new(ActingctlCode::DeviceKeyUnknown),
        });
    };
    let binding_key = phone
        .binding_key
        .ok_or_else(|| list_failed(IosListProblem::AnswerUnreadable))?;
    let mut kept = bindings
        .iter()
        .filter(|binding| binding.alias != request.alias)
        .cloned()
        .collect::<Vec<_>>();
    kept.push(DeviceBinding::ios(
        request.alias,
        binding_key,
        now_unix_ms()?,
        request.via,
    ));
    write(state_root, kept)?;
    let mut answer = Map::new();
    answer.insert("alias".to_owned(), json!(request.alias));
    answer.insert(DEVICE_KEY_MEMBER.to_owned(), json!(device_key));
    answer.insert(SYSTEM_MEMBER.to_owned(), json!(DeviceSystem::Ios.as_str()));
    answer.insert("restart_required".to_owned(), json!(true));
    Ok(answer)
}

/// #398 DB-2: the keyer a bind by key finds its device with: the caller's `salt`, else the
/// installation's own `secret`; neither is `actingctl_usage_invalid` [`KEY_SALT_FLAG`]
/// `required`, since no row could carry a key. Workflow #479 F-2: with the salt it was made with.
fn key_maker(
    salt: Option<&KeySalt>,
    secret: &dyn Fn() -> Result<Option<Vec<u8>>, Outcome>,
) -> Result<(Keyer, Vec<u8>), Outcome> {
    if let Some(salt) = salt {
        let keyer = Keyer::new(&salt.bytes)?;
        return Ok((keyer, salt.bytes.clone()));
    }
    match secret()? {
        Some(secret) => {
            let keyer = Keyer::new(&secret)?;
            Ok((keyer, secret))
        }
        None => Err(cli_line::usage_invalid(
            Some(KEY_SALT_FLAG),
            ArgumentProblem::Required,
        )),
    }
}

/// #398 DB-2: the row of `rows` whose device key under `keyer` is `device_key`, if any.
fn keyed(rows: Vec<Listed>, keyer: &Keyer, device_key: &str) -> Result<Option<Listed>, Outcome> {
    for listed in rows {
        if keyer.device_key(&listed.serial)? == device_key {
            return Ok(Some(listed));
        }
    }
    Ok(None)
}

/// #398 DB-2 (`contracts/device-binding.md`, "Choosing a device"): whether the row `listed` can be
/// the device of the handset `alias`, whose transport is `declared`: adb must report it
/// [`STATE_DEVICE`], the state of an authorized device online (not `unauthorized`, `offline`, `no
/// permissions`, `authorizing`, `recovery` or another), and its transport must be the declared
/// one (a handset on USB is not bound to a `tcp` row, an emulator's loopback row among them).
/// Otherwise `device_row_unbindable` with the alias, the row's handle when it has one, and
/// `device_row_problem` (`state_not_device`, `transport_mismatch`), before anything is written.
/// Fix round 1: `state_not_device` with the row's state as a word of `device_link_state`
/// ([`link_state`]), never the row's own text.
fn bindable(listed: &Listed, declared: &str, alias: &str) -> Result<(), Outcome> {
    let unbindable = || Outcome::new(ActingctlCode::DeviceRowUnbindable).alias(alias);
    let refused = if listed.state != STATE_DEVICE {
        unbindable()
            .device_row_problem(DeviceRowProblem::StateNotDevice)
            .device_link_state(link_state(&listed.state))
    } else if listed.transport() != declared {
        unbindable().device_row_problem(DeviceRowProblem::TransportMismatch)
    } else {
        return Ok(());
    };
    Err(match listed.handle() {
        Some(handle) => refused.handle(handle),
        None => refused,
    })
}

/// #398 DB-2 fix round 1: the word of `device_link_state` for a row's adb state, the row's second
/// word in `adb devices -l`: `device`, `offline` and `unauthorized` as adb writes them; `no`, the
/// first word of adb's `no permissions` (the listing's words are separated by white space), is
/// `no_permissions`; any other state `other`. The Runtime's link watch reads the whole state `adb
/// track-devices` writes (`actingcommand_device::link_state_of_adb_state`, of a crate this one
/// does not build on), so this reads the listing's form of the same states.
fn link_state(state: &str) -> DeviceLinkState {
    match state {
        STATE_DEVICE => DeviceLinkState::Device,
        "offline" => DeviceLinkState::Offline,
        "unauthorized" => DeviceLinkState::Unauthorized,
        "no" => DeviceLinkState::NoPermissions,
        _ => DeviceLinkState::Other,
    }
}

/// Removes the binding of `alias`: `{alias, unbound, restart_required}`, both true when the file
/// had one and false when it had none (nothing is written then).
pub fn unbind(state_root: &Path, alias: &str) -> Result<Value, Outcome> {
    let bindings = bindings(state_root)?;
    let unbound = bindings.get(alias).is_some();
    if unbound {
        write(
            state_root,
            bindings
                .iter()
                .filter(|binding| binding.alias != alias)
                .cloned()
                .collect(),
        )?;
    }
    Ok(json!({"alias": alias, "unbound": unbound, "restart_required": unbound}))
}

/// The configured instances of the selected configuration, read as `actingctl packs` reads it:
/// one that cannot be read is `config_read_failed`, one that does not decode
/// `config_decode_failed`, each with its `path`.
fn configured_instances(
    installation: Option<&InstalledProcess>,
    config: &Path,
) -> Result<Vec<ConfigInstance>, Outcome> {
    let bytes = configuration_bytes(installation, config)?;
    serde_json::from_slice::<ConfigFile>(&bytes)
        .map(|file| file.instances)
        .map_err(|error| decode_failed(config, &error))
}

/// The bytes of the configuration `config`: through its install selection when it is the
/// selection's, else read from disk; `config_read_failed` with its `path` when they cannot be read.
fn configuration_bytes(
    installation: Option<&InstalledProcess>,
    config: &Path,
) -> Result<Vec<u8>, Outcome> {
    match installation {
        Some(selected) => selected.config_bytes().map_err(|error| {
            Outcome::new(SharedCode::ConfigReadFailed)
                .path(config)
                .caused_by(Outcome::new(error.code()))
        }),
        None => fs::read(config).map_err(|error| {
            Outcome::new(SharedCode::ConfigReadFailed)
                .path(config)
                .caused_by(Outcome::from_io_error(&error, IoOp::Read))
        }),
    }
}

/// `config_decode_failed` for the configuration `config`, caused by the decoder's refusal.
fn decode_failed(config: &Path, error: &serde_json::Error) -> Outcome {
    Outcome::new(SharedCode::ConfigDecodeFailed)
        .path(config)
        .caused_by(
            Outcome::new(ContractCode::ForeignDataError)
                .raw_source(RawSource::Json)
                .raw_text(error.to_string()),
        )
}

/// #398 DB-1: a candidate configuration's instances and the state root it names; #398 DB-2: and
/// the installation's own secret it provides ([`secret_of`]).
struct Candidate {
    instances: Vec<ConfigInstance>,
    state_root: PathBuf,
    secret: Option<Vec<u8>>,
}

/// #398 DB-1: the candidate configuration `file`, read as the configuration of a root without a
/// selection is read ([`configured_instances`]: `config_read_failed`, `config_decode_failed`), and
/// the absolute state root it names, by the rule the Lab locates a state root by
/// (`locate::state_root_of`: `install_state_root_unresolved` with `config_path`).
fn read_candidate(file: &Path) -> Result<Candidate, Outcome> {
    let bytes = configuration_bytes(None, file)?;
    let document =
        serde_json::from_slice::<Value>(&bytes).map_err(|error| decode_failed(file, &error))?;
    let instances = ConfigFile::deserialize(&document)
        .map_err(|error| decode_failed(file, &error))?
        .instances;
    let state_root = crate::locate::state_root_of(&document, file)?;
    Ok(Candidate {
        instances,
        state_root,
        secret: secret_of(&document),
    })
}

/// Writes `bindings` as the private file of `state_root`, checked by the file's own rules first
/// (one alias and one device each); the file is replaced whole through a temporary sibling.
fn write(state_root: &Path, bindings: Vec<DeviceBinding>) -> Result<(), Outcome> {
    let path = device_bindings_path(state_root);
    let write_failed = |error: &std::io::Error, op: IoOp, at: &Path| {
        Outcome::new(ActingctlCode::DeviceBindingsWriteFailed)
            .path(&path)
            .caused_by(Outcome::from_io_error(error, op).path(at))
    };
    // Workflow #479 F-2: version 2 when an iOS row is among them, version 1 otherwise, so a file of
    // Android rows alone is written as before.
    let file = DeviceBindingsFile {
        schema: device_bindings_schema(&bindings).to_owned(),
        bindings,
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|error| {
        Outcome::new(ActingctlCode::DeviceBindingsWriteFailed)
            .path(&path)
            .caused_by(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(error.to_string()),
            )
    })?;
    parse_device_bindings(&bytes).map_err(|problem| {
        Outcome::new(ActingctlCode::DeviceBindingsInvalid)
            .path(&path)
            .device_binding_problem(problem)
    })?;
    let directory = path.parent().unwrap_or(state_root);
    fs::create_dir_all(directory)
        .map_err(|error| write_failed(&error, IoOp::CreateDir, directory))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, &bytes).map_err(|error| write_failed(&error, IoOp::Write, &temporary))?;
    fs::rename(&temporary, &path).map_err(|error| write_failed(&error, IoOp::Rename, &path))
}

fn now_unix_ms() -> Result<u64, Outcome> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| Outcome::new(ActingctlCode::ClockUnavailable))
}

/// The flags and words of one `devices` call.
struct Parsed {
    action: &'static str,
    alias: Option<String>,
    handle: Option<String>,
    /// #398 DB-2: `bind --device-key <key>`, in place of the handle.
    device_key: Option<String>,
    /// #398 DB-2: the caller's salt, `--key-salt` or `--key-salt-env`.
    key_salt: Option<KeySalt>,
    against: Against,
    root: Option<PathBuf>,
    /// #398 DB-1: `bind --bound-via ui`; absent, the command's own channel, `cli`.
    bound_via: Option<DeviceBoundVia>,
}

/// Where a `devices` call reads and writes the private binding file.
pub enum Against {
    /// The state root `--state-root` names; `bind` checks its alias in the selected configuration.
    StateRoot(PathBuf),
    /// #398 DB-1: `bind --config <file>`: the candidate configuration, which names its own state
    /// root; #398 DB-2: `list --config <file>` too.
    Candidate(PathBuf),
    /// #398 DB-2: neither: `list` reads no binding; `bind` and `unbind` refuse it
    /// (`--state-root` `required`).
    Unnamed,
}

/// Workflow #434 LAB-5B1 (test builds and, since Workflow #434 LW-1, the feature `test-hooks`, for
/// `core_ops::handed_refusal`): [`parse`]'s refusal of the words [`dispatch`] is given
/// (its command's word first), if any. #398 DB-2: in no environment (nothing is read): a
/// `--key-salt-env` names an unset variable here.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn parse_refusal(arguments: &[OsString]) -> Option<Outcome> {
    parse(&arguments[1..], &|_: &str| -> Option<OsString> { None }).err()
}

/// `devices <list|bind <alias> <handle>|unbind <alias>>` and the flags `flags_of` names for the
/// action, each at most once and with a value; any other word, a repeated flag or a missing value
/// is `actingctl_usage_invalid` naming it (`argument`, `argument_problem`; Workflow #382
/// A1-FLAG). #398 DB-1: `bind` takes `--config <file>` in place of `--state-root` (both:
/// `--state-root` `conflicting`; neither: `--state-root` `required`) and `--bound-via ui` (another
/// word: `not_recognised`). #398 DB-2: `list` takes `--config` too, and neither it nor
/// `--state-root` (it then reads no binding); `bind` takes `--device-key <key>` in place of its
/// handle (neither: `handle` `required`; both: `--device-key` `conflicting`); both take the salt
/// as `--key-salt <text>` or `--key-salt-env <NAME>`, read from `environment` ([`KeySalt`]; both
/// forms: `--key-salt-env` `conflicting`; on `bind` without `--device-key` the salt's flag
/// `conflicting`).
fn parse(
    arguments: &[OsString],
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Parsed, Outcome> {
    let usage = |argument: &str, problem: ArgumentProblem| {
        Outcome::new(ActingctlCode::ActingctlUsageInvalid)
            .argument(argument)
            .argument_problem(problem)
    };
    let word = |index: usize, name: &str| -> Result<String, Outcome> {
        let word = arguments
            .get(index)
            .ok_or_else(|| usage(name, ArgumentProblem::Required))?
            .to_str()
            .ok_or_else(|| usage(name, ArgumentProblem::NotUtf8))?;
        if word.starts_with("--") || word.trim().is_empty() {
            return Err(usage(name, ArgumentProblem::Required));
        }
        Ok(word.to_owned())
    };
    let action = match word(0, "action")?.as_str() {
        LIST => LIST,
        BIND => BIND,
        UNBIND => UNBIND,
        other => return Err(usage(other, ArgumentProblem::Unknown)),
    };
    let (alias, handle, mut index) = match action {
        // #398 DB-2: the handle is the word after the alias when one stands there: a device key
        // may take its place.
        BIND => {
            let alias = word(1, "alias")?;
            match arguments.get(2) {
                Some(given) => {
                    let given = given
                        .to_str()
                        .ok_or_else(|| usage("handle", ArgumentProblem::NotUtf8))?;
                    if given.starts_with("--") {
                        (Some(alias), None, 2)
                    } else if given.trim().is_empty() {
                        return Err(usage("handle", ArgumentProblem::Required));
                    } else {
                        (Some(alias), Some(given.to_owned()), 3)
                    }
                }
                None => (Some(alias), None, 2),
            }
        }
        UNBIND => (Some(word(1, "alias")?), None, 2),
        _ => (None, None, 1),
    };
    let takes = crate::flags_of(COMMAND, Some(action)).unwrap_or_default();
    let mut state_root = None;
    let mut root = None;
    let mut config = None;
    let mut bound_via = None;
    let mut device_key = None;
    // #398 DB-2: the salt and the flag that gave it.
    let mut key_salt: Option<(KeySalt, &str)> = None;
    while index < arguments.len() {
        let text = arguments[index]
            .to_str()
            .ok_or_else(|| usage("flag", ArgumentProblem::NotUtf8))?;
        let Some(flag) = takes.iter().copied().find(|flag| *flag == text) else {
            return Err(usage(text, ArgumentProblem::Unknown));
        };
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| usage(flag, ArgumentProblem::ValueMissing))?;
        index += 2;
        // #398 DB-1: the channel, whose one word is `ui`.
        if flag == BOUND_VIA_FLAG {
            if value.to_str() != Some(UI) {
                return Err(usage(flag, ArgumentProblem::NotRecognised));
            }
            if bound_via.replace(DeviceBoundVia::Ui).is_some() {
                return Err(usage(flag, ArgumentProblem::Repeated));
            }
            continue;
        }
        // #398 DB-2: the salt, as text or from a variable of this process; one form, once.
        if flag == KEY_SALT_FLAG || flag == KEY_SALT_ENV_FLAG {
            if let Some((_, first)) = &key_salt {
                return Err(if *first == flag {
                    usage(flag, ArgumentProblem::Repeated)
                } else {
                    usage(KEY_SALT_ENV_FLAG, ArgumentProblem::Conflicting)
                });
            }
            let text = value
                .to_str()
                .ok_or_else(|| usage(flag, ArgumentProblem::NotUtf8))?;
            let salt = if flag == KEY_SALT_FLAG {
                KeySalt::new(text)?
            } else {
                KeySalt::from_environment(text, environment)?
            };
            key_salt = Some((salt, flag));
            continue;
        }
        // #398 DB-2: the device key, in place of the handle.
        if flag == DEVICE_KEY_FLAG {
            let text = value
                .to_str()
                .ok_or_else(|| usage(flag, ArgumentProblem::NotUtf8))?;
            if device_key.replace(text.to_owned()).is_some() {
                return Err(usage(flag, ArgumentProblem::Repeated));
            }
            continue;
        }
        let slot = if flag == "--root" {
            &mut root
        } else if flag == CONFIG_FLAG {
            &mut config
        } else {
            &mut state_root
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(usage(flag, ArgumentProblem::Repeated));
        }
    }
    // #398 DB-2: `bind` names its device by its handle or by its key, never both; a salt goes
    // with the key.
    if action == BIND {
        match (&handle, &device_key) {
            (None, None) => return Err(usage("handle", ArgumentProblem::Required)),
            (Some(_), Some(_)) => {
                return Err(usage(DEVICE_KEY_FLAG, ArgumentProblem::Conflicting));
            }
            _ => {}
        }
        if let (Some((_, flag)), None) = (&key_salt, &device_key) {
            return Err(usage(flag, ArgumentProblem::Conflicting));
        }
    }
    // #398 DB-1: the state root `--state-root` names, or the candidate configuration, which names
    // its own; never both. #398 DB-2: a listing may name neither.
    let against = match (state_root, config) {
        (Some(_), Some(_)) => return Err(usage("--state-root", ArgumentProblem::Conflicting)),
        (Some(state_root), None) => Against::StateRoot(state_root),
        (None, Some(config)) => Against::Candidate(config),
        (None, None) if action == LIST => Against::Unnamed,
        (None, None) => return Err(usage("--state-root", ArgumentProblem::Required)),
    };
    Ok(Parsed {
        action,
        alias,
        handle,
        device_key,
        key_salt: key_salt.map(|(salt, _)| salt),
        against,
        root,
        bound_via,
    })
}

/// Runs `devices …` when the first argument names it; `None` leaves every other command to the
/// CLI. One result line (Workflow #378 A5): the answer with `device_list_reported` or
/// `device_binding_changed`, or `{outcome}` with the refusal and exit 1.
pub fn dispatch(arguments: &[OsString]) -> Option<cli_line::Exit> {
    dispatch_with(arguments, &run_adb)
}

/// #398 DB-1: [`dispatch`] with `run` for its run of `adb devices -l` ([`run_adb`] in
/// [`dispatch`]; a test's own listing), so that a test reads what a command line binds.
pub fn dispatch_with(
    arguments: &[OsString],
    run: &dyn Fn(&Path) -> AdbRun,
) -> Option<cli_line::Exit> {
    dispatch_in(arguments, run, &|name: &str| std::env::var_os(name))
}

/// #398 DB-2: [`dispatch_with`] in the environment `environment` (this process's own,
/// `std::env::var_os`, in [`dispatch_with`]), which `--key-salt-env` reads its variable from, so
/// that a test gives a variable without touching the environment of the process that runs it.
pub fn dispatch_in(
    arguments: &[OsString],
    run: &dyn Fn(&Path) -> AdbRun,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Option<cli_line::Exit> {
    dispatch_answer(arguments, run, &run_acib, environment, None)
}

/// Workflow #479 F-2: [`dispatch_in`] with `acib` for its run of `acib devices` ([`run_acib`] in
/// [`dispatch_in`]; a test's scripted `acib`, which sees the call's arguments and environment).
pub fn dispatch_in_with_acib(
    arguments: &[OsString],
    run: &dyn Fn(&Path) -> AdbRun,
    acib: &dyn Fn(&Path, &AcibCall) -> AcibRun,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Option<cli_line::Exit> {
    dispatch_answer(arguments, run, acib, environment, None)
}

/// #398 DB-2 fix round 1: [`dispatch`] of the words the Lab's row `devices` gives where its place
/// located no state root (`core_ops::unlocated_words`): `located` is the place's refusal, which the
/// answer's `bindings_unread` carries as its cause, so that the real cause (a selection that
/// cannot be read, a configuration without a state root) is not lost.
pub fn dispatch_unlocated(arguments: &[OsString], located: &Outcome) -> Option<cli_line::Exit> {
    dispatch_unlocated_with(arguments, located, &run_adb, &|name: &str| {
        std::env::var_os(name)
    })
}

/// #398 DB-2 fix round 1: [`dispatch_unlocated`] with `run` and in `environment`, as
/// [`dispatch_in`] takes them, so that a test lists with its own `adb devices -l`.
pub fn dispatch_unlocated_with(
    arguments: &[OsString],
    located: &Outcome,
    run: &dyn Fn(&Path) -> AdbRun,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Option<cli_line::Exit> {
    dispatch_answer(arguments, run, &run_acib, environment, Some(located))
}

/// The answer of [`dispatch_in`] and [`dispatch_unlocated_with`]: `unlocated`, the refusal of a
/// place that located no state root, is the cause of a listing's `bindings_unread`. Workflow #479
/// F-2: `acib` runs `acib devices`.
fn dispatch_answer(
    arguments: &[OsString],
    run: &dyn Fn(&Path) -> AdbRun,
    acib: &dyn Fn(&Path, &AcibCall) -> AcibRun,
    environment: &dyn Fn(&str) -> Option<OsString>,
    unlocated: Option<&Outcome>,
) -> Option<cli_line::Exit> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let parsed = match parse(&arguments[1..], environment) {
        Ok(parsed) => parsed,
        Err(outcome) => return Some(cli_line::failure(&outcome, Some(USAGE))),
    };
    let program = std::env::current_exe().and_then(fs::canonicalize).ok();
    let root = parsed.root.as_deref();
    let answer = match (parsed.action, parsed.alias.as_deref()) {
        (BIND, Some(alias)) => {
            // #398 DB-2: by the handle, or by the device key under the caller's salt.
            let chosen = match (parsed.handle.as_deref(), parsed.device_key.as_deref()) {
                (Some(handle), _) => Chosen::Handle(handle),
                (None, Some(device_key)) => Chosen::Key {
                    device_key,
                    salt: parsed.key_salt.as_ref(),
                },
                (None, None) => {
                    return Some(cli_line::failure(
                        &cli_line::usage_invalid(Some("handle"), ArgumentProblem::Required),
                        Some(USAGE),
                    ));
                }
            };
            let request = Request {
                alias,
                chosen,
                via: parsed.bound_via.unwrap_or(DeviceBoundVia::Cli),
            };
            bind_against(
                &parsed.against,
                root,
                program.as_deref(),
                &request,
                run,
                acib,
            )
            .map(|answer| (answer, ActingctlCode::DeviceBindingChanged))
        }
        (UNBIND, Some(alias)) => match &parsed.against {
            Against::StateRoot(state_root) => unbind(state_root, alias)
                .map(|answer| (answer, ActingctlCode::DeviceBindingChanged)),
            // `--config` is `bind`'s and `list`'s alone (`flags_of`): `unbind` refuses it as
            // unknown, and a state root is required.
            Against::Candidate(_) => Err(cli_line::usage_invalid(
                Some(CONFIG_FLAG),
                ArgumentProblem::Unknown,
            )),
            Against::Unnamed => Err(cli_line::usage_invalid(
                Some("--state-root"),
                ArgumentProblem::Required,
            )),
        },
        _ => {
            let request = ListRequest {
                against: &parsed.against,
                root,
                key_salt: parsed.key_salt.as_ref(),
                unlocated,
            };
            list_with_acib(&request, program.as_deref(), run, acib)
                .map(|answer| (answer, ActingctlCode::DeviceListReported))
        }
    };
    Some(match answer {
        Ok((answer, code)) => {
            cli_line::success(cli_line::members(answer, COMMAND), &Outcome::new(code))
        }
        Err(outcome) => cli_line::failure(&outcome, None),
    })
}

// Test-only: Workflow #398 RD-1c, model-398 v1.1 §12 rows RD-1c-T1, T2, T3 and T5, with a fake
// `adb devices -l` (neutral serials, a hand-written listing) and no device.
#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::outcome::Category;
    use tempfile::TempDir;

    const PHONE: &str = "node.phone";
    const SECOND_PHONE: &str = "node.phone-b";
    const EMULATOR: &str = "node.emulator";
    // A handset on USB, a loopback emulator and a handset that has not allowed debugging yet.
    const LISTING: &str = "List of devices attached\n\
        TESTSERIAL0001         device usb:1-1 product:neutral model:Neutral_Phone device:neutral transport_id:7\n\
        127.0.0.1:5555         device product:neutral_emu model:Neutral_Emulator device:neutral transport_id:3\n\
        TESTSERIAL0002         unauthorized usb:1-2 transport_id:8\n\n";

    // An install root without slots whose configuration has two handsets and an emulator, and
    // an empty state root.
    fn roots() -> (TempDir, TempDir) {
        let install = TempDir::new().expect("install root");
        fs::write(
            install.path().join("actingd.config.json"),
            json!({
                "instances": [
                    {"alias": PHONE, "device": {"kind": "handset", "transport": "usb"}},
                    {"alias": SECOND_PHONE, "device": {"kind": "handset"}},
                    {"alias": EMULATOR, "port": 5555},
                ],
            })
            .to_string(),
        )
        .expect("write the configuration");
        (install, TempDir::new().expect("state root"))
    }

    fn listed(_: &Path) -> AdbRun {
        AdbRun::Printed(LISTING.to_owned())
    }

    fn request<'a>(alias: &'a str, handle: &'a str) -> Bind<'a> {
        Bind {
            alias,
            handle,
            via: DeviceBoundVia::Cli,
        }
    }

    // T1: one row per listed device with its handle, transport, state, model and bound alias,
    // a loopback endpoint for the emulator, and no serial anywhere in the answer.
    #[test]
    fn rd1c_t1_list_answers_each_device_by_handle_and_never_its_serial() {
        let (install, state) = roots();
        bind(
            state.path(),
            Some(install.path()),
            &request(PHONE, "usb:7"),
            &listed,
        )
        .expect("bind the phone");
        let answer = list(state.path(), Some(install.path()), &listed).expect("the list");
        assert_eq!(
            answer,
            json!({"devices": [
                {"handle": "usb:7", "transport": "usb", "state": "device", "model": "Neutral_Phone", "bound_alias": PHONE},
                {"handle": "tcp:3", "transport": "tcp", "state": "device", "model": "Neutral_Emulator", "endpoint": "127.0.0.1:5555"},
                {"handle": "usb:8", "transport": "usb", "state": "unauthorized"},
            ]})
        );
        let text = answer.to_string();
        assert!(
            !text.contains("TESTSERIAL0001") && !text.contains("TESTSERIAL0002"),
            "{text}"
        );
    }

    // T2: binding writes the device's serial for the alias into the private file, with the
    // channel; the answer is the alias, the handle, the model and restart_required, no serial.
    #[test]
    fn rd1c_t2_bind_writes_the_serial_for_the_alias_and_answers_without_it() {
        let (install, state) = roots();
        let answer = bind(
            state.path(),
            Some(install.path()),
            &request(PHONE, "usb:7"),
            &listed,
        )
        .expect("bind the phone");
        assert_eq!(
            answer,
            json!({"alias": PHONE, "handle": "usb:7", "model": "Neutral_Phone", "restart_required": true})
        );
        let bindings = read_device_bindings(state.path()).expect("the private file");
        let binding = bindings.get(PHONE).expect("the phone's binding");
        assert_eq!(
            (binding.serial(), binding.bound_via),
            (Some("TESTSERIAL0001"), DeviceBoundVia::Cli)
        );
        assert!(binding.bound_at_unix_ms > 0);
    }

    // T3: an unlisted handle is device_handle_unknown, an alias that is no handset
    // device_kind_unsupported, one the configuration does not have instance_unknown, a device
    // already bound to another alias device_bindings_invalid (duplicate_serial), and adb that
    // fails device_list_failed with adb's text, the bound serial replaced by its label. No
    // refusal names a serial, and none writes the file.
    #[test]
    fn rd1c_t3_each_refusal_carries_its_code_and_names_no_serial() {
        let (install, state) = roots();
        let root = Some(install.path());
        bind(state.path(), root, &request(PHONE, "usb:7"), &listed).expect("bind the phone");
        let refused = |request: &Bind<'_>, run: &dyn Fn(&Path) -> AdbRun| {
            bind(state.path(), root, request, run).expect_err("a refusal")
        };
        let unknown_handle = refused(&request(PHONE, "usb:99"), &listed);
        assert!(
            unknown_handle.is(ActingctlCode::DeviceHandleUnknown)
                && unknown_handle.value("handle") == Some(&json!("usb:99")),
            "{unknown_handle}"
        );
        let not_handset = refused(&request(EMULATOR, "tcp:3"), &listed);
        assert!(
            not_handset.is(ActingctlCode::DeviceKindUnsupported)
                && not_handset.value("alias") == Some(&json!(EMULATOR)),
            "{not_handset}"
        );
        let unknown_alias = refused(&request("node.unknown", "usb:7"), &listed);
        assert!(
            unknown_alias.is(RuntimeErrorCode::InstanceUnknown),
            "{unknown_alias}"
        );
        let twice = refused(&request(SECOND_PHONE, "usb:7"), &listed);
        assert!(
            twice.is(ActingctlCode::DeviceBindingsInvalid)
                && twice.value("device_binding_problem") == Some(&json!("duplicate_serial")),
            "{twice}"
        );
        let failing = |_: &Path| AdbRun::Failed {
            exit_code: Some(1),
            stderr: "error: device 'TESTSERIAL0001' not found\n".to_owned(),
        };
        let adb_failed = refused(&request(PHONE, "usb:7"), &failing);
        assert!(
            adb_failed.is(ActingctlCode::DeviceListFailed)
                && adb_failed.category() == Category::Error,
            "{adb_failed}"
        );
        let envelope = serde_json::to_string(&adb_failed.envelope(Detail::Full))
            .expect("an envelope serialises");
        assert!(
            !envelope.contains("TESTSERIAL0001")
                && envelope.contains("[device:handset:node.phone]"),
            "{envelope}"
        );
        for refusal in [
            &unknown_handle,
            &not_handset,
            &unknown_alias,
            &twice,
            &adb_failed,
        ] {
            let text = format!("{refusal}");
            assert!(!text.contains("TESTSERIAL"), "{text}");
        }
        let bindings = read_device_bindings(state.path()).expect("the private file");
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.alias.as_str())
                .collect::<Vec<_>>(),
            [PHONE]
        );
    }

    // T5: unbinding removes the alias's binding, which the Runtime's reader no longer finds (a
    // start then registers the handset unbound, RD-1b-T4), and keeps the others; an alias
    // without a binding answers unbound false and writes nothing.
    #[test]
    fn rd1c_t5_unbind_removes_the_binding_the_runtime_reads_and_keeps_the_others() {
        let (install, state) = roots();
        let root = Some(install.path());
        bind(state.path(), root, &request(PHONE, "usb:7"), &listed).expect("bind the phone");
        let second = |_: &Path| {
            AdbRun::Printed(
                "List of devices attached\nTESTSERIAL0002 device usb:1-2 model:Neutral_Phone_B transport_id:9\n"
                    .to_owned(),
            )
        };
        bind(state.path(), root, &request(SECOND_PHONE, "usb:9"), &second)
            .expect("bind the second phone");
        assert_eq!(
            unbind(state.path(), PHONE).expect("unbind the phone"),
            json!({"alias": PHONE, "unbound": true, "restart_required": true})
        );
        let bindings = read_device_bindings(state.path()).expect("the private file");
        assert!(bindings.get(PHONE).is_none());
        assert_eq!(
            bindings.get(SECOND_PHONE).and_then(DeviceBinding::serial),
            Some("TESTSERIAL0002")
        );
        assert_eq!(
            unbind(state.path(), PHONE).expect("unbind it again"),
            json!({"alias": PHONE, "unbound": false, "restart_required": false})
        );
    }
}
