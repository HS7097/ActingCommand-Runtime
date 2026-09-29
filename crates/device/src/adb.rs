// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(test)]
use crate::mumu::mumu_adb_candidates;
use crate::mumu::{
    MumuInstallSource, MumuInstallation, resolve_mumu_adb, resolve_mumu_installation,
};
use crate::{
    DeviceError, DeviceErrorCategory, DeviceErrorDiagnosticMessage, DeviceResourceCloseOutcome,
    DeviceResourceClosePhase, DeviceResourceKind, DeviceResourceQuiescence, DeviceResult,
};
use actingcommand_contract::FencedWrite;
use std::ffi::OsString;
use std::io::{self, Read};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
mod recovery;
pub use recovery::{
    AdbCommandEvidence, AdbRecoveryPath, AdbRecoveryPhase, AdbRecoveryStep, AdbRecoveryText,
    AdbTargetRecovery, AdbTransportState, MAX_ADB_RECOVERY_STEPS, MAX_ADB_RECOVERY_TEXT_BYTES,
};

pub const ACTINGCOMMAND_ADB_PATH_ENV: &str = "ACTINGCOMMAND_ADB_PATH";
pub const ACTINGCOMMAND_NEMU_FOLDER_ENV: &str = "ACTINGCOMMAND_NEMU_FOLDER";
pub const ACTINGCOMMAND_NEMU_IPC_DLL_ENV: &str = "ACTINGCOMMAND_NEMU_IPC_DLL";
pub const ACTINGCOMMAND_DROIDCAST_RAW_APK_ENV: &str = "ACTINGCOMMAND_DROIDCAST_RAW_APK";
pub const ACTINGCOMMAND_MINITOUCH_PATH_ENV: &str = "ACTINGCOMMAND_MINITOUCH_PATH";
pub const ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC_ENV: &str =
    "ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC";

/// The `ACTINGCOMMAND_*` environment fallbacks as values the caller injects (Workflow #318
/// cfg3). This crate never reads these variables itself: every former read point takes
/// the matching field, and `Default` (every field `None`) is "no fallback". The caller
/// decides whether the environment is consulted at all; `actingd` does so only under
/// `allow_env_overrides`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverrides {
    /// `ACTINGCOMMAND_ADB_PATH`: preferred over a configured ADB in [`resolve_adb_path`].
    pub adb_path: Option<PathBuf>,
    /// `ACTINGCOMMAND_NEMU_FOLDER`: the MuMu root for ADB and `MuMuManager` resolution and
    /// the Nemu IPC capture fallback.
    pub nemu_folder: Option<PathBuf>,
    /// `ACTINGCOMMAND_NEMU_IPC_DLL`: the Nemu IPC capture DLL fallback.
    pub nemu_ipc_dll: Option<PathBuf>,
    /// `ACTINGCOMMAND_DROIDCAST_RAW_APK`: the DroidCast_raw APK fallback.
    pub droidcast_apk: Option<PathBuf>,
    /// `ACTINGCOMMAND_MINITOUCH_PATH`: replaces the bundled minitouch path.
    pub minitouch_path: Option<PathBuf>,
    /// `ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC`, verbatim: consumed by the PPOCR
    /// provider, which validates it.
    pub ppocr_node_placement_diagnostic: Option<OsString>,
}

impl EnvOverrides {
    /// Every variable an [`EnvOverrides`] field carries, in field order.
    pub const VARIABLES: [&'static str; 6] = [
        ACTINGCOMMAND_ADB_PATH_ENV,
        ACTINGCOMMAND_NEMU_FOLDER_ENV,
        ACTINGCOMMAND_NEMU_IPC_DLL_ENV,
        ACTINGCOMMAND_DROIDCAST_RAW_APK_ENV,
        ACTINGCOMMAND_MINITOUCH_PATH_ENV,
        ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC_ENV,
    ];

    /// Fills every field from `lookup` (for example `std::env::var_os`), exactly as the
    /// former read points did: a set variable, even an empty one, is `Some`.
    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Self {
        Self {
            adb_path: lookup(ACTINGCOMMAND_ADB_PATH_ENV).map(PathBuf::from),
            nemu_folder: lookup(ACTINGCOMMAND_NEMU_FOLDER_ENV).map(PathBuf::from),
            nemu_ipc_dll: lookup(ACTINGCOMMAND_NEMU_IPC_DLL_ENV).map(PathBuf::from),
            droidcast_apk: lookup(ACTINGCOMMAND_DROIDCAST_RAW_APK_ENV).map(PathBuf::from),
            minitouch_path: lookup(ACTINGCOMMAND_MINITOUCH_PATH_ENV).map(PathBuf::from),
            ppocr_node_placement_diagnostic: lookup(
                ACTINGCOMMAND_PPOCR_NODE_PLACEMENT_DIAGNOSTIC_ENV,
            ),
        }
    }
}

#[cfg(test)]
std::thread_local! {
    static TEST_MUMU_DISCOVERY_ERROR: std::cell::RefCell<Option<DeviceError>> = const {
        std::cell::RefCell::new(None)
    };
}

#[derive(Debug, Clone)]
pub struct AdbConfig {
    pub adb_path: String,
    pub command_timeout: Duration,
}

impl Default for AdbConfig {
    fn default() -> Self {
        Self {
            // Discovery is fallible and must be requested through `resolve`.
            adb_path: String::new(),
            command_timeout: Duration::from_secs(12),
        }
    }
}

impl AdbConfig {
    pub fn resolve(
        configured: Option<&str>,
        env: &EnvOverrides,
    ) -> DeviceResult<(Self, ResolvedAdbPath)> {
        let resolved = resolve_adb_path(configured, env)?;
        let config = Self {
            adb_path: resolved.path.clone(),
            ..Self::default()
        };
        Ok((config, resolved))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAdbPath {
    pub path: String,
    pub source: AdbPathSource,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbPathSource {
    Environment,
    MumuFolderEnvironment,
    MumuRunningProcess,
    MumuRegistryUninstall,
    MumuVendorEnumeration,
    UserConfig,
    PathBaseline,
}

impl AdbPathSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "env:ACTINGCOMMAND_ADB_PATH",
            Self::MumuFolderEnvironment => "env:ACTINGCOMMAND_NEMU_FOLDER",
            Self::MumuRunningProcess => "mumu_running_process",
            Self::MumuRegistryUninstall => "mumu_registry_uninstall",
            Self::MumuVendorEnumeration => "mumu_vendor_enumeration",
            Self::UserConfig => "user_config",
            Self::PathBaseline => "path_adb_baseline",
        }
    }
}

/// Resolves the ADB executable: the injected `env.adb_path`, the configured path, the
/// injected `env.nemu_folder` MuMu root, then MuMu discovery with the `PATH` baseline.
pub fn resolve_adb_path(
    configured: Option<&str>,
    env: &EnvOverrides,
) -> DeviceResult<ResolvedAdbPath> {
    if let Some(path) = env.adb_path.clone() {
        return resolved_existing_adb(path, AdbPathSource::Environment);
    }
    if let Some(path) = configured.filter(|value| !value.trim().is_empty()) {
        return resolved_existing_adb(PathBuf::from(path), AdbPathSource::UserConfig);
    }
    let explicit_root = env.nemu_folder.clone();
    if explicit_root.is_some() {
        let installation = resolve_mumu_installation_for_adb(explicit_root)?;
        return resolve_adb_path_after_discovery(installation, None);
    }

    let path_candidate = path_adb_candidate();
    match resolve_mumu_installation_for_adb(None) {
        Ok(installation) => resolve_adb_path_after_discovery(installation, path_candidate),
        Err(discovery_error) => {
            let Some(path) = path_candidate else {
                return Err(discovery_error);
            };
            let mut resolved = resolved_existing_adb(path, AdbPathSource::PathBaseline)?;
            resolved.warning = Some(format!(
                "WARNING: context=automatic_mumu_discovery original_error={discovery_error} fallback=path_adb_baseline fallback_attempts=1 affected_module=actingcommand_device::adb user_impact=MuMu-specific ADB discovery is unavailable; continuing with PATH adb {}",
                resolved.path
            ));
            Ok(resolved)
        }
    }
}

