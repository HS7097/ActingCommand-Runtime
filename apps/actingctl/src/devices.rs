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
//!
//! None of them runs the Runtime or records anything in the ledger: the next start reads the
//! file and records the binding by fingerprint, so a change answers `restart_required`. No
//! answer, outcome value or developer line names a device serial: adb's own text has every
//! serial the file binds replaced by the device's label `[device:handset:<alias>]`
//! (`raw_text_redacted`, model-398 §3.5 rule 4).

use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem};
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_contract::{
    DEVICE_BINDINGS_SCHEMA, DeviceBinding, DeviceBindings, DeviceBindingsFile, DeviceBoundVia,
    InstalledProcess, InstanceDeviceKind, RuntimeErrorCode, device_bindings_path,
    parse_device_bindings, read_device_bindings,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const COMMAND: &str = "devices";
pub(crate) const LIST: &str = "list";
pub(crate) const BIND: &str = "bind";
pub(crate) const UNBIND: &str = "unbind";
const USAGE: &str = "usage: actingctl devices list --state-root <dir> [--root <install root>]\n       actingctl devices bind <alias> <handle> --state-root <dir> [--root <install root>]\n       actingctl devices unbind <alias> --state-root <dir>";

/// One run of `adb devices -l`: what it printed, or why it printed nothing.
pub(crate) enum AdbRun {
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
pub(crate) fn run_adb(adb: &Path) -> AdbRun {
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
        if self.usb { "usb" } else { "tcp" }
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
}

/// The selected configuration of the install root and its adb, found as `actingctl packs` finds
/// them (command table D5): this process's install selection, else `root`'s, else the install
/// root this executable runs from. Its adb is `<install root>\tools\platform-tools\adb.exe`.
fn install(root: Option<&Path>) -> Result<(Option<InstalledProcess>, PathBuf, PathBuf), Outcome> {
    let (installation, config) =
        crate::packs::selected_config(root).map_err(|error| error.outcome())?;
    let install_root = installation
        .as_ref()
        .map(|installation| installation.root().to_path_buf())
        .or_else(|| root.map(Path::to_path_buf))
        .or_else(crate::packs::executable_install_root)
        .ok_or_else(|| Outcome::new(ActingctlCode::InstallRootUnresolved))?;
    let adb = install_root
        .join("tools")
        .join("platform-tools")
        .join("adb.exe");
    Ok((installation, config, adb))
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
    for binding in bindings.iter() {
        if text.contains(binding.serial()) {
            text = text.replace(
                binding.serial(),
                &format!("[device:handset:{}]", binding.alias),
            );
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

/// `{devices: [{handle?, transport, state, model?, bound_alias?, endpoint?}]}`: every device adb
/// lists now, in its order, with the alias the private file of `state_root` binds it to. No row
/// names a serial; a loopback `tcp` row (an emulator) names its `endpoint`.
pub(crate) fn list(
    state_root: &Path,
    root: Option<&Path>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    let bindings = bindings(state_root)?;
    let (_, _, adb) = install(root)?;
    let rows = listing(&adb, &bindings, run)?
        .iter()
        .map(|listed| {
            let mut row = Map::new();
            if let Some(handle) = listed.handle() {
                row.insert("handle".to_owned(), json!(handle));
            }
            row.insert("transport".to_owned(), json!(listed.transport()));
            row.insert("state".to_owned(), json!(listed.state));
            if let Some(model) = &listed.model {
                row.insert("model".to_owned(), json!(model));
            }
            if let Some(binding) = bindings
                .iter()
                .find(|binding| binding.serial() == listed.serial)
            {
                row.insert("bound_alias".to_owned(), json!(binding.alias));
            }
            let loopback = listed
                .serial
                .parse::<SocketAddr>()
                .is_ok_and(|address| address.ip().is_loopback());
            if !listed.usb && loopback {
                row.insert("endpoint".to_owned(), json!(listed.serial));
            }
            Value::Object(row)
        })
        .collect::<Vec<_>>();
    Ok(json!({"devices": rows}))
}

/// One binding request: the handset's alias and the handle of the device `list` showed.
pub(crate) struct Bind<'a> {
    pub(crate) alias: &'a str,
    pub(crate) handle: &'a str,
    pub(crate) via: DeviceBoundVia,
}

/// Binds the device of `handle` to the handset `alias`: `{alias, handle, model?,
/// restart_required: true}`. An alias the selected configuration does not have is
/// `instance_unknown`; one that is not a handset `device_kind_unsupported`; a handle adb does not
/// list now `device_handle_unknown`; a binding the file's rules refuse (the device already bound
/// to another alias) `device_bindings_invalid`; a file that cannot be written
/// `device_bindings_write_failed`.
pub(crate) fn bind(
    state_root: &Path,
    root: Option<&Path>,
    request: &Bind<'_>,
    run: &dyn Fn(&Path) -> AdbRun,
) -> Result<Value, Outcome> {
    let (installation, config, adb) = install(root)?;
    let instances = configured_instances(installation.as_ref(), &config)?;
    let instance = instances
        .iter()
        .find(|instance| instance.alias == request.alias)
        .ok_or_else(|| {
            Outcome::new(RuntimeErrorCode::InstanceUnknown).instance_selector(request.alias)
        })?;
    if instance.device.as_ref().map(|device| device.kind) != Some(InstanceDeviceKind::Handset) {
        return Err(Outcome::new(ActingctlCode::DeviceKindUnsupported).alias(request.alias));
    }
    let bindings = bindings(state_root)?;
    let listed = listing(&adb, &bindings, run)?
        .into_iter()
        .find(|listed| listed.handle().as_deref() == Some(request.handle))
        .ok_or_else(|| Outcome::new(ActingctlCode::DeviceHandleUnknown).handle(request.handle))?;
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
    answer.insert("handle".to_owned(), json!(request.handle));
    if let Some(model) = &listed.model {
        answer.insert("model".to_owned(), json!(model));
    }
    answer.insert("restart_required".to_owned(), json!(true));
    Ok(Value::Object(answer))
}

/// Removes the binding of `alias`: `{alias, unbound, restart_required}`, both true when the file
/// had one and false when it had none (nothing is written then).
pub(crate) fn unbind(state_root: &Path, alias: &str) -> Result<Value, Outcome> {
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
    let bytes = match installation {
        Some(selected) => selected.config_bytes().map_err(|error| {
            Outcome::new(SharedCode::ConfigReadFailed)
                .path(config)
                .caused_by(Outcome::new(error.code()))
        })?,
        None => fs::read(config).map_err(|error| {
            Outcome::new(SharedCode::ConfigReadFailed)
                .path(config)
                .caused_by(Outcome::from_io_error(&error, IoOp::Read))
        })?,
    };
    serde_json::from_slice::<ConfigFile>(&bytes)
        .map(|file| file.instances)
        .map_err(|error| {
            Outcome::new(SharedCode::ConfigDecodeFailed)
                .path(config)
                .caused_by(
                    Outcome::new(ContractCode::ForeignDataError)
                        .raw_source(RawSource::Json)
                        .raw_text(error.to_string()),
                )
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
    let file = DeviceBindingsFile {
        schema: DEVICE_BINDINGS_SCHEMA.to_owned(),
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
    state_root: PathBuf,
    root: Option<PathBuf>,
}

/// `devices <list|bind <alias> <handle>|unbind <alias>>` and the flags `flags_of` names for the
/// action, each at most once and with a value; any other word, a repeated flag or a missing value
/// is `actingctl_usage_invalid` naming it (`argument`, `argument_problem`; Workflow #382
/// A1-FLAG).
fn parse(arguments: &[OsString]) -> Result<Parsed, Outcome> {
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
        BIND => (Some(word(1, "alias")?), Some(word(2, "handle")?), 3),
        UNBIND => (Some(word(1, "alias")?), None, 2),
        _ => (None, None, 1),
    };
    let takes = crate::flags_of(COMMAND, Some(action)).unwrap_or_default();
    let mut state_root = None;
    let mut root = None;
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
        let slot = if flag == "--root" {
            &mut root
        } else {
            &mut state_root
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(usage(flag, ArgumentProblem::Repeated));
        }
        index += 2;
    }
    Ok(Parsed {
        action,
        alias,
        handle,
        state_root: state_root.ok_or_else(|| usage("--state-root", ArgumentProblem::Required))?,
        root,
    })
}

/// Runs `devices …` when the first argument names it; `None` leaves every other command to the
/// CLI. One result line (Workflow #378 A5): the answer with `device_list_reported` or
/// `device_binding_changed`, or `{outcome}` with the refusal and exit 1.
pub(crate) fn dispatch(arguments: &[OsString]) -> Option<ExitCode> {
    if arguments.first()?.to_str()? != COMMAND {
        return None;
    }
    let parsed = match parse(&arguments[1..]) {
        Ok(parsed) => parsed,
        Err(outcome) => return Some(cli_line::failure(&outcome, Some(USAGE))),
    };
    let root = parsed.root.as_deref();
    let answer = match (
        parsed.action,
        parsed.alias.as_deref(),
        parsed.handle.as_deref(),
    ) {
        (BIND, Some(alias), Some(handle)) => bind(
            &parsed.state_root,
            root,
            &Bind {
                alias,
                handle,
                via: DeviceBoundVia::Cli,
            },
            &run_adb,
        )
        .map(|answer| (answer, ActingctlCode::DeviceBindingChanged)),
        (UNBIND, Some(alias), _) => unbind(&parsed.state_root, alias)
            .map(|answer| (answer, ActingctlCode::DeviceBindingChanged)),
        _ => list(&parsed.state_root, root, &run_adb)
            .map(|answer| (answer, ActingctlCode::DeviceListReported)),
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
    use actingcommand_contract::outcome::{Category, Detail};
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
            ("TESTSERIAL0001", DeviceBoundVia::Cli)
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
            bindings.get(SECOND_PHONE).map(DeviceBinding::serial),
            Some("TESTSERIAL0002")
        );
        assert_eq!(
            unbind(state.path(), PHONE).expect("unbind it again"),
            json!({"alias": PHONE, "unbound": false, "restart_required": false})
        );
    }
}
