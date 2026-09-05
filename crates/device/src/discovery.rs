// SPDX-License-Identifier: AGPL-3.0-only

use crate::mumu::{mumu_adb_candidates, mumu_root_from_path};
use crate::{
    DeviceError, DeviceResult, MumuInstallSource, NemuResolutionContext, NemuResolutionCountKind,
    NemuResolutionReason,
};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

const MUMU_BASE_ADB_PORT: u16 = 16_384;
const MUMU_PORT_STEP: u16 = 32;
const MAX_MUMU_PROCESS_DIAGNOSTICS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    pub serial: String,
    pub adb_path: String,
    pub emulator: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDiscoveryProcess {
    pub process_id: u32,
    pub name: String,
    pub executable_path: Option<PathBuf>,
    pub command_line: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDiscoveryDiagnostic {
    pub process_id: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDiscoveryReport {
    pub devices: Vec<DiscoveredDevice>,
    pub diagnostics: Vec<DeviceDiscoveryDiagnostic>,
}

pub fn discover_devices() -> DeviceResult<DeviceDiscoveryReport> {
    let processes = system_processes()?;
    Ok(discover_mumu_devices_from_processes(&processes))
}

pub fn discover_mumu_devices_from_processes(
    processes: &[DeviceDiscoveryProcess],
) -> DeviceDiscoveryReport {
    let mut diagnostics = Vec::new();
    let mut devices = processes
        .iter()
        .filter_map(|process| mumu_device_from_process(process, &mut diagnostics))
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.serial.cmp(&right.serial));
    DeviceDiscoveryReport {
        devices: dedup_discovered_devices(devices),
        diagnostics,
    }
}

fn mumu_device_from_process(
    process: &DeviceDiscoveryProcess,
    diagnostics: &mut Vec<DeviceDiscoveryDiagnostic>,
) -> Option<DiscoveredDevice> {
    if !is_mumu_device_process(process) {
        return None;
    }
    let instance_id = match mumu_instance_id(process) {
        Ok(instance_id) => instance_id,
        Err(message) => {
            diagnostics.push(DeviceDiscoveryDiagnostic {
                process_id: process.process_id,
                message,
            });
            return None;
        }
    };
    let adb_path = match mumu_process_adb_path(process) {
        Ok(Some(path)) => path,
        Ok(None) => {
            diagnostics.push(DeviceDiscoveryDiagnostic {
                process_id: process.process_id,
                message: "MuMu process installation contains no discoverable ADB executable"
                    .to_string(),
            });
            return None;
        }
        Err(err) => {
            diagnostics.push(DeviceDiscoveryDiagnostic {
                process_id: process.process_id,
                message: err.message().to_string(),
            });
            return None;
        }
    };
    let port = mumu_instance_port(instance_id)?;
    Some(DiscoveredDevice {
        serial: format!("127.0.0.1:{port}"),
        adb_path: adb_path.to_string_lossy().to_string(),
        emulator: format!("mumu:{instance_id}"),
    })
}

fn is_mumu_device_process(process: &DeviceDiscoveryProcess) -> bool {
    if process.name.eq_ignore_ascii_case("MuMuNxDevice.exe") {
        return true;
    }
    process
        .executable_path
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("MuMuNxDevice.exe"))
}

fn mumu_instance_id(process: &DeviceDiscoveryProcess) -> Result<u16, String> {
    let command_line = process.command_line.as_deref().unwrap_or_default();
    match parse_dash_v_instance(command_line) {
        Ok(Some(instance_id)) => Ok(instance_id),
        Ok(None) => parse_mumu_player_comment_instance(command_line).ok_or_else(|| {
            "MuMu process has no recoverable instance id (no -v, no MuMuPlayer- comment)"
                .to_string()
        }),
        Err(err) => parse_mumu_player_comment_instance(command_line).ok_or(err),
    }
}