fn resolve_mumu_installation_for_adb(
    explicit_root: Option<PathBuf>,
) -> DeviceResult<Option<MumuInstallation>> {
    #[cfg(test)]
    if explicit_root.is_none()
        && let Some(error) = TEST_MUMU_DISCOVERY_ERROR.with(|slot| slot.borrow().clone())
    {
        return Err(error);
    }
    resolve_mumu_installation(explicit_root)
}

fn resolve_adb_path_after_discovery(
    installation: Option<MumuInstallation>,
    path_candidate: Option<PathBuf>,
) -> DeviceResult<ResolvedAdbPath> {
    if let Some(installation) = installation {
        let source = match installation.source {
            MumuInstallSource::ExplicitFolder => AdbPathSource::MumuFolderEnvironment,
            MumuInstallSource::ConfiguredBackendPath => AdbPathSource::UserConfig,
            MumuInstallSource::RunningProcess => AdbPathSource::MumuRunningProcess,
            MumuInstallSource::RegistryUninstall => AdbPathSource::MumuRegistryUninstall,
            MumuInstallSource::VendorEnumeration => AdbPathSource::MumuVendorEnumeration,
        };
        return resolved_existing_adb(resolve_mumu_adb(&installation)?, source);
    }
    if let Some(path) = path_candidate {
        let mut resolved = resolved_existing_adb(path, AdbPathSource::PathBaseline)?;
        resolved.warning = Some(
            "WARNING: using PATH adb as a non-MuMu baseline channel because MuMu-specific ADB discovery and user configuration did not resolve an adb path"
                .to_string(),
        );
        return Ok(resolved);
    }
    Err(DeviceError::fatal(
        "ADB path is not configured. Set ACTINGCOMMAND_ADB_PATH, set ACTINGCOMMAND_NEMU_FOLDER to a MuMu folder, install MuMu at a known path, configure actinglab adb_path, or install adb on PATH for the non-MuMu baseline channel.",
    ))
}

fn resolved_existing_adb(path: PathBuf, source: AdbPathSource) -> DeviceResult<ResolvedAdbPath> {
    if !path.is_file() {
        return Err(DeviceError::fatal(format!(
            "resolved ADB path from {} does not exist or is not a file: {}",
            source.as_str(),
            path.display()
        )));
    }
    Ok(ResolvedAdbPath {
        path: path.to_string_lossy().to_string(),
        source,
        warning: None,
    })
}

fn path_adb_candidate() -> Option<PathBuf> {
    let names = if cfg!(windows) {
        &["adb.exe"][..]
    } else {
        &["adb"][..]
    };
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub stdout_lossy_decode: bool,
    pub stderr_lossy_decode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub stderr_lossy_decode: bool,
}

#[derive(Debug, Clone)]
pub struct Adb {
    config: AdbConfig,
}

impl Adb {
    pub fn new(config: AdbConfig) -> Self {
        Self { config }
    }

    pub fn connect(&self, serial: &str) -> DeviceResult<CommandOutput> {
        self.run(&["connect", serial])
    }

    pub fn get_state(&self, serial: &str) -> DeviceResult<String> {
        let output = self.run(&["-s", serial, "get-state"])?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn ensure_device(&self, serial: &str, connect_allowed: bool) -> DeviceResult<String> {
        device_state_sequence(
            serial,
            connect_allowed,
            &answered_device,
            &|| true,
            &mut |_, args| self.run(args),
        )
        .into_ensured(serial)
    }

    /// One baseline probe; every command shares the caller's deadline and stop condition.
    pub fn ensure_device_until(
        &self,
        serial: &str,
        connect_allowed: bool,
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<String> {
        let run = |args: &[&str]| {
            if stopped() || Instant::now() >= deadline {
                return Err(DeviceError::fatal(
                    "ADB baseline stopped or deadline expired before command",
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = run_text_command_until(
                &self.config.adb_path,
                args,
                self.config.command_timeout.min(remaining),
                None,
                Some((deadline, stopped)),
            );
            if stopped() || Instant::now() >= deadline {
                return Err(match result {
                    Err(error) => error,
                    Ok(output) => DeviceError::fatal(format!(
                        "ADB baseline stopped or deadline expired after command; command={args:?}; output={output:?}"
                    )),
                });
            }
            result
        };
        let sequence = device_state_sequence(
            serial,
            connect_allowed,
            &answered_device,
            &|| !stopped() && Instant::now() < deadline,
            &mut |_, args| run(args),
        );
        until_verdict(serial, sequence)
    }

    pub fn screen_size(&self, serial: &str) -> DeviceResult<String> {
        let output = self.run(&["-s", serial, "shell", "wm", "size"])?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn shell_input_tap(
        &self,
        witness: &FencedWrite,
        serial: &str,
        x: i32,
        y: i32,
    ) -> DeviceResult<CommandOutput> {
        let x = x.to_string();
        let y = y.to_string();
        self.run_write(witness, &["-s", serial, "shell", "input", "tap", &x, &y])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn shell_input_swipe(
        &self,
        witness: &FencedWrite,
        serial: &str,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> DeviceResult<CommandOutput> {
        let x1 = x1.to_string();
        let y1 = y1.to_string();
        let x2 = x2.to_string();
        let y2 = y2.to_string();
        let duration_ms = duration_ms.to_string();
        self.run_write(
            witness,
            &[
                "-s",
                serial,
                "shell",
                "input",
                "swipe",
                &x1,
                &y1,
                &x2,
                &y2,
                &duration_ms,
            ],
        )
    }

    pub fn force_stop(
        &self,
        witness: &FencedWrite,
        serial: &str,
        package: &str,
    ) -> DeviceResult<CommandOutput> {
        self.run_write(
            witness,
            &["-s", serial, "shell", "am", "force-stop", package],
        )
    }

    pub fn launch_package(
        &self,
        witness: &FencedWrite,
        serial: &str,
        package: &str,
    ) -> DeviceResult<CommandOutput> {
        self.run_write(
            witness,
            &[
                "-s",
                serial,
                "shell",
                "monkey",
                "-p",
                package,
                "-c",
                "android.intent.category.LAUNCHER",
                "1",
            ],
        )
    }

    /// Read-only: the package name of the activity Android reports as resumed
    /// (`dumpsys activity activities`, filtered on the device to the marker lines,
    /// `topResumedActivity` first, then `mResumedActivity` / `ResumedActivity`). `Ok(None)`
    /// means the command ran but no resumed activity was reported (for example
    /// mid-transition); an error is an ADB failure, never a parse outcome.
    ///
    /// Workflow #191 E1: the transport is re-checked (get-state, one `adb connect` when
    /// `connect_allowed`, get-state) only when the query itself fails, and the query runs
    /// once more only when that connect brought adbd back to `device`. Every outcome is the
    /// one the former `ensure_device`-then-query order gave.
    pub fn foreground_package(
        &self,
        serial: &str,
        connect_allowed: bool,
    ) -> DeviceResult<Option<String>> {
        foreground_with_recheck(
            serial,
            connect_allowed,
            &mut || self.query_foreground_package(serial),
            &mut |_, args| self.run(args),
        )
    }

    fn query_foreground_package(&self, serial: &str) -> DeviceResult<Option<String>> {
        let output = self.run(&["-s", serial, "shell", FOREGROUND_QUERY])?;
        foreground_query_result(&output)
    }

    pub fn screencap(&self, serial: &str, timeout: Duration) -> DeviceResult<BinaryOutput> {
        run_binary_with_timeout(
            &self.config.adb_path,
            &["-s", serial, "exec-out", "screencap", "-p"],
            timeout,
        )
    }

    pub fn forward(&self, serial: &str, local: &str, remote: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "forward", local, remote])
    }

    pub fn shell_spawn(&self, serial: &str, args: &[&str]) -> DeviceResult<Child> {
        validate_adb_path(&self.config.adb_path)?;
        Command::new(&self.config.adb_path)
            .args(["-s", serial, "shell"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                DeviceError::fatal(format!(
                    "failed to spawn adb shell {}: {err}",
                    args.join(" ")
                ))
            })
    }

    pub fn push(&self, serial: &str, local: &str, remote: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "push", local, remote])
    }

    pub fn chmod(&self, serial: &str, remote: &str, mode: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "shell", "chmod", mode, remote])
    }

    /// Read-only commands, plus the connection-time commands of backend opening
    /// (`connect`, `push`, `chmod`, `forward`), which stay under the open-phase authority.
    /// A command with a device effect (input, application lifecycle) goes through
    /// `run_write`.
    pub fn run(&self, args: &[&str]) -> DeviceResult<CommandOutput> {
        run_text_with_timeout(&self.config.adb_path, args, self.config.command_timeout)
    }

    /// A device-effect command, admitted by the scheduler-issued step witness. The
    /// witness is the type-level proof that a step was begun; the scheduler validates it.
    pub fn run_write(&self, _witness: &FencedWrite, args: &[&str]) -> DeviceResult<CommandOutput> {
        run_text_with_timeout(&self.config.adb_path, args, self.config.command_timeout)
    }

    pub(crate) fn run_until(
        &self,
        args: &[&str],
        deadline: Instant,
    ) -> DeviceResult<CommandOutput> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                DeviceError::fatal("capture geometry deadline expired before adb command")
            })?;
        let mut config = self.config.clone();
        config.command_timeout = config.command_timeout.min(remaining);
        // The existing command owner still performs its original bounded cleanup.
        let output = Self::new(config).run(args)?;
        if Instant::now() >= deadline {
            return Err(DeviceError::fatal(
                "capture geometry deadline expired during adb command",
            ));
        }
        Ok(output)
    }
}

