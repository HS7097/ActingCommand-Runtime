// SPDX-License-Identifier: AGPL-3.0-only

use crate::{DeviceError, DeviceResult};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const ACTINGCOMMAND_ADB_PATH_ENV: &str = "ACTINGCOMMAND_ADB_PATH";
pub const ACTINGCOMMAND_NEMU_FOLDER_ENV: &str = "ACTINGCOMMAND_NEMU_FOLDER";

#[derive(Debug, Clone)]
pub struct AdbConfig {
    pub adb_path: String,
    pub command_timeout: Duration,
}

impl Default for AdbConfig {
    fn default() -> Self {
        Self {
            adb_path: resolve_adb_path(None)
                .map(|resolved| resolved.path)
                .unwrap_or_default(),
            command_timeout: Duration::from_secs(12),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAdbPath {
    pub path: String,
    pub source: AdbPathSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbPathSource {
    Environment,
    MumuDiscovery,
    UserConfig,
}

impl AdbPathSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "env:ACTINGCOMMAND_ADB_PATH",
            Self::MumuDiscovery => "mumu_discovery",
            Self::UserConfig => "user_config",
        }
    }
}

pub fn resolve_adb_path(configured: Option<&str>) -> DeviceResult<ResolvedAdbPath> {
    if let Some(path) = std::env::var_os(ACTINGCOMMAND_ADB_PATH_ENV).map(PathBuf::from) {
        return resolved_existing_adb(path, AdbPathSource::Environment);
    }
    if let Some(path) = discover_mumu_adb() {
        return resolved_existing_adb(path, AdbPathSource::MumuDiscovery);
    }
    if let Some(path) = configured.filter(|value| !value.trim().is_empty()) {
        return resolved_existing_adb(PathBuf::from(path), AdbPathSource::UserConfig);
    }
    Err(DeviceError::fatal(
        "ADB path is not configured. Set ACTINGCOMMAND_ADB_PATH, set ACTINGCOMMAND_NEMU_FOLDER to a MuMu folder, install MuMu at a known path, or configure actinglab adb_path. ActingCommand will not fall back to PATH adb because mixed adb versions can kill the MuMu server.",
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
    })
}

fn discover_mumu_adb() -> Option<PathBuf> {
    mumu_folder_candidates()
        .into_iter()
        .flat_map(|folder| mumu_adb_candidates(&folder))
        .find(|path| path.is_file())
}

fn mumu_folder_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    candidates.extend(std::env::var_os(ACTINGCOMMAND_NEMU_FOLDER_ENV).map(PathBuf::from));
    candidates.push(PathBuf::from(r"D:\BST\MuMuPlayer"));
    candidates.extend(
        ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .flat_map(|root| {
                let root = PathBuf::from(root);
                [
                    root.join("Netease").join("MuMu Player 12"),
                    root.join("Netease").join("MuMuPlayer-12.0"),
                    root.join("MuMuPlayer-12.0"),
                ]
            }),
    );
    dedup_paths(candidates)
}

fn mumu_adb_candidates(folder: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![folder.join("nx_main").join("adb.exe")];
    candidates.extend(mumu_nx_device_adb_candidates(folder));
    candidates
}

fn mumu_nx_device_adb_candidates(folder: &Path) -> Vec<PathBuf> {
    let nx_device = folder.join("nx_device");
    let Ok(entries) = std::fs::read_dir(nx_device) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("shell").join("adb.exe"))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut output = Vec::new();
    for path in paths {
        if !output.iter().any(|existing| existing == &path) {
            output.push(path);
        }
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
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
        match self.get_state(serial) {
            Ok(state) if state == "device" => Ok(state),
            first_state => {
                if !connect_allowed {
                    return Err(device_state_error(serial, first_state, None));
                }
                let connect_result = self.connect(serial).map(|_| ());
                let second_state = self.get_state(serial);
                match second_state {
                    Ok(state) if state == "device" => Ok(state),
                    state => Err(device_state_error(serial, state, Some(connect_result))),
                }
            }
        }
    }

    pub fn screen_size(&self, serial: &str) -> DeviceResult<String> {
        let output = self.run(&["-s", serial, "shell", "wm", "size"])?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn shell_input_tap(&self, serial: &str, x: i32, y: i32) -> DeviceResult<CommandOutput> {
        let x = x.to_string();
        let y = y.to_string();
        self.run(&["-s", serial, "shell", "input", "tap", &x, &y])
    }

    pub fn shell_input_swipe(
        &self,
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
        self.run(&[
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
        ])
    }

    pub fn force_stop(&self, serial: &str, package: &str) -> DeviceResult<CommandOutput> {
        self.run(&["-s", serial, "shell", "am", "force-stop", package])
    }

    pub fn launch_package(&self, serial: &str, package: &str) -> DeviceResult<CommandOutput> {
        self.run(&[
            "-s",
            serial,
            "shell",
            "monkey",
            "-p",
            package,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ])
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

    pub fn run(&self, args: &[&str]) -> DeviceResult<CommandOutput> {
        run_text_with_timeout(&self.config.adb_path, args, self.config.command_timeout)
    }
}

fn device_state_error(
    serial: &str,
    state: DeviceResult<String>,
    connect_result: Option<DeviceResult<()>>,
) -> DeviceError {
    let state_text = match state {
        Ok(state) => format!("state={state:?}"),
        Err(err) => format!("get-state failed: {err}"),
    };
    let connect_attempt_text = match connect_result {
        Some(Ok(())) => "; one adb connect was attempted".to_string(),
        Some(Err(err)) => format!("; one adb connect failed: {err}"),
        None => String::new(),
    };
    DeviceError::fatal(format!(
        "target device {serial} is not available in device state ({state_text}{connect_attempt_text})"
    ))
}

struct RawCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub fn run_text_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<CommandOutput> {
    validate_adb_path(adb_path)?;
    let output = run_raw_with_timeout(adb_path, args, timeout)?;
    let stdout = decode_adb_text(output.stdout, "stdout", args)?;
    let stderr = decode_adb_text(output.stderr, "stderr", args)?;
    if output.status.success() {
        return Ok(CommandOutput { stdout, stderr });
    }
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        args.join(" "),
        output.status
    )))
}