fn parse_dash_v_instance(command_line: &str) -> Result<Option<u16>, String> {
    let mut tokens = command_line.split_whitespace();
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case("-v") {
            let Some(value) = tokens.next() else {
                return Err("MuMu process command line has -v without an instance id".to_string());
            };
            return value.parse().map(Some).map_err(|err| {
                format!("MuMu process command line has invalid -v instance id {value:?}: {err}")
            });
        }
    }
    Ok(None)
}

fn parse_mumu_player_comment_instance(command_line: &str) -> Option<u16> {
    command_line
        .split_whitespace()
        .find(|token| token.contains("MuMuPlayer-"))
        .and_then(|token| token.split("MuMuPlayer-").nth(1))
        .and_then(|suffix| {
            suffix
                .split('-')
                .find(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        })
        .and_then(|part| part.parse().ok())
}

fn mumu_process_adb_path(process: &DeviceDiscoveryProcess) -> DeviceResult<Option<PathBuf>> {
    let Some(executable) = process.executable_path.as_deref() else {
        return Ok(None);
    };
    let Some(parent) = executable.parent() else {
        return Ok(None);
    };
    let sibling_adb = parent.join("adb.exe");
    if sibling_adb.is_file() {
        return Ok(Some(sibling_adb));
    }
    let Some(root) = mumu_root_from_path(executable) else {
        return Ok(None);
    };
    Ok(mumu_adb_candidates(&root)?
        .into_iter()
        .find(|path| path.is_file()))
}

pub(crate) fn running_mumu_executable_paths() -> DeviceResult<Vec<PathBuf>> {
    let mut paths = system_processes()?
        .into_iter()
        .filter(is_mumu_device_process)
        .filter_map(|process| process.executable_path)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(crate) fn running_mumu_executable_for_target(
    target_serial: &str,
    explicit_instance_id: Option<i32>,
) -> DeviceResult<PathBuf> {
    let processes = system_processes()?;
    running_mumu_executable_for_target_from_processes(
        target_serial,
        explicit_instance_id,
        &processes,
    )
}

pub(crate) fn running_mumu_executable_for_target_from_processes(
    target_serial: &str,
    explicit_instance_id: Option<i32>,
    processes: &[DeviceDiscoveryProcess],
) -> DeviceResult<PathBuf> {
    let target_instance_id = target_mumu_instance_id(target_serial, explicit_instance_id)?;
    let mut matches = Vec::new();
    let mut observed = Vec::new();
    let mut invalid = Vec::new();

    for process in processes
        .iter()
        .filter(|process| is_mumu_device_process(process))
    {
        match mumu_instance_id(process) {
            Ok(instance_id) => {
                push_bounded_process_diagnostic(
                    &mut observed,
                    format!(
                        "process_id={} instance_id={instance_id}",
                        process.process_id
                    ),
                );
                if instance_id == target_instance_id && matches.len() < 2 {
                    matches.push(process);
                }
            }
            Err(message) => push_bounded_process_diagnostic(
                &mut invalid,
                format!(
                    "process_id={} invalid_topology={message}",
                    process.process_id
                ),
            ),
        }
    }

    if matches.len() > 1 {
        let details = matches
            .iter()
            .map(|process| {
                format!(
                    "process_id={} executable={}",
                    process.process_id,
                    process
                        .executable_path
                        .as_deref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "<missing>".to_string())
                )
            })
            .collect::<Vec<_>>();
        return Err(DeviceError::fatal(format!(
            "running MuMu process selection is ambiguous for target serial={target_serial} instance_id={target_instance_id}; matches: {}",
            bounded_process_diagnostics(&details)
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::TargetProcessAmbiguous)
                .with_count(NemuResolutionCountKind::MatchedTargetProcesses, matches.len(), true)
                .with_source(MumuInstallSource::RunningProcess),
        ));
    }

    if let Some(process) = matches.into_iter().next() {
        let executable = process.executable_path.as_ref().ok_or_else(|| {
            DeviceError::fatal(format!(
                "running MuMu process has invalid topology for target serial={target_serial} instance_id={target_instance_id}: process_id={} has no executable path",
                process.process_id
            )).with_nemu_resolution_context_if_absent(
                NemuResolutionContext::new(NemuResolutionReason::TargetExecutableMissing)
                    .with_count(NemuResolutionCountKind::MatchedTargetProcesses, 1, false)
                    .with_source(MumuInstallSource::RunningProcess),
            )
        })?;
        return Ok(executable.clone());
    }

    let observed = bounded_process_diagnostics(&observed);
    let invalid = bounded_process_diagnostics(&invalid);
    Err(DeviceError::fatal(format!(
        "no running MuMu process matches target serial={target_serial} instance_id={target_instance_id}; observed=[{observed}]; invalid=[{invalid}]"
    )).with_nemu_resolution_context_if_absent(
        NemuResolutionContext::new(NemuResolutionReason::TargetProcessAbsent)
            .with_count(NemuResolutionCountKind::MatchedTargetProcesses, 0, false)
            .with_source(MumuInstallSource::RunningProcess),
    ))
}

fn target_mumu_instance_id(
    target_serial: &str,
    explicit_instance_id: Option<i32>,
) -> DeviceResult<u16> {
    let serial_instance_id = mumu_instance_id_from_serial(target_serial);
    let Some(explicit_instance_id) = explicit_instance_id else {
        return serial_instance_id.ok_or_else(|| {
            DeviceError::fatal(format!(
                "MuMu target identity has invalid topology: cannot derive an instance id from serial {target_serial}"
            )).with_nemu_resolution_context_if_absent(
                NemuResolutionContext::new(NemuResolutionReason::TargetIdentityUnavailable),
            )
        });
    };
    let explicit_instance_id = u16::try_from(explicit_instance_id).map_err(|_| {
        DeviceError::fatal(format!(
            "MuMu target identity has invalid explicit instance id {explicit_instance_id} for serial {target_serial}"
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::TargetIdentityInvalid),
        )
    })?;
    if let Some(serial_instance_id) = serial_instance_id
        && serial_instance_id != explicit_instance_id
    {
        return Err(DeviceError::fatal(format!(
            "MuMu target identity conflicts: serial {target_serial} resolves to instance_id={serial_instance_id} but explicit instance_id={explicit_instance_id}"
        )).with_nemu_resolution_context_if_absent(
            NemuResolutionContext::new(NemuResolutionReason::TargetIdentityMismatch),
        ));
    }
    Ok(explicit_instance_id)
}

fn mumu_instance_id_from_serial(serial: &str) -> Option<u16> {
    let (host, port) = serial.rsplit_once(':')?;
    let host = host.trim_matches(['[', ']']);
    if !host.eq_ignore_ascii_case("127.0.0.1")
        && !host.eq_ignore_ascii_case("localhost")
        && host != "::1"
    {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    let offset = port.checked_sub(MUMU_BASE_ADB_PORT)?;
    (offset % MUMU_PORT_STEP == 0).then_some(offset / MUMU_PORT_STEP)
}

fn bounded_process_diagnostics(entries: &[String]) -> String {
    let mut rendered = entries
        .iter()
        .take(MAX_MUMU_PROCESS_DIAGNOSTICS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if entries.len() > MAX_MUMU_PROCESS_DIAGNOSTICS {
        rendered.push_str(", ... additional entries omitted");
    }
    rendered
}

fn push_bounded_process_diagnostic(entries: &mut Vec<String>, entry: String) {
    if entries.len() <= MAX_MUMU_PROCESS_DIAGNOSTICS {
        entries.push(entry);
    }
}

fn mumu_instance_port(instance_id: u16) -> Option<u16> {
    MUMU_PORT_STEP
        .checked_mul(instance_id)
        .and_then(|offset| MUMU_BASE_ADB_PORT.checked_add(offset))
}

fn dedup_discovered_devices(devices: Vec<DiscoveredDevice>) -> Vec<DiscoveredDevice> {
    let mut output = Vec::new();
    for device in devices {
        if !output.iter().any(|existing: &DiscoveredDevice| {
            existing.serial == device.serial && existing.adb_path == device.adb_path
        }) {
            output.push(device);
        }
    }
    output
}

#[cfg(windows)]
fn system_processes() -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
    // Discovery is process-metadata only because mixed ADB servers can disturb MuMu instances.
    let script = r#"
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Get-CimInstance Win32_Process | ForEach-Object {
  $fields = @(
    [string]$_.ProcessId,
    [string]$_.Name,
    [string]$_.ExecutablePath,
    [string]$_.CommandLine
  ) | ForEach-Object { ($_ -replace "`r|`n|`t", " ") }
  [Console]::Out.WriteLine(($fields -join "`t"))
}
"#;
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| {
            root.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"));
    let output = Command::new(&powershell)
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .map_err(|err| {
            DeviceError::fatal(format!("failed to enumerate Windows processes: {err}"))
        })?;
    let stdout = String::from_utf8(output.stdout).map_err(|err| {
        DeviceError::fatal(format!(
            "Windows process enumeration produced non-UTF-8 stdout: {err}"
        ))
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(DeviceError::fatal(format!(
            "Windows process enumeration failed with {}\nstderr:\n{stderr}",
            output.status
        )));
    }
    parse_process_rows(&stdout)
}

#[cfg(not(windows))]
fn system_processes() -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
    Ok(Vec::new())
}

fn parse_process_rows(stdout: &str) -> DeviceResult<Vec<DeviceDiscoveryProcess>> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_process_row)
        .collect()
}

fn parse_process_row(line: &str) -> DeviceResult<DeviceDiscoveryProcess> {
    let mut fields = line.splitn(4, '\t');
    let process_id = fields
        .next()
        .ok_or_else(|| DeviceError::fatal("process row is missing process id"))?
        .trim()
        .parse()
        .map_err(|err| DeviceError::fatal(format!("invalid process id in row {line:?}: {err}")))?;
    let name = fields
        .next()
        .ok_or_else(|| DeviceError::fatal("process row is missing process name"))?
        .trim()
        .to_string();
    let executable_path = optional_process_field(fields.next())
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let command_line = optional_process_field(fields.next()).map(str::to_string);
    Ok(DeviceDiscoveryProcess {
        process_id,
        name,
        executable_path,
        command_line,
    })
}

fn optional_process_field(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn discovery_lists_running_mumu_serials() {
        let root = temp_mumu_root("lists-running");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![mumu_process(
            42,
            &executable,
            &format!("{} -v 2", executable.display()),
        )];

        let report = discover_mumu_devices_from_processes(&processes);
        let devices = report.devices;

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "127.0.0.1:16448");
        assert_eq!(devices[0].emulator, "mumu:2");
        assert_eq!(
            Path::new(&devices[0].adb_path),
            executable.parent().unwrap().join("adb.exe")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_skips_mumu_device_without_recoverable_instance_id() {
        let root = temp_mumu_root("no-recoverable-instance");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![mumu_process(
            7,
            &executable,
            &executable.display().to_string(),
        )];

        let report = discover_mumu_devices_from_processes(&processes);

        assert!(report.devices.is_empty());
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].process_id, 7);
        assert!(
            report.diagnostics[0]
                .message
                .contains("no recoverable instance id")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_accepts_explicit_dash_v_zero() {
        let root = temp_mumu_root("explicit-zero");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![mumu_process(
            8,
            &executable,
            &format!("{} -v 0", executable.display()),
        )];

        let report = discover_mumu_devices_from_processes(&processes);

        assert_eq!(report.devices.len(), 1);
        assert_eq!(report.devices[0].serial, "127.0.0.1:16384");
        assert_eq!(report.devices[0].emulator, "mumu:0");
        assert!(report.diagnostics.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_ignores_non_mumu_adb_processes() {
        let processes = vec![DeviceDiscoveryProcess {
            process_id: 9,
            name: "adb.exe".to_string(),
            executable_path: Some(PathBuf::from(r"C:\Android\platform-tools\adb.exe")),
            command_line: Some("adb version".to_string()),
        }];

        let report = discover_mumu_devices_from_processes(&processes);
        assert!(report.devices.is_empty());
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn discovery_deduplicates_same_mumu_instance() {
        let root = temp_mumu_root("dedup");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![
            mumu_process(1, &executable, &format!("{} -v 1", executable.display())),
            mumu_process(2, &executable, &format!("{} -v 1", executable.display())),
        ];

        let report = discover_mumu_devices_from_processes(&processes);
        let devices = report.devices;

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "127.0.0.1:16416");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_falls_back_to_existing_nx_main_adb_when_sibling_missing() {
        let root = temp_mumu_root("fallback-adb");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        let nx_main_adb = root.join("nx_main").join("adb.exe");
        fs::create_dir_all(nx_main_adb.parent().unwrap()).expect("nx_main");
        fs::write(&nx_main_adb, b"adb").expect("nx_main adb");
        let processes = vec![mumu_process(
            12,
            &executable,
            &format!("{} -v 3", executable.display()),
        )];

        let report = discover_mumu_devices_from_processes(&processes);
        let devices = report.devices;

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "127.0.0.1:16480");
        assert_eq!(Path::new(&devices[0].adb_path), nx_main_adb);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_prefers_existing_sibling_adb() {
        let root = temp_mumu_root("sibling-adb");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        let sibling_adb = executable.parent().unwrap().join("adb.exe");
        let nx_main_adb = root.join("nx_main").join("adb.exe");
        fs::create_dir_all(nx_main_adb.parent().unwrap()).expect("nx_main");
        fs::write(&sibling_adb, b"sibling").expect("sibling adb");
        fs::write(&nx_main_adb, b"nx_main").expect("nx_main adb");
        let processes = vec![mumu_process(
            13,
            &executable,
            &format!("{} -v 4", executable.display()),
        )];

        let report = discover_mumu_devices_from_processes(&processes);
        let devices = report.devices;

        assert_eq!(devices.len(), 1);
        assert_eq!(Path::new(&devices[0].adb_path), sibling_adb);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_recovers_mumu_player_instance_from_non_final_segment() {
        let root = temp_mumu_root("comment-instance");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![
            mumu_process(1, &executable, &format!("{} -v 0", executable.display())),
            mumu_process(
                2,
                &executable,
                &format!("{} --comment MuMuPlayer-3-primary", executable.display()),
            ),
        ];

        let report = discover_mumu_devices_from_processes(&processes);
        let devices = report.devices;

        assert_eq!(devices.len(), 2);
        assert!(
            devices
                .iter()
                .any(|device| device.serial == "127.0.0.1:16384")
        );
        assert!(
            devices
                .iter()
                .any(|device| device.serial == "127.0.0.1:16480")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_skips_invalid_dash_v_without_aliasing_to_zero() {
        let root = temp_mumu_root("invalid-instance");
        let executable = root
            .join("nx_device")
            .join("13.7")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![
            mumu_process(1, &executable, &format!("{} -v 0", executable.display())),
            mumu_process(2, &executable, &format!("{} -v abc", executable.display())),
        ];

        let report = discover_mumu_devices_from_processes(&processes);

        assert_eq!(report.devices.len(), 1);
        assert_eq!(report.devices[0].serial, "127.0.0.1:16384");
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].process_id, 2);
        assert!(
            report.diagnostics[0]
                .message
                .contains("invalid -v instance id")
        );
        let _ = fs::remove_dir_all(root);
    }

    // Task Contract: Workflow #256.
    // Test class: authorized Defect regression.
    #[test]
    fn target_identity_selects_only_its_running_mumu_process() {
        let first_root = temp_mumu_root("target-first");
        let second_root = temp_mumu_root("target-second");
        let first = first_root.join("nx_device/13.7/shell/MuMuNxDevice.exe");
        let second = second_root.join("nx_device/13.7/shell/MuMuNxDevice.exe");
        fs::write(&first, b"first").expect("first process executable");
        fs::write(&second, b"second").expect("second process executable");
        let processes = vec![
            mumu_process(21, &first, &format!("{} -v 1", first.display())),
            mumu_process(22, &second, &format!("{} -v 3", second.display())),
        ];

        let selected =
            running_mumu_executable_for_target_from_processes("127.0.0.1:16480", None, &processes)
                .expect("target process");

        assert_eq!(selected, second);
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn target_identity_rejects_ambiguous_running_processes() {
        let root = temp_mumu_root("target-ambiguous");
        let executable = root.join("nx_device/13.7/shell/MuMuNxDevice.exe");
        fs::write(&executable, b"process").expect("process executable");
        let processes = vec![
            mumu_process(31, &executable, &format!("{} -v 2", executable.display())),
            mumu_process(32, &executable, &format!("{} -v 2", executable.display())),
        ];

        let err =
            running_mumu_executable_for_target_from_processes("127.0.0.1:16448", None, &processes)
                .expect_err("duplicate target identity must fail");

        assert!(err.message().contains("ambiguous"));
        assert!(err.message().contains("process_id=31"));
        assert!(err.message().contains("process_id=32"));
        let _ = fs::remove_dir_all(root);
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn target_identity_distinguishes_no_match_from_invalid_topology() {
        let root = temp_mumu_root("target-no-match");
        let executable = root.join("nx_device/13.7/shell/MuMuNxDevice.exe");
        fs::write(&executable, b"process").expect("process executable");
        let no_match = vec![mumu_process(
            41,
            &executable,
            &format!("{} -v 4", executable.display()),
        )];
        let no_match_err =
            running_mumu_executable_for_target_from_processes("127.0.0.1:16416", None, &no_match)
                .expect_err("unmatched target must fail");
        assert!(
            no_match_err
                .message()
                .contains("no running MuMu process matches")
        );
        assert!(no_match_err.message().contains("instance_id=4"));

        let invalid = vec![DeviceDiscoveryProcess {
            process_id: 42,
            name: "MuMuNxDevice.exe".to_string(),
            executable_path: None,
            command_line: Some("MuMuNxDevice.exe -v 1".to_string()),
        }];
        let invalid_err =
            running_mumu_executable_for_target_from_processes("127.0.0.1:16416", None, &invalid)
                .expect_err("missing executable topology must fail");
        assert!(invalid_err.message().contains("invalid topology"));
        assert!(invalid_err.message().contains("process_id=42"));
        let _ = fs::remove_dir_all(root);
    }

    // Task Contract: Workflow #256.
    // Test class: specification criterion.
    #[test]
    fn target_identity_rejects_conflicting_explicit_instance() {
        let err = target_mumu_instance_id("127.0.0.1:16416", Some(2))
            .expect_err("conflicting target identity must fail");
        assert!(err.message().contains("target identity conflicts"));
    }

    #[test]
    fn parses_windows_process_rows() {
        let rows = "3896\tMuMuNxDevice.exe\tD:\\BST\\MuMuPlayer\\nx_device\\13.7\\shell\\MuMuNxDevice.exe\tD:\\BST\\MuMuPlayer\\nx_device\\13.7\\shell\\MuMuNxDevice.exe -v 1\n";

        let processes = parse_process_rows(rows).expect("process rows should parse");

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].process_id, 3896);
        assert_eq!(processes[0].name, "MuMuNxDevice.exe");
        assert_eq!(
            processes[0].executable_path.as_deref(),
            Some(Path::new(
                r"D:\BST\MuMuPlayer\nx_device\13.7\shell\MuMuNxDevice.exe"
            ))
        );
    }

    fn temp_mumu_root(label: &str) -> PathBuf {
        let index = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "actingcommand-device-discovery-{label}-{}-{index}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nx_device").join("13.7").join("shell"))
            .expect("device shell");
        root
    }

    fn mumu_process(
        process_id: u32,
        executable_path: &Path,
        command_line: &str,
    ) -> DeviceDiscoveryProcess {
        DeviceDiscoveryProcess {
            process_id,
            name: "MuMuNxDevice.exe".to_string(),
            executable_path: Some(executable_path.to_path_buf()),
            command_line: Some(command_line.to_string()),
        }
    }
}