/// Pure parse of `dumpsys activity activities`: the package of the first
/// `ActivityRecord{<hash> u<user> <package>/<activity> ...}` behind a resumed-activity
/// marker, `topResumedActivity=` taking precedence over `mResumedActivity:` /
/// `ResumedActivity:`. `None` when no marker carries a record.
pub fn parse_foreground_package(dumpsys: &str) -> Option<String> {
    const MARKERS: [&str; 3] = [
        "topResumedActivity=",
        "mResumedActivity:",
        "ResumedActivity:",
    ];
    MARKERS.iter().find_map(|marker| {
        dumpsys.lines().find_map(|line| {
            line.find(marker)
                .and_then(|index| activity_record_package(&line[index + marker.len()..]))
        })
    })
}

fn activity_record_package(record: &str) -> Option<String> {
    let body = record
        .find('{')
        .map_or(record, |index| &record[index + 1..]);
    let body = body.split('}').next()?;
    let mut tokens = body.split_whitespace();
    while let Some(token) = tokens.next() {
        let is_user = token.len() > 1
            && token.starts_with('u')
            && token[1..].bytes().all(|byte| byte.is_ascii_digit());
        if !is_user {
            continue;
        }
        let package = tokens.next()?.split('/').next()?;
        if package.is_empty()
            || package
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return None;
        }
        return Some(package.to_owned());
    }
    None
}

/// Workflow #191 E1: every line the parser can read (all three markers) plus dumpsys' own
/// exit status, so "no marker" stays `Ok(None)` and a failed dumpsys stays an error.
const FOREGROUND_QUERY: &str = "{ dumpsys activity activities; echo ac_dumpsys_rc=$?; } | grep -e topResumedActivity= -e ResumedActivity: -e ac_dumpsys_rc=";
const FOREGROUND_STATUS: &str = "ac_dumpsys_rc=";

/// The filtered query's verdict: the text before the last status marker is parsed, a
/// missing or non-integer status and a non-zero dumpsys status are errors.
fn foreground_query_result(output: &CommandOutput) -> DeviceResult<Option<String>> {
    let bounded = |text: &str| {
        let text = AdbRecoveryText::new(text);
        format!("{:?} (truncated={})", text.text, text.truncated)
    };
    let Some((body, status)) = output.stdout.trim_end().rsplit_once(FOREGROUND_STATUS) else {
        return Err(DeviceError::fatal(format!(
            "adb foreground query returned no dumpsys status; stdout: {}; stderr: {}",
            bounded(&output.stdout),
            bounded(&output.stderr)
        )));
    };
    match status.parse::<i32>() {
        Ok(0) => Ok(parse_foreground_package(body)),
        Ok(code) => Err(DeviceError::fatal(format!(
            "dumpsys activity activities exited with status {code}; stderr: {}",
            bounded(&output.stderr)
        ))),
        Err(error) => Err(DeviceError::fatal(format!(
            "adb foreground query returned a non-integer dumpsys status ({error}); stdout: {}; stderr: {}",
            bounded(&output.stdout),
            bounded(&output.stderr)
        ))),
    }
}

/// Workflow #191 E1: the foreground query first; only a failed query re-checks the
/// transport, and only a connect that brought adbd back to `device` repeats the query.
fn foreground_with_recheck(
    serial: &str,
    connect_allowed: bool,
    query: &mut dyn FnMut() -> DeviceResult<Option<String>>,
    run: &mut dyn FnMut(DeviceStateStep, &[&str]) -> DeviceResult<CommandOutput>,
) -> DeviceResult<Option<String>> {
    let query_error = match query() {
        Err(error)
            if error.resource_quiescence() != Some(DeviceResourceQuiescence::Unconfirmed) =>
        {
            error
        }
        answered => return answered, // Ok, or Unconfirmed as is
    };
    let sequence = device_state_sequence(serial, connect_allowed, &answered_device, &|| true, run);
    if answered_device(&sequence.first) {
        // adbd answers: the query itself failed (formerly get-state ok, dumpsys failed).
        return Err(query_error);
    }
    sequence.into_ensured(serial).map_err(|error| {
        let severity = error.severity();
        let message = format!(
            "{}; foreground query failed first: {query_error}",
            error.message()
        );
        error.with_severity_and_message(severity, message)
    })?;
    // One connect brought adbd back to `device`: the former order queried right here.
    query()
}

/// Workflow #191 E3: the one get-state → connect → get-state transport check behind
/// `ensure_device`, `ensure_device_until`, `foreground_package` and the fenced input
/// recovery. A step whose child or pipe cleanup is unconfirmed ends the sequence.
#[derive(Clone, Copy)]
enum DeviceStateStep {
    InitialState,
    Connect,
    ConnectedState,
}

struct DeviceStateSequence {
    first: DeviceResult<CommandOutput>,
    connect: Option<DeviceResult<CommandOutput>>,
    second: Option<DeviceResult<CommandOutput>>,
}

/// Runs get-state; stops when `answered`, unconfirmed, connect is not allowed or
/// `proceed` is false. Otherwise runs connect; stops when it is unconfirmed or `proceed`
/// is false. Otherwise runs get-state again.
fn device_state_sequence(
    serial: &str,
    connect_allowed: bool,
    answered: &dyn Fn(&DeviceResult<CommandOutput>) -> bool,
    proceed: &dyn Fn() -> bool,
    run: &mut dyn FnMut(DeviceStateStep, &[&str]) -> DeviceResult<CommandOutput>,
) -> DeviceStateSequence {
    let get_state = ["-s", serial, "get-state"];
    let mut sequence = DeviceStateSequence {
        first: run(DeviceStateStep::InitialState, &get_state),
        connect: None,
        second: None,
    };
    let first = &sequence.first;
    if answered(first) || unconfirmed(first).is_some() || !connect_allowed || !proceed() {
        return sequence;
    }
    let connect = sequence
        .connect
        .insert(run(DeviceStateStep::Connect, &["connect", serial]));
    if unconfirmed(connect).is_some() || !proceed() {
        return sequence;
    }
    sequence.second = Some(run(DeviceStateStep::ConnectedState, &get_state));
    sequence
}

