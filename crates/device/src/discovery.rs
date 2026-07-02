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

pub fn discover_devices() -> DeviceResult<Vec<DiscoveredDevice>> {
    let processes = system_processes()?;
    Ok(discover_mumu_devices_from_processes(&processes))
}

pub fn discover_mumu_devices_from_processes(
    processes: &[DeviceDiscoveryProcess],
) -> Vec<DiscoveredDevice> {
    let mut devices = processes
        .iter()
        .filter_map(mumu_device_from_process)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.serial.cmp(&right.serial));
    dedup_discovered_devices(devices)
}

fn mumu_device_from_process(process: &DeviceDiscoveryProcess) -> Option<DiscoveredDevice> {
    if !is_mumu_device_process(process) {
        return None;
    }
    let instance_id = mumu_instance_id(process)?;
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

fn mumu_instance_id(process: &DeviceDiscoveryProcess) -> Option<u16> {
    let command_line = process.command_line.as_deref().unwrap_or_default();
    parse_dash_v_instance(command_line)
        .or_else(|| parse_mumu_player_comment_instance(command_line))
        .or_else(|| Some(0).filter(|_| is_mumu_device_process(process)))
}

fn parse_dash_v_instance(command_line: &str) -> Option<u16> {
    let mut tokens = command_line.split_whitespace();
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case("-v") {
            return tokens.next()?.parse().ok();
        }
    }
    None
}

fn parse_mumu_player_comment_instance(command_line: &str) -> Option<u16> {
    command_line
        .split_whitespace()
        .find(|token| token.contains("MuMuPlayer-"))
        .and_then(|token| token.rsplit('-').next())
        .and_then(|suffix| {
            suffix
                .trim_matches(|ch: char| !ch.is_ascii_digit())
                .parse()
                .ok()
        })
}

fn mumu_process_adb_path(process: &DeviceDiscoveryProcess) -> Option<PathBuf> {
    let executable = process.executable_path.as_deref()?;
    let sibling_adb = executable.parent()?.join("adb.exe");
    if sibling_adb.file_name().is_some() {
        return Some(sibling_adb);
    }
    mumu_root_from_path(executable).and_then(|root| mumu_adb_candidates(&root).into_iter().next())
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

    #[test]
    fn discovery_lists_running_mumu_serials() {
        let processes = vec![mumu_process(
            42,
            r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe",
            r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe -v 2",
        )];

        let devices = discover_mumu_devices_from_processes(&processes);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "127.0.0.1:16448");
        assert_eq!(devices[0].emulator, "mumu:2");
        assert_eq!(
            devices[0].adb_path,
            r"D:\BST\MuMuPlayer\nx_device\12.0\shell\adb.exe"
        );
    }

    #[test]
    fn discovery_defaults_mumu_device_without_v_to_instance_zero() {
        let processes = vec![mumu_process(
            7,
            r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe",
            r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe",
        )];

        let devices = discover_mumu_devices_from_processes(&processes);

        assert_eq!(devices[0].serial, "127.0.0.1:16384");
        assert_eq!(devices[0].emulator, "mumu:0");
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
        let processes = vec![
            mumu_process(
                1,
                r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe",
                r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe -v 1",
            ),
            mumu_process(
                2,
                r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe",
                r"D:\BST\MuMuPlayer\nx_device\12.0\shell\MuMuNxDevice.exe -v 1",
            ),
        ];

        let devices = discover_mumu_devices_from_processes(&processes);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial, "127.0.0.1:16416");
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

    fn mumu_process(
        process_id: u32,
        executable_path: &'static str,
        command_line: &'static str,
    ) -> DeviceDiscoveryProcess {
        DeviceDiscoveryProcess {
            process_id,
            name: "MuMuNxDevice.exe".to_string(),
            executable_path: Some(PathBuf::from(executable_path)),
            command_line: Some(command_line.to_string()),
        }
    }
}
