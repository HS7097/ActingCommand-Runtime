// SPDX-License-Identifier: AGPL-3.0-only

#[cfg(test)]
use crate::mumu::mumu_adb_candidates;
use crate::mumu::{
    MumuInstallSource, MumuInstallation, resolve_mumu_adb, resolve_mumu_installation,
};
use crate::{DeviceError, DeviceResult};
use std::io::{self, Read};
use std::path::PathBuf;
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
            // Discovery is fallible and must be requested through `resolve`.
            adb_path: String::new(),
            command_timeout: Duration::from_secs(12),
        }
    }
}

impl AdbConfig {
    pub fn resolve(configured: Option<&str>) -> DeviceResult<(Self, ResolvedAdbPath)> {
        let resolved = resolve_adb_path(configured)?;
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
            Self::MumuVendorEnumeration => "mumu_vendor_enumeration",
            Self::UserConfig => "user_config",
            Self::PathBaseline => "path_adb_baseline",
        }
    }
}

pub fn resolve_adb_path(configured: Option<&str>) -> DeviceResult<ResolvedAdbPath> {
    if let Some(path) = std::env::var_os(ACTINGCOMMAND_ADB_PATH_ENV).map(PathBuf::from) {
        return resolved_existing_adb(path, AdbPathSource::Environment);
    }
    if let Some(path) = configured.filter(|value| !value.trim().is_empty()) {
        return resolved_existing_adb(PathBuf::from(path), AdbPathSource::UserConfig);
    }
    let explicit_root = std::env::var_os(ACTINGCOMMAND_NEMU_FOLDER_ENV).map(PathBuf::from);
    let installation = resolve_mumu_installation(explicit_root)?;
    resolve_adb_path_after_discovery(installation, path_adb_candidate())
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
    Err(DeviceError::fatal(format!(
        "adb {} failed with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        args.join(" "),
        output.status,
        stdout = stdout.diagnostic_text(),
        stderr = stderr.diagnostic_text()
    )))
}

pub fn run_binary_with_timeout(
    adb_path: &str,
    args: &[&str],
    timeout: Duration,
) -> DeviceResult<BinaryOutput> {
    validate_adb_path(adb_path)?;
    let output = run_raw_with_timeout(adb_path, args, timeout)?;
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

struct DecodedAdbText {
    text: String,
    lossy: bool,
    stream_name: &'static str,
    command: String,
}

impl DecodedAdbText {
    fn diagnostic_text(&self) -> String {
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

fn decode_adb_text(bytes: Vec<u8>, stream_name: &'static str, args: &[&str]) -> DecodedAdbText {
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
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
            AdbPathSource::MumuVendorEnumeration.as_str(),
            "mumu_vendor_enumeration"
        );
        assert_eq!(AdbPathSource::UserConfig.as_str(), "user_config");
        assert_eq!(AdbPathSource::PathBaseline.as_str(), "path_adb_baseline");
    }

    #[test]
    fn join_pipe_reader_returns_fatal_error_when_reader_panics() {
        let reader = thread::spawn(|| -> io::Result<Vec<u8>> {
            panic!("injected reader panic");
        });

        let err = join_pipe_reader(reader, "stdout").expect_err("reader panic must be fatal");

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
        let original_path = std::env::var_os("PATH");
        let original_adb = std::env::var_os(ACTINGCOMMAND_ADB_PATH_ENV);
        let original_mumu = std::env::var_os(ACTINGCOMMAND_NEMU_FOLDER_ENV);
        let original_program_files = std::env::var_os("ProgramFiles");
        let original_program_files_x86 = std::env::var_os("ProgramFiles(x86)");
        let program_files = temp.join("program-files");
        let program_files_x86 = temp.join("program-files-x86");
        fs::create_dir_all(&program_files).unwrap();
        fs::create_dir_all(&program_files_x86).unwrap();
        unsafe {
            std::env::set_var("PATH", &temp);
            std::env::remove_var(ACTINGCOMMAND_ADB_PATH_ENV);
            std::env::remove_var(ACTINGCOMMAND_NEMU_FOLDER_ENV);
            std::env::set_var("ProgramFiles", &program_files);
            std::env::set_var("ProgramFiles(x86)", &program_files_x86);
        }

        let resolved = resolve_adb_path_after_discovery(None, path_adb_candidate())
            .expect("PATH adb baseline");

        unsafe {
            match original_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
            match original_adb {
                Some(value) => std::env::set_var(ACTINGCOMMAND_ADB_PATH_ENV, value),
                None => std::env::remove_var(ACTINGCOMMAND_ADB_PATH_ENV),
            }
            match original_mumu {
                Some(value) => std::env::set_var(ACTINGCOMMAND_NEMU_FOLDER_ENV, value),
                None => std::env::remove_var(ACTINGCOMMAND_NEMU_FOLDER_ENV),
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
        unsafe {
            std::env::set_var("PATH", path);
        }

        let candidate = path_adb_candidate();

        unsafe {
            match original_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = fs::remove_file(&adb);
        let _ = fs::remove_dir(&temp);

        if cfg!(windows) {
            assert!(candidate.is_none());
        } else {
            assert_eq!(candidate.as_deref(), Some(adb.as_path()));
        }
    }
}