impl DeviceStateSequence {
    /// The `ensure_device` verdict: a first `device` answer, else the first unconfirmed
    /// step as is, else the second answer, else `device_state_error`.
    fn into_ensured(self, serial: &str) -> DeviceResult<String> {
        if answered_device(&self.first) {
            return trimmed_state(&self.first);
        }
        let steps = [
            Some(&self.first),
            self.connect.as_ref(),
            self.second.as_ref(),
        ];
        if let Some(error) = steps.into_iter().flatten().find_map(unconfirmed) {
            return Err(error.clone());
        }
        let last = self.second.as_ref().unwrap_or(&self.first);
        if answered_device(last) {
            return trimmed_state(last);
        }
        let connect = self.connect.map(|result| result.map(|_| ()));
        Err(device_state_error(serial, trimmed_state(last), connect))
    }
}

/// The `ensure_device_until` verdict, shaped as before Workflow #191 E3: a failed second
/// get-state keeps its own error object under the baseline detail.
fn until_verdict(serial: &str, sequence: DeviceStateSequence) -> DeviceResult<String> {
    let first = trimmed_state(&sequence.first);
    if first.as_ref().is_ok_and(|state| state == "device") {
        return first;
    }
    let steps = [Some(&sequence.first), sequence.connect.as_ref()];
    if let Some(error) = steps.into_iter().flatten().find_map(unconfirmed) {
        return Err(error.clone());
    }
    let connected = sequence.connect.map(|result| result.map(|_| ()));
    let Some(second) = sequence.second else {
        return Err(device_state_error(serial, first, connected));
    };
    match (trimmed_state(&second), connected) {
        (Ok(state), _) if state == "device" => Ok(state),
        (Err(error), Some(connected)) => {
            let detail = format!(
                "ADB baseline initial_state={first:?}; connect={connected:?}; final_error={error}"
            );
            let severity = error.severity();
            Err(error.with_severity_and_message(severity, detail))
        }
        (state, connected) => Err(device_state_error(serial, state, connected)),
    }
}

fn answered_device(result: &DeviceResult<CommandOutput>) -> bool {
    result
        .as_ref()
        .is_ok_and(|output| output.stdout.trim() == "device")
}

fn unconfirmed(result: &DeviceResult<CommandOutput>) -> Option<&DeviceError> {
    result
        .as_ref()
        .err()
        .filter(|error| error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed))
}

fn trimmed_state(result: &DeviceResult<CommandOutput>) -> DeviceResult<String> {
    result
        .as_ref()
        .map(|output| output.stdout.trim().to_owned())
        .map_err(DeviceError::clone)
}

fn device_state_error(
    serial: &str,
    state: DeviceResult<String>,
    connect_result: Option<DeviceResult<()>>,
) -> DeviceError {
    let state_was_checked = state.is_ok();
    let diagnostic_message = match &connect_result {
        Some(Ok(())) => DeviceErrorDiagnosticMessage::AdbDeviceStateAfterConnectAttempt,
        Some(Err(_)) => DeviceErrorDiagnosticMessage::AdbDeviceStateConnectFailed,
        None => DeviceErrorDiagnosticMessage::AdbDeviceStateConnectDisabled,
    };
    let state_text = match state {
        Ok(state) => format!("state={state:?}"),
        Err(err) => format!("get-state failed: {err}"),
    };
    let connect_attempt_text = match connect_result {
        Some(Ok(())) => "; one adb connect was attempted".to_string(),
        Some(Err(err)) => format!("; one adb connect failed: {err}"),
        None => String::new(),
    };
    let error = DeviceError::fatal(format!(
        "target device {serial} is not available in device state ({state_text}{connect_attempt_text})"
    ))
        .with_diagnostic(DeviceErrorCategory::Native, "adb.ensure_device.get_state")
        .with_diagnostic_message(diagnostic_message);
    if state_was_checked {
        error.input_parameter_failure()
    } else {
        error
    }
}

pub(crate) struct RawCommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Diagnostic identity and spawn profile of a command-line program run through
/// `run_raw_with_timeout`. Labels are `'static` because resource close causes retain them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommandProgram {
    pub(crate) name: &'static str,
    pub(crate) stdout_reader: &'static str,
    pub(crate) stderr_reader: &'static str,
    /// Windows process creation flags (0 keeps the default console behaviour).
    pub(crate) windows_creation_flags: u32,
    /// Diagnostic stage attached to the bound-expiry error so a caller can tell an expired
    /// bound from a spawn or poll failure; `None` leaves that error untyped (the `adb` default).
    pub(crate) timeout_stage: Option<&'static str>,
}

pub(crate) const ADB_PROGRAM: CommandProgram = CommandProgram {
    name: "adb",
    stdout_reader: "adb_stdout",
    stderr_reader: "adb_stderr",
    windows_creation_flags: 0,
    timeout_stage: None,
};

pub fn run_text_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<CommandOutput> {
    run_text_command(adb_path, args, timeout, None)
}

pub(crate) fn run_text_in_directory_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
    directory: &std::path::Path,
) -> DeviceResult<CommandOutput> {
    run_text_command(program, args, timeout, Some(directory))
}

fn run_text_command(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
    directory: Option<&std::path::Path>,
) -> DeviceResult<CommandOutput> {
    run_text_command_until(adb_path, args, timeout, directory, None)
}

fn run_text_command_until(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
    directory: Option<&std::path::Path>,
    boundary: Option<(Instant, &dyn Fn() -> bool)>,
) -> DeviceResult<CommandOutput> {
    validate_adb_path(adb_path)?;
    let output = run_raw_with_boundary(ADB_PROGRAM, adb_path, args, timeout, directory, boundary)?;
    let stdout = decode_adb_text(output.stdout, "stdout", args);
    let stderr = decode_adb_text(output.stderr, "stderr", args);
    if output.status.success() {
        return Ok(CommandOutput {
            stdout: stdout.text,
            stderr: stderr.text,
            stdout_lossy_decode: stdout.lossy,
            stderr_lossy_decode: stderr.lossy,
        });
    }
    let evidence = recovery::command_evidence(
        &CommandOutput {
            stdout: stdout.text.clone(),
            stderr: stderr.text.clone(),
            stdout_lossy_decode: stdout.lossy,
            stderr_lossy_decode: stderr.lossy,
        },
        false,
        output.status.code(),
    );
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        args.join(" "),
        output.status,
        stdout = stdout.diagnostic_text(),
        stderr = stderr.diagnostic_text()
    ))
    .with_adb_command(evidence))
}

pub fn run_binary_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<BinaryOutput> {
    validate_adb_path(adb_path)?;
    let output = run_raw_with_timeout(ADB_PROGRAM, adb_path, args, timeout)?;
    let stderr = decode_adb_text(output.stderr, "stderr", args);
    if output.status.success() {
        return Ok(BinaryOutput {
            stdout: output.stdout,
            stderr: stderr.text,
            stderr_lossy_decode: stderr.lossy,
        });
    }
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout bytes: {}\nstderr:\n{stderr}",
        args.join(" "),
        output.status,
        output.stdout.len(),
        stderr = stderr.diagnostic_text()
    )))
}

