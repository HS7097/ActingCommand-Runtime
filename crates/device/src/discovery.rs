// SPDX-License-Identifier: AGPL-3.0-only

use crate::adb::mumu_adb_candidates;
use crate::{DeviceError, DeviceResult};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

const MUMU_BASE_ADB_PORT: u16 = 16_384;
const MUMU_PORT_STEP: u16 = 32;

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

pub fn discover_devices() -> DeviceResult<Vec<DiscoveredDevice>> {
    let processes = system_processes()?;
    Ok(discover_mumu_devices_from_processes(&processes))
}

pub fn discover_mumu_devices_from_processes(
    processes: &[DeviceDiscoveryProcess],
) -> Vec<DiscoveredDevice> {
    discover_mumu_devices_from_processes_with_diagnostics(processes).devices
}

pub fn discover_mumu_devices_from_processes_with_diagnostics(
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
    let adb_path = mumu_process_adb_path(process)?;
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
        Ok(None) => Ok(parse_mumu_player_comment_instance(command_line).unwrap_or(0)),
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

fn mumu_process_adb_path(process: &DeviceDiscoveryProcess) -> Option<PathBuf> {
    let executable = process.executable_path.as_deref()?;
    let sibling_adb = executable.parent()?.join("adb.exe");
    if sibling_adb.is_file() {
        return Some(sibling_adb);
    }
    mumu_root_from_path(executable).and_then(|root| {
        mumu_adb_candidates(&root)
            .into_iter()
            .find(|path| path.is_file())
    })
}

fn mumu_root_from_path(path: &Path) -> Option<PathBuf> {
    let path_text = path.to_string_lossy();
    split_before_marker(&path_text, r"\nx_device\")
        .or_else(|| split_before_marker(&path_text, "/nx_device/"))
        .or_else(|| split_before_marker(&path_text, r"\nx_main\"))
        .or_else(|| split_before_marker(&path_text, "/nx_main/"))
        .map(PathBuf::from)
}

fn split_before_marker(path: &str, marker: &str) -> Option<String> {
    let lower_path = path.to_ascii_lowercase();
    let lower_marker = marker.to_ascii_lowercase();
    lower_path
        .find(&lower_marker)
        .map(|index| path[..index].to_string())
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
    let output = Command::new("powershell.exe")
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
            .join("12.0")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![mumu_process(
            42,
            &executable,
            &format!("{} -v 2", executable.display()),
        )];

        let devices = discover_mumu_devices_from_processes(&processes);

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
    fn discovery_defaults_mumu_device_without_v_to_instance_zero() {
        let root = temp_mumu_root("defaults-zero");
        let executable = root
            .join("nx_device")
            .join("12.0")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![mumu_process(
            7,
            &executable,
            &executable.display().to_string(),
        )];

        let devices = discover_mumu_devices_from_processes(&processes);

        assert_eq!(devices[0].serial, "127.0.0.1:16384");
        assert_eq!(devices[0].emulator, "mumu:0");
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

        assert!(discover_mumu_devices_from_processes(&processes).is_empty());
    }

    #[test]
    fn discovery_deduplicates_same_mumu_instance() {
        let root = temp_mumu_root("dedup");
        let executable = root
            .join("nx_device")
            .join("12.0")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![
            mumu_process(1, &executable, &format!("{} -v 1", executable.display())),
            mumu_process(2, &executable, &format!("{} -v 1", executable.display())),
        ];

        let devices = discover_mumu_devices_from_processes(&processes);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "127.0.0.1:16416");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_falls_back_to_existing_nx_main_adb_when_sibling_missing() {
        let root = temp_mumu_root("fallback-adb");
        let executable = root
            .join("nx_device")
            .join("12.0")
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

        let devices = discover_mumu_devices_from_processes(&processes);

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
            .join("12.0")
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

        let devices = discover_mumu_devices_from_processes(&processes);

        assert_eq!(devices.len(), 1);
        assert_eq!(Path::new(&devices[0].adb_path), sibling_adb);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_recovers_mumu_player_instance_from_non_final_segment() {
        let root = temp_mumu_root("comment-instance");
        let executable = root
            .join("nx_device")
            .join("12.0")
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

        let devices = discover_mumu_devices_from_processes(&processes);

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
            .join("12.0")
            .join("shell")
            .join("MuMuNxDevice.exe");
        fs::write(executable.parent().unwrap().join("adb.exe"), b"adb").expect("sibling adb");
        let processes = vec![
            mumu_process(1, &executable, &format!("{} -v 0", executable.display())),
            mumu_process(2, &executable, &format!("{} -v abc", executable.display())),
        ];

        let report = discover_mumu_devices_from_processes_with_diagnostics(&processes);

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

    #[test]
    fn parses_windows_process_rows() {
        let rows = "3896\tMuMuNxDevice.exe\tD:\\BST\\MuMuPlayer\\nx_device\\12.0\\shell\\MuMuNxDevice.exe\tD:\\BST\\MuMuPlayer\\nx_device\\12.0\\shell\\MuMuNxDevice.exe -v 1\n";

        let processes = parse_process_rows(rows).expect("process rows should parse");

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].process_id, 3896);
        assert_eq!(processes[0].name, "MuMuNxDevice.exe");
        assert_eq!(
            processes[0].executable_path.as_deref(),
            Some(Path::new(
                r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe"
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
        fs::create_dir_all(root.join("nx_device").join("12.0").join("shell"))
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