pub fn run_binary_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<BinaryOutput> {
    validate_adb_path(adb_path)?;
    let output = run_raw_with_timeout(adb_path, args, timeout)?;
    let stderr = decode_adb_text(output.stderr, "stderr", args)?;
    if output.status.success() {
        return Ok(BinaryOutput {
            stdout: output.stdout,
            stderr,
        });
    }
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout bytes: {}\nstderr:\n{stderr}",
        args.join(" "),
        output.status,
        output.stdout.len()
    )))
}

fn validate_adb_path(adb_path: &str) -> DeviceResult<()> {
    if adb_path.trim().is_empty() {
        return Err(DeviceError::fatal(
            "ADB path is unresolved. Set ACTINGCOMMAND_ADB_PATH or ACTINGCOMMAND_NEMU_FOLDER, or configure actinglab adb_path. ActingCommand intentionally does not fall back to PATH adb.",
        ));
    }
    Ok(())
}

fn run_raw_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<RawCommandOutput> {
    let mut child = Command::new(adb_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            DeviceError::fatal(format!("failed to spawn adb {}: {err}", args.join(" ")))
        })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        DeviceError::fatal(format!("failed to open adb {} stdout", args.join(" ")))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        DeviceError::fatal(format!("failed to open adb {} stderr", args.join(" ")))
    })?;
    let stdout_thread = spawn_pipe_reader(stdout);
    let stderr_thread = spawn_pipe_reader(stderr);

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|err| {
            DeviceError::fatal(format!(
                "failed to poll adb {} process: {err}",
                args.join(" ")
            ))
        })? {
            return collect_raw_output(status, stdout_thread, stderr_thread);
        }
        if started.elapsed() >= timeout {
            let stopped = stop_child(&mut child, Duration::from_millis(500));
            if !stopped {
                return Err(DeviceError::fatal(format!(
                    "adb {} timed out after {:?}; adb process did not exit after kill and pipe reader threads were detached to avoid a shutdown hang",
                    args.join(" "),
                    timeout
                )));
            }
            let stdout = join_pipe_reader(stdout_thread, "stdout")?;
            let stderr = join_pipe_reader(stderr_thread, "stderr")?;
            let stdout = String::from_utf8_lossy(&stdout);
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(DeviceError::fatal(format!(
                "adb {} timed out after {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                args.join(" "),
                timeout
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn collect_raw_output(
    status: ExitStatus,
    stdout_thread: JoinHandle<io::Result<Vec<u8>>>,
    stderr_thread: JoinHandle<io::Result<Vec<u8>>>,
) -> DeviceResult<RawCommandOutput> {
    Ok(RawCommandOutput {
        status,
        stdout: join_pipe_reader(stdout_thread, "stdout")?,
        stderr: join_pipe_reader(stderr_thread, "stderr")?,
    })
}

fn spawn_pipe_reader(mut reader: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe_reader(
    thread: JoinHandle<io::Result<Vec<u8>>>,
    stream_name: &str,
) -> DeviceResult<Vec<u8>> {
    thread
        .join()
        .map_err(|_| DeviceError::fatal(format!("adb {stream_name} reader thread panicked")))?
        .map_err(|err| DeviceError::fatal(format!("failed to read adb {stream_name}: {err}")))
}

fn decode_adb_text(bytes: Vec<u8>, stream_name: &str, args: &[&str]) -> DeviceResult<String> {
    String::from_utf8(bytes).map_err(|err| {
        DeviceError::fatal(format!(
            "adb {} produced non-UTF-8 {stream_name}: {err}",
            args.join(" ")
        ))
    })
}

pub fn stop_child(child: &mut Child, timeout: Duration) -> bool {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return true;
    }
    let _ = child.kill();
    let started = Instant::now();
    while started.elapsed() < timeout {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mumu_adb_candidates_prefer_nx_main_before_device_shells() {
        let folder = PathBuf::from(r"D:\BST\MuMuPlayer");
        let candidates = mumu_adb_candidates(&folder);

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

        assert!(err.to_string().contains("does not fall back to PATH adb"));
    }

    #[test]
    fn adb_path_source_labels_are_stable() {
        assert_eq!(
            AdbPathSource::Environment.as_str(),
            "env:ACTINGCOMMAND_ADB_PATH"
        );
        assert_eq!(AdbPathSource::MumuDiscovery.as_str(), "mumu_discovery");
        assert_eq!(AdbPathSource::UserConfig.as_str(), "user_config");
    }
}