fn validate_adb_path(adb_path: &str) -> DeviceResult<()> {
    if adb_path.trim().is_empty() {
        return Err(DeviceError::fatal(
            "ADB path is unresolved. Set ACTINGCOMMAND_ADB_PATH or ACTINGCOMMAND_NEMU_FOLDER, configure actinglab adb_path, or install adb on an absolute PATH entry for the non-MuMu baseline channel.",
        ));
    }
    Ok(())
}

pub(crate) fn run_raw_with_timeout(
    program: CommandProgram,
    program_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<RawCommandOutput> {
    run_raw_with_timeout_in_directory(program, program_path, args, timeout, None)
}

fn run_raw_with_timeout_in_directory(
    program: CommandProgram,
    program_path: &str,
    args: &[&str],
    timeout: Duration,
    directory: Option<&std::path::Path>,
) -> DeviceResult<RawCommandOutput> {
    run_raw_with_boundary(program, program_path, args, timeout, directory, None)
}

fn run_raw_with_boundary(
    program: CommandProgram,
    program_path: &str,
    args: &[&str],
    timeout: Duration,
    directory: Option<&std::path::Path>,
    boundary: Option<(Instant, &dyn Fn() -> bool)>,
) -> DeviceResult<RawCommandOutput> {
    let name = program.name;
    let expired =
        || boundary.is_some_and(|(deadline, stopped)| stopped() || Instant::now() >= deadline);
    if expired() {
        return Err(DeviceError::fatal(format!(
            "{name} command stopped or deadline expired before spawn"
        )));
    }
    let mut command = Command::new(program_path);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    if program.windows_creation_flags != 0 {
        command.creation_flags(program.windows_creation_flags);
    }
    let mut child = command.spawn().map_err(|err| {
        DeviceError::fatal(format!("failed to spawn {name} {}: {err}", args.join(" ")))
    })?;

    let acquired_count = 1 + u16::from(child.stdout.is_some()) + u16::from(child.stderr.is_some());
    let mut stdout_thread = None;
    let mut stderr_thread = None;
    let execution = (|| {
        let stdout = child.stdout.take().ok_or_else(|| {
            DeviceError::fatal(format!("failed to open {name} {} stdout", args.join(" ")))
        })?;
        stdout_thread = Some(spawn_pipe_reader(stdout, name)?);
        let stderr = child.stderr.take().ok_or_else(|| {
            DeviceError::fatal(format!("failed to open {name} {} stderr", args.join(" ")))
        })?;
        stderr_thread = Some(spawn_pipe_reader(stderr, name)?);
        let started = Instant::now();
        loop {
            if expired() {
                return Err(DeviceError::fatal(format!(
                    "{name} {} stopped or deadline expired while waiting",
                    args.join(" ")
                )));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if expired() {
                        return Err(DeviceError::fatal(format!(
                            "{name} {} completed after stop or deadline; status={status}",
                            args.join(" ")
                        )));
                    }
                    return Ok(status);
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(DeviceError::fatal(format!(
                        "failed to poll {name} {} process: {error}",
                        args.join(" ")
                    )));
                }
            }
            if started.elapsed() >= timeout {
                let expired = DeviceError::fatal(format!(
                    "{name} {} timed out after {timeout:?}",
                    args.join(" ")
                ));
                return Err(match program.timeout_stage {
                    Some(stage) => {
                        expired.with_diagnostic(DeviceErrorCategory::BackendLaunch, stage)
                    }
                    None => expired,
                });
            }
            let mut wait = timeout.saturating_sub(started.elapsed());
            if let Some((deadline, _)) = boundary {
                // stopped() has no wake-up of its own: keep the 25 ms check period.
                wait = wait
                    .min(COMMAND_POLL_INTERVAL)
                    .min(deadline.saturating_duration_since(Instant::now()));
            }
            wait_for_child_exit(&child, wait).map_err(|error| {
                DeviceError::fatal(format!(
                    "failed to wait for {name} {} process: {error}",
                    args.join(" ")
                ))
            })?;
        }
    })();
    let (status, mut failure) = match execution {
        Ok(status) => (Some(status), None),
        Err(error) => (None, Some(error)),
    };
    let close_deadline = Instant::now() + Duration::from_millis(500);
    let mut child_confirmed = status.is_some();
    if !child_confirmed {
        match stop_child(&mut child, Duration::from_millis(500), name) {
            Ok(_) => child_confirmed = true,
            Err(error) => {
                child_confirmed =
                    error.resource_quiescence() == Some(DeviceResourceQuiescence::Confirmed);
                failure = Some(
                    failure
                        .take()
                        .expect("command failure")
                        .merge_resource_cleanup(error),
                );
            }
        }
    }
    let stdout = join_pipe_reader(&mut stdout_thread, "stdout", close_deadline, program);
    let stderr = join_pipe_reader(&mut stderr_thread, "stderr", close_deadline, program);
    let readers_confirmed = stdout_thread.is_none() && stderr_thread.is_none();
    let mut output = [None, None];
    for (index, result) in [stdout, stderr].into_iter().enumerate() {
        match result {
            Ok(bytes) => output[index] = Some(bytes),
            Err(error) => {
                failure = Some(match failure.take() {
                    Some(primary) => primary.merge_resource_cleanup(error),
                    None => error,
                })
            }
        }
    }
    if !child_confirmed {
        std::mem::forget(child);
    }
    if let Some(reader) = stdout_thread {
        std::mem::forget(reader);
    }
    if let Some(reader) = stderr_thread {
        std::mem::forget(reader);
    }
    if let Some(error) = failure {
        return Err(error.with_resource_summary(
            if child_confirmed && readers_confirmed {
                DeviceResourceQuiescence::Confirmed
            } else {
                DeviceResourceQuiescence::Unconfirmed
            },
            acquired_count,
        ));
    }
    Ok(RawCommandOutput {
        status: status.expect("successful command status"),
        stdout: output[0].take().expect("successful stdout reader"),
        stderr: output[1].take().expect("successful stderr reader"),
    })
}

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Workflow #191 E2: blocks until the child exits or `wait` elapses; the caller's loop
/// re-reads the exit through `try_wait`.
#[cfg(windows)]
fn wait_for_child_exit(child: &Child, wait: Duration) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    // Round up so a sub-millisecond remainder waits instead of spinning; never INFINITE.
    let millis = u32::try_from(wait.as_nanos().div_ceil(1_000_000))
        .map_or(u32::MAX - 1, |millis| millis.min(u32::MAX - 1));
    // SAFETY: `child` owns the process handle for the whole call; waiting neither closes
    // nor transfers it.
    match unsafe { WaitForSingleObject(child.as_raw_handle(), millis) } {
        WAIT_OBJECT_0 | WAIT_TIMEOUT => Ok(()),
        other => Err(io::Error::other(format!(
            "WaitForSingleObject returned {other:#x}: {}",
            io::Error::last_os_error()
        ))),
    }
}

#[cfg(not(windows))]
fn wait_for_child_exit(_child: &Child, wait: Duration) -> io::Result<()> {
    // Non-Windows keeps the 25 ms polling.
    thread::sleep(wait.min(COMMAND_POLL_INTERVAL));
    Ok(())
}

/// Workflow #191 E2: a pipe reader thread and its completion signal, so the owner wakes
/// on completion instead of polling `is_finished`.
struct PipeReader {
    handle: JoinHandle<io::Result<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

/// Sends the completion signal when the reader thread ends, also while a panic unwinds.
/// The receiver is dropped only after the owner's join (the signal was sent before) or
/// never (it is leaked with a forgotten reader), so the send cannot fail: this is the one
/// ignored result of the executor and it hides no state.
struct ReaderDone(mpsc::Sender<()>);

impl Drop for ReaderDone {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn spawn_pipe_reader(
    reader: impl Read + Send + 'static,
    name: &'static str,
) -> DeviceResult<PipeReader> {
    let (sender, done) = mpsc::channel();
    thread::Builder::new()
        .spawn(move || {
            // Declared first, dropped last: the pipe handle is closed before the signal.
            let _done = ReaderDone(sender);
            let mut reader = reader;
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .map(|handle| PipeReader { handle, done })
        .map_err(|error| DeviceError::fatal(format!("failed to start {name} pipe reader: {error}")))
}

fn join_pipe_reader(
    reader: &mut Option<PipeReader>,
    stream_name: &'static str,
    deadline: Instant,
    program: CommandProgram,
) -> DeviceResult<Vec<u8>> {
    let Some(pipe) = reader.as_ref() else {
        return Ok(Vec::new());
    };
    // The signal is sent just before the thread finishes, so the received signal, not
    // `is_finished`, is what proves the reader closed its pipe.
    let open = !pipe.handle.is_finished()
        && matches!(
            pipe.done
                .recv_timeout(deadline.saturating_duration_since(Instant::now())),
            Err(RecvTimeoutError::Timeout)
        );
    let name = program.name;
    let backend = if stream_name == "stdout" {
        program.stdout_reader
    } else {
        program.stderr_reader
    };
    if open {
        return Err(DeviceError::fatal(format!(
            "{name} {stream_name} reader remains open at close deadline"
        ))
        .with_resource_close_cause(
            DeviceResourceKind::PipeReader,
            DeviceResourceClosePhase::WorkerJoin,
            backend,
            None,
            None,
            DeviceResourceQuiescence::Unconfirmed,
            1,
        ));
    }
    reader
        .take()
        .expect("acquired reader")
        .handle
        .join()
        .map_err(|_| DeviceError::fatal(format!("{name} {stream_name} reader thread panicked")))
        .and_then(|result| {
            result.map_err(|error| {
                DeviceError::fatal(format!("failed to read {name} {stream_name}: {error}"))
            })
        })
        .map_err(|error| {
            error.with_resource_close_cause(
                DeviceResourceKind::PipeReader,
                DeviceResourceClosePhase::WorkerJoin,
                backend,
                None,
                None,
                DeviceResourceQuiescence::Confirmed,
                1,
            )
        })
}

pub(crate) struct DecodedAdbText {
    pub(crate) text: String,
    pub(crate) lossy: bool,
    stream_name: &'static str,
    command: String,
}

impl DecodedAdbText {
    pub(crate) fn diagnostic_text(&self) -> String {
        if self.lossy {
            format!(
                "[lossy_decode=true stream={} command={}] {}",
                self.stream_name, self.command, self.text
            )
        } else {
            self.text.clone()
        }
    }
}

pub(crate) fn decode_adb_text(
    bytes: Vec<u8>,
    stream_name: &'static str,
    args: &[&str],
) -> DecodedAdbText {
    match String::from_utf8(bytes) {
        Ok(text) => DecodedAdbText {
            text,
            lossy: false,
            stream_name,
            command: args.join(" "),
        },
        Err(err) => {
            let text = String::from_utf8_lossy(err.as_bytes()).to_string();
            DecodedAdbText {
                text,
                lossy: true,
                stream_name,
                command: args.join(" "),
            }
        }
    }
}

pub fn stop_child(
    child: &mut Child,
    timeout: Duration,
    backend: &'static str,
) -> DeviceResult<DeviceResourceCloseOutcome> {
    let mut errors = Vec::new();
    match child.try_wait() {
        Ok(Some(_)) => return Ok(DeviceResourceCloseOutcome::confirmed(1)),
        Ok(None) => {}
        Err(error) => errors.push(child_close_error(
            backend,
            DeviceResourceClosePhase::InitialPoll,
            error,
            DeviceResourceQuiescence::Unconfirmed,
        )),
    }
    if let Err(error) = child.kill() {
        errors.push(child_close_error(
            backend,
            DeviceResourceClosePhase::Kill,
            error,
            DeviceResourceQuiescence::Unconfirmed,
        ));
    }
    let started = Instant::now();
    while started.elapsed() < timeout {
        match child.try_wait() {
            Ok(Some(_)) if errors.is_empty() => {
                return Ok(DeviceResourceCloseOutcome::confirmed(1));
            }
            Ok(Some(_)) => {
                return Err(aggregate_child_close_errors(
                    errors,
                    DeviceResourceQuiescence::Confirmed,
                ));
            }
            Ok(None) => {}
            Err(error) => {
                let observation = child_close_error(
                    backend,
                    DeviceResourceClosePhase::ExitPoll,
                    error,
                    DeviceResourceQuiescence::Unconfirmed,
                );
                if let Some(current) = errors.iter_mut().find(|error| {
                    error
                        .resource_close_causes()
                        .first()
                        .is_some_and(|cause| cause.phase() == DeviceResourceClosePhase::ExitPoll)
                }) {
                    if let Err(capacity) = current.fold_resource_observation(observation) {
                        errors.push(capacity);
                        return Err(aggregate_child_close_errors(
                            errors,
                            DeviceResourceQuiescence::Unconfirmed,
                        ));
                    }
                } else {
                    errors.push(observation);
                }
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    errors.push(
        DeviceError::fatal(format!("{backend} child did not exit within {timeout:?}"))
            .with_resource_close_cause(
                DeviceResourceKind::ExternalChild,
                DeviceResourceClosePhase::Deadline,
                backend,
                None,
                None,
                DeviceResourceQuiescence::Unconfirmed,
                1,
            ),
    );
    Err(aggregate_child_close_errors(
        errors,
        DeviceResourceQuiescence::Unconfirmed,
    ))
}

fn child_close_error(
    backend: &'static str,
    phase: DeviceResourceClosePhase,
    error: std::io::Error,
    quiescence: DeviceResourceQuiescence,
) -> DeviceError {
    DeviceError::fatal(format!("{backend} child close {phase:?} failed: {error}"))
        .with_resource_close_cause(
            DeviceResourceKind::ExternalChild,
            phase,
            backend,
            None,
            None,
            quiescence,
            1,
        )
}

fn aggregate_child_close_errors(
    mut errors: Vec<DeviceError>,
    quiescence: DeviceResourceQuiescence,
) -> DeviceError {
    let primary = errors.remove(0);
    errors
        .into_iter()
        .fold(primary, |primary, cleanup| {
            primary.merge_resource_cleanup(cleanup.with_resource_quiescence(quiescence, 1))
        })
        .with_resource_summary(quiescence, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;
    use std::fs;
    use std::path::Path;

    // Defect regression D14: PR298 review 5120590779, Workflow #257 C1B9 v16.
    #[test]
    fn c1b9_d14_child_occurrence_folding() {
        let mut first = child_close_error(
            "adb",
            DeviceResourceClosePhase::ExitPoll,
            io::Error::other("first"),
            DeviceResourceQuiescence::Unconfirmed,
        );
        for detail in ["middle", "last"] {
            first
                .fold_resource_observation(child_close_error(
                    "adb",
                    DeviceResourceClosePhase::ExitPoll,
                    io::Error::other(detail),
                    DeviceResourceQuiescence::Unconfirmed,
                ))
                .expect("bounded observation");
        }
        let merged = first.clone().merge_resource_cleanup(first);
        assert_eq!(merged.resource_close_causes().len(), 1);
        let cause = &merged.resource_close_causes()[0];
        assert!(cause.detail().ends_with("first"));
        assert!(cause.last_detail().expect("last detail").ends_with("last"));
        assert_eq!(cause.observation_count(), 3);
        assert_eq!(cause.dropped_count(), 1);
    }

    #[test]
    fn mumu_adb_candidates_prefer_nx_main_before_device_shells() {
        let folder = PathBuf::from(r"D:\BST\MuMuPlayer");
        let candidates = mumu_adb_candidates(&folder).expect("MuMu ADB candidates");

        assert_eq!(
            candidates.first().unwrap(),
            &folder.join("nx_main").join("adb.exe")
        );
    }

    #[test]
    fn empty_adb_config_does_not_fall_back_to_path_adb() {
        let config = AdbConfig {
            adb_path: String::new(),
            command_timeout: Duration::from_millis(1),
        };
        let adb = Adb::new(config);
        let err = adb.run(&["version"]).expect_err("empty adb must fail");

        assert!(err.to_string().contains("non-MuMu baseline channel"));
    }

    #[test]
    fn default_adb_config_is_inert_until_fallible_resolution() {
        let config = AdbConfig::default();

        assert!(config.adb_path.is_empty());
        assert_eq!(config.command_timeout, Duration::from_secs(12));
    }

    #[test]
    fn device_state_error_carries_bounded_native_stage() {
        let error = device_state_error(
            "private-device",
            Err(DeviceError::fatal("raw device state failure")),
            Some(Err(DeviceError::fatal("raw connect failure"))),
        );

        let debug = format!("{error:?}");
        assert_eq!(
            debug,
            format!(
                "DeviceError {{ severity: Fatal, message: {:?}, diagnostic: Some(DeviceErrorDiagnostic {{ category: Native, stage: \"adb.ensure_device.get_state\" }}), context: None }}",
                error.message()
            )
        );
        assert!(!debug.contains("diagnostic_message"));
        assert!(!debug.contains("AdbDeviceStateConnectFailed"));
        assert!(error.message().contains("private-device"));
        assert!(error.message().contains("raw device state failure"));
        assert!(error.message().contains("raw connect failure"));
        let diagnostic_message = error
            .diagnostic_message()
            .expect("bounded diagnostic message");
        assert_eq!(
            diagnostic_message,
            "adb device state and one connect attempt failed"
        );
        assert!(diagnostic_message.len() <= 1_024);
        assert!(!diagnostic_message.contains("private-device"));
        assert!(!diagnostic_message.contains("raw device state failure"));
        assert!(!diagnostic_message.contains("raw connect failure"));
        let diagnostic = error.diagnostic().expect("device state diagnostic");
        assert_eq!(diagnostic.category(), DeviceErrorCategory::Native);
        assert_eq!(diagnostic.stage(), "adb.ensure_device.get_state");

        // Workflow #284 ADB-TARGET-RECOVERY-v1, B13 preserved native first red.
        // Reuse the owner's command seam; no external process or device is used.
        for mode in 0..12 {
            let serial = "127.0.0.1:5555";
            let mut calls = Vec::new();
            let mut states = 0;
            let mut connects = 0;
            let result =
                recovery::ensure_input_device_with_commands(
                    serial,
                    Duration::from_secs(12),
                    |args, remaining| {
                        assert!(remaining > Duration::ZERO && remaining <= Duration::from_secs(12));
                        calls.push(
                            args.iter()
                                .map(|value| (*value).to_owned())
                                .collect::<Vec<_>>(),
                        );
                        let output = |stdout: &str, stderr: &str| CommandOutput {
                            stdout: stdout.to_owned(),
                            stderr: stderr.to_owned(),
                            stdout_lossy_decode: false,
                            stderr_lossy_decode: false,
                        };
                        if args == ["connect", serial] {
                            connects += 1;
                            if (mode == 8 && connects == 2) || mode == 9 {
                                return Err(DeviceError::fatal("preserved connect failure"));
                            }
                            return Ok(output("already connected to configured endpoint", ""));
                        }
                        if args == ["disconnect", serial] {
                            if mode == 7 {
                                return Err(DeviceError::fatal("preserved disconnect failure"));
                            }
                            return Ok(output("disconnected configured endpoint", ""));
                        }
                        assert_eq!(args, ["-s", serial, "get-state"]);
                        states += 1;
                        if mode == 0
                            || (mode == 1 && states == 2)
                            || (mode == 2 && states == 3)
                            || (mode == 3 && states == 4)
                            || (mode == 10 && states == 3)
                        {
                            return Ok(output("device\r\n", ""));
                        }
                        if mode == 6 || (mode == 10 && states == 1) {
                            return Err(DeviceError::fatal("unknown command failure"));
                        }
                        let stderr = if mode == 5 || (mode == 11 && states == 1) {
                            "error: device unauthorized.\nSee device authorization"
                        } else {
                            "error: device offline\r\n"
                        };
                        let response = output("", stderr);
                        Err(DeviceError::fatal(stderr).with_adb_command(
                            recovery::command_evidence(&response, false, Some(1)),
                        ))
                    },
                );
            let disconnects = calls
                .iter()
                .filter(|args| args.first().is_some_and(|arg| arg == "disconnect"))
                .count();
            assert_eq!(disconnects, usize::from(matches!(mode, 2..=4 | 7 | 8 | 10)));
            assert!(calls.len() <= MAX_ADB_RECOVERY_STEPS);
            assert_eq!(result.is_ok(), mode <= 3 || mode == 10);
            if mode == 0 {
                assert_eq!(calls.len(), 1);
                assert!(result.unwrap().recovery.is_none());
                continue;
            }
            let report = match result {
                Ok(ready) => {
                    assert_eq!(ready.state, "device");
                    ready
                        .recovery
                        .expect("successful recovery must carry a warning")
                }
                Err(error) => {
                    assert!(error.message().contains(if mode == 5 {
                        "unauthorized"
                    } else if mode == 6 {
                        "unknown command failure"
                    } else {
                        "offline"
                    }));
                    error
                        .adb_recovery()
                        .expect("failed recovery context")
                        .clone()
                }
            };
            assert_eq!(report.endpoint.text, serial);
            assert_eq!(report.steps.len(), calls.len());
            assert_eq!(report.recovered, mode <= 3 || mode == 10);
            assert!(!report.initial_error.text.is_empty());
            assert_eq!(report.dropped_count, 0);
            if mode != 9 {
                assert!(
                    report.steps[1]
                        .command
                        .as_ref()
                        .unwrap()
                        .stdout
                        .text
                        .contains("already connected")
                );
            }
            if mode == 4 {
                assert_eq!(report.final_state, AdbTransportState::Offline);
                assert_eq!(states, 4);
            }
            if mode == 7 || mode == 8 {
                assert_eq!(states, 2);
            }
        }
        for serial in [
            "emulator-5554",
            "usb-device",
            "",
            ":5555",
            "host:0",
            "-host:5555",
            "host:+5555",
            "host:65536",
        ] {
            assert!(!recovery::is_tcp_endpoint(serial));
        }
        assert!(recovery::is_tcp_endpoint("127.0.0.1:5555"));
        assert!(recovery::is_tcp_endpoint("[::1]:5555"));
        let inert = Adb::new(AdbConfig::default());
        for (serial, connect) in [("127.0.0.1:5555", false), ("usb-device", true)] {
            let error = inert
                .ensure_input_device(serial, connect)
                .err()
                .expect("inert config");
            assert!(
                error.adb_recovery().is_none(),
                "generic connect does not enable target recovery"
            );
        }
        let expired = recovery::ensure_input_device_with_commands(
            "127.0.0.1:5555",
            Duration::ZERO,
            |_, _| panic!("expired deadline cannot execute a command"),
        );
        assert!(expired.is_err());
    }

    #[test]
    fn adb_text_decode_is_lossy_with_diagnostic_flag() {
        let decoded = decode_adb_text(vec![b'o', b'k', 0xff], "stdout", &["shell", "echo"]);

        assert!(decoded.lossy);
        assert!(decoded.text.contains("ok"));
        assert!(decoded.diagnostic_text().contains("lossy_decode=true"));
    }

    #[test]
    fn adb_path_source_labels_are_stable() {
        assert_eq!(
            AdbPathSource::Environment.as_str(),
            "env:ACTINGCOMMAND_ADB_PATH"
        );
        assert_eq!(
            AdbPathSource::MumuFolderEnvironment.as_str(),
            "env:ACTINGCOMMAND_NEMU_FOLDER"
        );
        assert_eq!(
            AdbPathSource::MumuRunningProcess.as_str(),
            "mumu_running_process"
        );
        assert_eq!(
            AdbPathSource::MumuRegistryUninstall.as_str(),
            "mumu_registry_uninstall"
        );
        assert_eq!(
            AdbPathSource::MumuVendorEnumeration.as_str(),
            "mumu_vendor_enumeration"
        );
        assert_eq!(AdbPathSource::UserConfig.as_str(), "user_config");
        assert_eq!(AdbPathSource::PathBaseline.as_str(), "path_adb_baseline");
    }

    #[test]
    fn join_pipe_reader_returns_fatal_error_when_reader_panics() {
        let (sender, done) = mpsc::channel();
        let handle = thread::spawn(|| -> io::Result<Vec<u8>> {
            let _done = ReaderDone(sender);
            panic!("injected reader panic");
        });

        let err = join_pipe_reader(
            &mut Some(PipeReader { handle, done }),
            "stdout",
            Instant::now() + Duration::from_secs(1),
            ADB_PROGRAM,
        )
        .expect_err("reader panic must be fatal");

        assert_eq!(err.severity(), crate::DeviceErrorSeverity::Fatal);
        assert!(err.message().contains("stdout reader thread panicked"));
    }

    #[test]
    fn resolved_mumu_adb_preserves_discovery_source() {
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-mumu-adb-source-{}",
            std::process::id()
        ));
        let adb = temp.join("nx_main/adb.exe");
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(adb.parent().expect("ADB parent")).expect("ADB parent");
        fs::write(&adb, b"fixture").expect("ADB fixture");
        let installation = MumuInstallation {
            root: temp.clone(),
            source: MumuInstallSource::RunningProcess,
        };

        let resolved =
            resolve_adb_path_after_discovery(Some(installation), None).expect("resolved MuMu ADB");

        assert_eq!(resolved.source, AdbPathSource::MumuRunningProcess);
        assert_eq!(
            Path::new(&resolved.path),
            fs::canonicalize(&adb).expect("canonical ADB")
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn resolve_adb_path_uses_path_baseline_with_warning_when_mumu_and_config_are_absent() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-path-adb-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp).unwrap();
        let adb_name = if cfg!(windows) { "adb.exe" } else { "adb" };
        let adb = temp.join(adb_name);
        fs::write(&adb, b"test adb").unwrap();
        // Workflow #318 cfg3: the ADB / MuMu folder fallbacks are injected values now, so
        // only the discovery sources (`PATH`, `ProgramFiles*`) are staged in the environment.
        let original_path = std::env::var_os("PATH");
        let original_program_files = std::env::var_os("ProgramFiles");
        let original_program_files_x86 = std::env::var_os("ProgramFiles(x86)");
        let program_files = temp.join("program-files");
        let program_files_x86 = temp.join("program-files-x86");
        fs::create_dir_all(&program_files).unwrap();
        fs::create_dir_all(&program_files_x86).unwrap();
        let outcome = std::panic::catch_unwind(|| {
            unsafe {
                std::env::set_var("PATH", &temp);
                std::env::set_var("ProgramFiles", &program_files);
                std::env::set_var("ProgramFiles(x86)", &program_files_x86);
            }

            resolve_adb_path_after_discovery(None, path_adb_candidate()).expect("PATH adb baseline")
        });

        unsafe {
            match original_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
            match original_program_files {
                Some(value) => std::env::set_var("ProgramFiles", value),
                None => std::env::remove_var("ProgramFiles"),
            }
            match original_program_files_x86 {
                Some(value) => std::env::set_var("ProgramFiles(x86)", value),
                None => std::env::remove_var("ProgramFiles(x86)"),
            }
        }
        let _ = fs::remove_file(&adb);
        let _ = fs::remove_dir(&temp);
        let resolved = outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic));

        assert_eq!(resolved.source, AdbPathSource::PathBaseline);
        assert_eq!(Path::new(&resolved.path), adb.as_path());
        assert!(
            resolved
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("non-MuMu baseline"))
        );
    }

    #[test]
    fn public_adb_resolution_bounds_optional_discovery_fallback() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-public-adb-fallback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).expect("temporary PATH directory");
        let adb = temp.join(if cfg!(windows) { "adb.exe" } else { "adb" });
        fs::write(&adb, b"test adb").expect("PATH adb fixture");
        let missing_mumu = temp.join("missing-mumu-root");
        let original_path = std::env::var_os("PATH");
        let outcome = std::panic::catch_unwind(|| {
            let discovery_error = DeviceError::fatal(
                "failed to enumerate Windows processes: injected process discovery failure",
            );
            // Workflow #318 cfg3: no fallback is injected; `PATH` stays an environment source.
            let no_overrides = EnvOverrides::default();
            unsafe {
                std::env::set_var("PATH", &temp);
            }
            TEST_MUMU_DISCOVERY_ERROR.with(|slot| {
                *slot.borrow_mut() = Some(discovery_error.clone());
            });
            let fallback =
                resolve_adb_path(None, &no_overrides).expect("PATH fallback through public entry");

            unsafe {
                std::env::set_var("PATH", "");
            }
            let no_path_error =
                resolve_adb_path(None, &no_overrides).expect_err("missing PATH must stay fatal");

            unsafe {
                std::env::set_var("PATH", &temp);
            }
            let injected_root = EnvOverrides {
                nemu_folder: Some(missing_mumu.clone()),
                ..EnvOverrides::default()
            };
            let explicit_error = resolve_adb_path(None, &injected_root)
                .expect_err("explicit MuMu root must be strict");
            (fallback, no_path_error, explicit_error, discovery_error)
        });

        TEST_MUMU_DISCOVERY_ERROR.with(|slot| {
            *slot.borrow_mut() = None;
        });
        unsafe {
            match original_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = fs::remove_dir_all(&temp);
        let (fallback, no_path_error, explicit_error, discovery_error) =
            outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic));

        assert_eq!(fallback.source, AdbPathSource::PathBaseline);
        assert_eq!(Path::new(&fallback.path), adb.as_path());
        let warning = fallback.warning.expect("degraded-state warning");
        assert!(warning.contains("context=automatic_mumu_discovery"));
        assert!(warning.contains(&format!("original_error={discovery_error}")));
        assert!(warning.contains("fallback=path_adb_baseline"));
        assert!(warning.contains("fallback_attempts=1"));
        assert_eq!(no_path_error, discovery_error);
        assert!(explicit_error.message().contains("source=explicit_folder"));
        assert!(
            explicit_error
                .message()
                .contains(&missing_mumu.display().to_string())
        );
    }

    #[test]
    fn path_adb_candidate_ignores_empty_relative_and_windows_extensionless_entries() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = std::env::temp_dir().join(format!(
            "actingcommand-path-hygiene-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp).unwrap();
        let adb = temp.join("adb");
        fs::write(&adb, b"test adb").unwrap();
        let original_path = std::env::var_os("PATH");
        let path = std::env::join_paths([PathBuf::new(), temp.clone(), PathBuf::from("relative")])
            .expect("test PATH should join");
        let outcome = std::panic::catch_unwind(|| {
            unsafe {
                std::env::set_var("PATH", path);
            }

            path_adb_candidate()
        });

        unsafe {
            match original_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = fs::remove_file(&adb);
        let _ = fs::remove_dir(&temp);
        let candidate = outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic));

        if cfg!(windows) {
            assert!(candidate.is_none());
        } else {
            assert_eq!(candidate.as_deref(), Some(adb.as_path()));
        }
    }
}
