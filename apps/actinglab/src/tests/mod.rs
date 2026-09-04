use super::*;
#[path = "contained_semantic.rs"]
mod contained_semantic;
#[path = "semantic_fixture.rs"]
mod semantic_fixture;
#[path = "test_env.rs"]
mod test_env;
use actingcommand_contract::{IdentifierIssuer, InstanceId};
use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use semantic_fixture::{
    run_semantic_cli, seal_semantic_fixture, semantic_resource_root, synthetic_game_resource_root,
    template_drift_resource_root,
};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use test_env::TrustedRemoteEnvGuard;

static ENV_LOCK: Mutex<()> = Mutex::new(());
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_config_env(path: impl AsRef<Path>) {
    unsafe {
        env::set_var(CONFIG_ENV, path.as_ref());
    }
}

fn set_missing_config_env() {
    let path = env::temp_dir().join(format!(
        "actinglab-missing-config-{}-{}.json",
        std::process::id(),
        JSON_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    unsafe {
        env::set_var(CONFIG_ENV, path);
    }
}

struct RuntimeStateEnvGuard {
    previous: Option<OsString>,
}

impl Drop for RuntimeStateEnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.take() {
                env::set_var(RUNTIME_STATE_ROOT_ENV, previous);
            } else {
                env::remove_var(RUNTIME_STATE_ROOT_ENV);
            }
        }
    }
}

struct AuthoringRuntimeProvider {
    instance_id: InstanceId,
}

impl ExecutionBackendProvider for AuthoringRuntimeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["ak".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "ak")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<authoring-test>"))
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        Err(DeviceError::fatal(
            "resource authoring must not open an input backend",
        ))
    }

    fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        Err(DeviceError::fatal(
            "resource authoring must not open a capture backend",
        ))
    }

    fn control_application(
        &self,
        _instance_alias: &str,
        _action: actingcommand_contract::ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "resource authoring must not control applications",
        ))
    }
}

fn use_runtime_state_root(path: &Path) -> RuntimeStateEnvGuard {
    let previous = env::var_os(RUNTIME_STATE_ROOT_ENV);
    unsafe {
        env::set_var(RUNTIME_STATE_ROOT_ENV, path);
    }
    RuntimeStateEnvGuard { previous }
}

fn start_authoring_runtime(state_root: &Path) -> RuntimeHost {
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    RuntimeHost::start(
        RuntimeHostConfig::new(state_root, b"actinglab-resource-authoring-test"),
        Arc::new(AuthoringRuntimeProvider { instance_id }),
    )
    .expect("Runtime host")
}

fn prepare_promotable_record(config: &Path, state_dir: &Path, frame_path: &Path) {
    fs::write(frame_path, test_record_frame_png(12, 10)).expect("record frame");
    set_config_env(config);
    let start = run_cli(
        [
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "start",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--task-id",
            "daily-check",
        ],
        true,
    );
    let home_anchor = run_cli(
        [
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "step",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--kind",
            "anchor",
            "--step-id",
            "home-anchor",
            "--id",
            "page/home",
            "--region",
            "2,3,4,5",
            "--frame",
            frame_path.to_str().unwrap(),
        ],
        true,
    );
    let mail_anchor = run_cli(
        [
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "step",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--kind",
            "anchor",
            "--step-id",
            "mail-anchor",
            "--id",
            "page/mail",
            "--region",
            "2,3,4,5",
            "--frame",
            frame_path.to_str().unwrap(),
        ],
        true,
    );
    let operation = run_cli(
        [
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "step",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--kind",
            "operation",
            "--step-id",
            "home-to-mail",
            "--from",
            "page/home",
            "--to",
            "page/mail",
            "--click",
            "5,6",
        ],
        true,
    );
    for result in [start, home_anchor, mail_anchor, operation] {
        assert_eq!(
            result.exit_code(),
            0,
            "{}",
            serde_json::to_string_pretty(&result.envelope).unwrap()
        );
    }
}

fn set_isolated_app_env() -> TempDir {
    let temp = TempDir::new().unwrap();
    unsafe {
        env::set_var("LOCALAPPDATA", temp.path());
        env::set_var("APPDATA", temp.path());
    }
    temp
}

fn user_config_with_test_adb() -> (TempDir, UserConfig) {
    let temp = tempfile::tempdir().unwrap();
    let adb_name = if cfg!(windows) { "adb.exe" } else { "adb" };
    let adb_path = temp.path().join(adb_name);
    fs::write(&adb_path, b"test adb placeholder").unwrap();
    (
        temp,
        UserConfig {
            adb_path: Some(adb_path.to_string_lossy().to_string()),
            ..Default::default()
        },
    )
}

fn path_baseline_adb() -> actingcommand_device::ResolvedAdbPath {
    actingcommand_device::ResolvedAdbPath {
        path: "test-adb".to_string(),
        source: AdbPathSource::PathBaseline,
        warning: Some("WARNING: using PATH adb as a non-MuMu baseline channel".to_string()),
    }
}

#[test]
fn tests_mutate_config_env_only_through_fixture_helpers() {
    let source = include_str!("mod.rs");
    assert_eq!(
        source
            .matches(concat!("env::set", "_var(CONFIG_ENV"))
            .count(),
        2
    );
    assert!(!source.contains(concat!("env::remove", "_var(CONFIG_ENV")));
}

fn create_test_dir_alias(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (link, target);
        false
    }
}

fn test_record_frame_png(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[x as u8, y as u8, 128, 255]);
        }
    }
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgba8,
        CaptureBackendName::AdbScreencap,
    )
    .expect("test frame")
    .png_for_artifact()
    .expect("test frame png")
}

fn drift_test_record(steps: Value) -> SessionRecordContext {
    serde_json::from_value(json!({
        "schema_version": "session.record.v0.1",
        "record_id": "record-1",
        "task_id": "daily-check",
        "instance": "ak",
        "status": "recording",
        "started_at_unix_ms": 1,
        "updated_at_unix_ms": 2,
        "steps": steps
    }))
    .expect("drift test record")
}

fn drift_test_anchor_step(step_id: &str, id: &str) -> Value {
    json!({
        "schema_version": "session.record_step.v0.1",
        "step_id": step_id,
        "created_at_unix_ms": 1,
        "updated_at_unix_ms": 2,
        "kind": "anchor",
        "id": id,
        "region": {"mode": "rect", "rect": {"x": 1, "y": 2, "width": 3, "height": 4}},
        "color_check": false,
        "evaluation": {
            "status": "deferred",
            "reason": "synthetic"
        }
    })
}

fn test_contrast_record_frame_png(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                ((x * 37 + y * 17 + 91) % 256) as u8,
                ((x * 13 + y * 53 + 7) % 256) as u8,
                ((x * 97 + y * 11 + 3) % 256) as u8,
                255,
            ]);
        }
    }
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgba8,
        CaptureBackendName::AdbScreencap,
    )
    .expect("test contrast frame")
    .png_for_artifact()
    .expect("test contrast frame png")
}

fn test_auto_region_discrimination_frame_png(contrast: bool) -> Vec<u8> {
    let width = 12;
    let height = 9;
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let in_top_left = x < 4 && y < 3;
            let in_center = (4..8).contains(&x) && (3..6).contains(&y);
            let checker = if (x + y) % 2 == 0 { 240 } else { 40 };
            let value = if in_top_left {
                checker
            } else if in_center && !contrast {
                255 - checker
            } else {
                72
            };
            pixels.extend_from_slice(&[value, value, value, 255]);
        }
    }
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgba8,
        CaptureBackendName::AdbScreencap,
    )
    .expect("test auto region frame")
    .png_for_artifact()
    .expect("test auto region frame png")
}

#[test]
fn version_outputs_json_envelope() {
    let result = run_cli(["--json", "--version"], true);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert!(result.envelope.ok);
    assert_eq!(result.envelope.command, "version");
}

#[test]
fn status_without_runtime_is_exit_five() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));

    let result = run_cli(["--json", "status"], true);
    set_missing_config_env();

    assert_eq!(result.exit_code(), 5);
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "runtime_not_running"
    );
}

include!("runtime_transport.rs");

include!("cli_basics.rs");

include!("session_record.rs");

#[test]
fn session_record_start_requires_task_id() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let config = temp.path().join("config.json");
    let state_dir = temp.path().join("session");
    set_config_env(&config);

    let result = run_cli(
        [
            "--json",
            "--instance",
            "ak",
            "session",
            "record",
            "start",
            "--state-dir",
            state_dir.to_str().unwrap(),
        ],
        true,
    );
    set_missing_config_env();

    assert_eq!(result.exit_code(), 2);
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "validation_failed"
    );
}

#[test]
fn session_instance_list_reads_config() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let config = temp.path().join("config.json");
    set_config_env(&config);

    let _ = run_cli(
        [
            "--json",
            "config",
            "set",
            "instance.azur.serial",
            "127.0.0.1:16385",
        ],
        true,
    );
    let _ = run_cli(
        ["--json", "config", "set", "instance.azur.game", "azurlane"],
        true,
    );
    let _ = run_cli(
        ["--json", "config", "set", "instance.azur.server", "jp"],
        true,
    );
    let _ = run_cli(
        [
            "--json",
            "config",
            "set",
            "instance.azur.adb_path",
            "C:\\Tools\\adb.exe",
        ],
        true,
    );
    let _ = run_cli(
        [
            "--json",
            "config",
            "set",
            "instance.azur.capture_backend",
            "droidcast_raw",
        ],
        true,
    );
    let result = run_cli(["--json", "session", "instance", "list"], true);
    set_missing_config_env();

    assert_eq!(result.exit_code(), 0);
    let instances = result
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("instances")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].get("id").and_then(Value::as_str), Some("azur"));
    assert_eq!(
        instances[0].get("adb_path").and_then(Value::as_str),
        Some("C:\\Tools\\adb.exe")
    );
    assert_eq!(
        instances[0].get("capture_backend").and_then(Value::as_str),
        Some("droidcast_raw")
    );
}

#[test]
fn session_instance_registry_reports_contract() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    set_config_env(&config_path);

    let mut config = UserConfig::default();
    config.instances.insert(
        "ak-b".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16416".to_string()),
            game: Some("ark".to_string()),
            server: Some("cn-bilibili".to_string()),
            package: Some("com.hypergryph.arknights.bilibili".to_string()),
            adb_path: Some("C:\\Tools\\adb.exe".to_string()),
            capture_backend: Some("nemu_ipc".to_string()),
            touch_backend: None,
        },
    );
    config.instances.insert(
        "ba-jp".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16384".to_string()),
            game: Some("ba".to_string()),
            server: None,
            package: None,
            adb_path: None,
            capture_backend: None,
            touch_backend: None,
        },
    );
    write_user_config(&config).unwrap();

    let result = run_cli(["--json", "session", "instance", "registry"], true);
    set_missing_config_env();

    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.get("schema_version").and_then(Value::as_str),
        Some("session.instance_registry.v0.1")
    );
    assert_eq!(data.get("count").and_then(Value::as_u64), Some(2));
    assert_eq!(
        data.pointer("/capture_backends/2").and_then(Value::as_str),
        Some("droidcast_raw")
    );
    let instances = data.get("instances").and_then(Value::as_array).unwrap();
    let ak = instances
        .iter()
        .find(|instance| instance.get("id").and_then(Value::as_str) == Some("ak-b"))
        .unwrap();
    assert_eq!(
        ak.pointer("/effective/capture_backend")
            .and_then(Value::as_str),
        Some("nemu_ipc")
    );
    assert_eq!(
        ak.pointer("/validation/ready_for_device_control")
            .and_then(Value::as_bool),
        Some(true)
    );
    let ba = instances
        .iter()
        .find(|instance| instance.get("id").and_then(Value::as_str) == Some("ba-jp"))
        .unwrap();
    assert_eq!(
        ba.pointer("/effective/capture_backend")
            .and_then(Value::as_str),
        Some("auto")
    );
    assert_eq!(
        ba.pointer("/validation/ready_for_device_control")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        ba.pointer("/validation/missing_required_fields")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|field| field.as_str() == Some("server"))
    );
}

#[test]
fn session_instance_registry_rejects_invalid_configured_backend() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let config_path = temp.path().join("config.json");
    set_config_env(&config_path);

    let mut config = UserConfig::default();
    config.instances.insert(
        "ak-b".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16416".to_string()),
            game: Some("ark".to_string()),
            server: Some("cn-bilibili".to_string()),
            package: None,
            adb_path: None,
            capture_backend: Some("not-a-backend".to_string()),
            touch_backend: None,
        },
    );
    write_user_config(&config).unwrap();

    let result = run_cli(["--json", "session", "instance", "registry"], true);
    set_missing_config_env();

    assert_eq!(result.exit_code(), 2);
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "validation_failed"
    );
    assert!(
        result
            .envelope
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("invalid instance.ak-b.capture_backend")
    );
}

#[test]
fn capabilities_are_offline() {
    let result = run_cli(["--json", "capabilities"], true);
    assert_eq!(result.exit_code(), 0);
    assert!(
        result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("commands")
            .is_some()
    );
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.pointer("/session_layer/schema_version")
            .and_then(Value::as_str),
        Some("session.capabilities.v0.1")
    );
    assert_eq!(
        data.pointer("/session_layer/access_channels/1/id")
            .and_then(Value::as_str),
        Some("trusted_remote")
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request capabilities"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request contract"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request api"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str) == Some("session queue"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request queue"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session submit-plan"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session bootstrap"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request bootstrap"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request submit-plan"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session validation-plan"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request validation-plan"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request events"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session transport"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session request transport"))
    );
    assert!(
        data.get("commands")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|command| command.get("command").and_then(Value::as_str)
                == Some("session instance registry"))
    );
    let retired = data
        .get("commands")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|command| {
            command.get("command").and_then(Value::as_str) == Some("session instance keep-alive")
        })
        .expect("retired command remains discoverable");
    assert_eq!(
        retired.get("status").and_then(Value::as_str),
        Some("retired")
    );
}

#[test]
fn session_contract_is_offline_access_contract() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(REQUIRE_SESSION_DAEMON_ENV);
    }
    let result = run_cli(["--json", "session", "contract"], true);
    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.get("schema_version").and_then(Value::as_str),
        Some("session.access.v0.1")
    );
    assert_eq!(
        data.pointer("/entrypoints/local_cli/status")
            .and_then(Value::as_str),
        Some("available")
    );
    assert_eq!(
        data.pointer("/entrypoints/trusted_remote/authentication_required")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/entrypoints/trusted_remote/auth_env/token")
            .and_then(Value::as_str),
        Some(TRUSTED_REMOTE_TOKEN_ENV)
    );
    assert_eq!(
        data.pointer("/safety/control_requests_require_matching_lease")
            .and_then(Value::as_bool),
        Some(true)
    );
    let control_examples = data
        .pointer("/request_classes/control/examples")
        .and_then(Value::as_array)
        .unwrap();
    assert!(
        control_examples
            .iter()
            .any(|item| { item.as_str() == Some("stream --input-event <action,args>") })
    );
    assert!(
        control_examples
            .iter()
            .any(|item| { item.as_str() == Some("stream --relay-event <action,args>") })
    );
    assert_eq!(
        data.pointer("/daemon_queries/bootstrap")
            .and_then(Value::as_str),
        Some("session request bootstrap")
    );
    assert_eq!(
        data.pointer("/daemon_queries/throat_policy")
            .and_then(Value::as_str),
        Some("session request throat-policy")
    );
    assert_eq!(
        data.pointer("/daemon_queries/capture_policy")
            .and_then(Value::as_str),
        Some("session request capture-policy")
    );
    assert_eq!(
        data.pointer("/daemon_queries/self_heal_policy")
            .and_then(Value::as_str),
        Some("session request self-heal-policy")
    );
    assert_eq!(
        data.pointer("/daemon_queries/api").and_then(Value::as_str),
        Some("session request api")
    );
    assert_eq!(
        data.pointer("/daemon_queries/queue")
            .and_then(Value::as_str),
        Some("session request queue")
    );
    assert_eq!(
        data.pointer("/daemon_queries/submit_plan")
            .and_then(Value::as_str),
        Some("session request submit-plan <command...>")
    );
    assert_eq!(
        data.pointer("/daemon_queries/validation_plan")
            .and_then(Value::as_str),
        Some("session request validation-plan")
    );
    assert_eq!(
        data.pointer("/daemon_queries/phase_c_plan")
            .and_then(Value::as_str),
        Some("session request phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]")
    );
    assert_eq!(
        data.pointer("/daemon_queries/transport")
            .and_then(Value::as_str),
        Some("session request transport")
    );
}

#[test]
fn session_api_is_offline_api_contract() {
    let result = run_cli(["--json", "session", "api"], true);
    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.get("schema_version").and_then(Value::as_str),
        Some("session.api.v0.1")
    );
    assert_eq!(
        data.pointer("/access_channels/local_cli/status")
            .and_then(Value::as_str),
        Some("available")
    );
    assert_eq!(
        data.pointer("/access_channels/trusted_remote/authentication_required")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/access_channels/trusted_remote/network_listener_implemented")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        data.pointer("/access_channels/trusted_remote/blocked_without_auth_code")
            .and_then(Value::as_str),
        Some("trusted_remote_auth_required")
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/submit_modes/no_wait/flag")
            .and_then(Value::as_str),
        Some("--no-wait")
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/submit_modes/no_wait/waits_for_acknowledgement")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/submit_modes/no_wait/ack_timeout_flag")
            .and_then(Value::as_str),
        Some("--request-ack-timeout-ms")
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/cancel_query")
            .and_then(Value::as_str),
        Some("session request cancel <request-id> [--reason text] [--dry-run]")
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/cancel_records_journal")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/cancel_dry_run_preserves_queue")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/admission_gate/error_code")
            .and_then(Value::as_str),
        Some("request_queue_needs_attention")
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/admission_gate/preflight_command")
            .and_then(Value::as_str),
        Some("session command-check <command...>")
    );
    assert_eq!(
        data.pointer("/envelopes/command_check_view/throat_gate_field")
            .and_then(Value::as_str),
        Some("throat_gate")
    );
    assert_eq!(
        data.pointer("/envelopes/command_check_view/phase_c_scope_field")
            .and_then(Value::as_str),
        Some("phase_c_scope")
    );
    assert_eq!(
        data.pointer("/envelopes/command_check_view/phase_c_scope_schema_version")
            .and_then(Value::as_str),
        Some("session.command_phase_c_scope.v0.1")
    );
    let api_control_examples = data
        .pointer("/command_classes/control/examples")
        .and_then(Value::as_array)
        .unwrap();
    assert!(
        api_control_examples
            .iter()
            .any(|item| { item.as_str() == Some("stream --input-event <action,args>") })
    );
    assert!(
        api_control_examples
            .iter()
            .any(|item| { item.as_str() == Some("stream --relay-event <action,args>") })
    );
    let api_readonly_device_examples = data
        .pointer("/command_classes/read_only/device_affecting_examples")
        .and_then(Value::as_array)
        .unwrap();
    assert!(
        api_readonly_device_examples
            .iter()
            .any(|item| item.as_str() == Some("session record step --current-frame"))
    );
    let api_daemon_examples = data
        .pointer("/command_classes/daemon_state/examples")
        .and_then(Value::as_array)
        .unwrap();
    assert!(
        api_daemon_examples
            .iter()
            .any(|item| item.as_str() == Some("session record step --frame <png>"))
    );
    assert_eq!(
        data.pointer("/envelopes/record_policy_view/schema_version")
            .and_then(Value::as_str),
        Some("session.record_policy.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/record_policy_view/authorization_model_field")
            .and_then(Value::as_str),
        Some("authorization_model")
    );
    assert_eq!(
        data.pointer("/envelopes/record_policy_view/allowed_step_kinds_field")
            .and_then(Value::as_str),
        Some("allowed_step_kinds")
    );
    assert_eq!(
        data.pointer("/envelopes/record_policy_view/does_not_write_resource_repositories")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/daemon_request_queue/submit_modes/sync_wait/consumes_response_on_success")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/list_schema_version")
            .and_then(Value::as_str),
        Some("session.lease_list.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/list_query")
            .and_then(Value::as_str),
        Some("session lease list [--holder <id>] [--lease-id <id>]")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/daemon_list_query")
            .and_then(Value::as_str),
        Some("session request lease list [--holder <id>] [--lease-id <id>]")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/freshness_field")
            .and_then(Value::as_str),
        Some("freshness")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/freshness_stale_after_ms")
            .and_then(Value::as_u64),
        Some(SESSION_LEASE_STALE_MS)
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/status_schema_version")
            .and_then(Value::as_str),
        Some("session.lease_status.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/touch_schema_version")
            .and_then(Value::as_str),
        Some("session.lease_touch.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/touch_query")
            .and_then(Value::as_str),
        Some("session lease touch [--holder <id>] [--lease-id <id>]")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/daemon_touch_query")
            .and_then(Value::as_str),
        Some("session request lease touch [--holder <id>] [--lease-id <id>]")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/touch_requires_matching_holder")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/wait_schema_version")
            .and_then(Value::as_str),
        Some("session.lease_wait.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/wait_query")
            .and_then(Value::as_str),
        Some(
            "session lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]"
        )
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/daemon_wait_query")
            .and_then(Value::as_str),
        Some(
            "session request lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]"
        )
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/wait_default_status")
            .and_then(Value::as_str),
        Some("free")
    );
    assert_eq!(
        data.pointer("/envelopes/lease_view/wait_timeout_returns_current_state")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/schema_version")
            .and_then(Value::as_str),
        Some("session.events.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/wait_query")
            .and_then(Value::as_str),
        Some("session events wait [--timeout-ms N] [--poll-ms N]")
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/wait_timeout_default_ms")
            .and_then(Value::as_u64),
        Some(SESSION_DAEMON_REQUEST_TIMEOUT_MS)
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/wait_poll_default_ms")
            .and_then(Value::as_u64),
        Some(100)
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/wait_timeout_returns_empty_events")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/filters/1")
            .and_then(Value::as_str),
        Some("--after-unix-ms")
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/cursor_fields/1")
            .and_then(Value::as_str),
        Some("next_after_unix_ms")
    );
    assert_eq!(
        data.pointer("/envelopes/event_view/cursor_fields/3")
            .and_then(Value::as_str),
        Some("next_after_request_id")
    );
    assert_eq!(
        data.pointer("/envelopes/response_view/schema_version")
            .and_then(Value::as_str),
        Some("session.response.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/response_view/delete_after_successful_parse")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/response_view/wait_query")
            .and_then(Value::as_str),
        Some("session response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]")
    );
    assert_eq!(
        data.pointer("/envelopes/response_view/wait_timeout_default_ms")
            .and_then(Value::as_u64),
        Some(SESSION_DAEMON_REQUEST_TIMEOUT_MS)
    );
    assert_eq!(
        data.pointer("/envelopes/response_view/wait_poll_default_ms")
            .and_then(Value::as_u64),
        Some(100)
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/schema_version")
            .and_then(Value::as_str),
        Some("session.request_state.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/list_schema_version")
            .and_then(Value::as_str),
        Some("session.request_state_list.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/wait_query")
            .and_then(Value::as_str),
        Some(
            "session request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]"
        )
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/daemon_wait_query")
            .and_then(Value::as_str),
        Some(
            "session request request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]"
        )
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/wait_default_statuses/0")
            .and_then(Value::as_str),
        Some("response_available")
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/wait_timeout_default_ms")
            .and_then(Value::as_u64),
        Some(SESSION_DAEMON_REQUEST_TIMEOUT_MS)
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/wait_timeout_returns_current_state")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/daemon_list_query")
            .and_then(Value::as_str),
        Some(
            "session request request-state list [--limit N] [--status <state>] [--lease-holder <id>]"
        )
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/list_global_filters/0")
            .and_then(Value::as_str),
        Some("--instance")
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/lease_holder_filter_repeats")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/statuses/1")
            .and_then(Value::as_str),
        Some("running")
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/statuses/2")
            .and_then(Value::as_str),
        Some("response_available")
    );
    assert_eq!(
        data.pointer("/envelopes/request_state_view/state_sources/3")
            .and_then(Value::as_str),
        Some("request-journal")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/schema_version")
            .and_then(Value::as_str),
        Some("session.transport.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/check_query")
            .and_then(Value::as_str),
        Some("session transport check --endpoint <url>")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/plan_query")
            .and_then(Value::as_str),
        Some("session transport plan [--endpoint <url>]")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/plan_schema_version")
            .and_then(Value::as_str),
        Some("session.transport_plan.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/plan_next_actions_field")
            .and_then(Value::as_str),
        Some("next_actions")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/plan_trusted_remote_gate_field")
            .and_then(Value::as_str),
        Some("trusted_remote_gate")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/plan_trusted_remote_gate_schema_version")
            .and_then(Value::as_str),
        Some("session.trusted_remote_gate.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/transport_view/check_schema_version")
            .and_then(Value::as_str),
        Some("session.transport_check.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/validation_plan_view/pending_live_acceptance_field")
            .and_then(Value::as_str),
        Some("pending_live_acceptance")
    );
    assert_eq!(
        data.pointer("/envelopes/validation_plan_view/phase_acceptance_matrix_field")
            .and_then(Value::as_str),
        Some("phase_acceptance_matrix")
    );
    assert_eq!(
        data.pointer("/envelopes/validation_plan_view/next_actions_field")
            .and_then(Value::as_str),
        Some("next_actions")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/schema_version")
            .and_then(Value::as_str),
        Some("session.bootstrap.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_field")
            .and_then(Value::as_str),
        Some("status_diagnostics")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_capture_freshness_field")
            .and_then(Value::as_str),
        Some("status_diagnostics.capture_freshness")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_self_heal_field")
            .and_then(Value::as_str),
        Some("status_diagnostics.self_heal")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_interaction_flow_field")
            .and_then(Value::as_str),
        Some("status_diagnostics.interaction_flow")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_trusted_channel_field")
            .and_then(Value::as_str),
        Some("status_diagnostics.trusted_channel")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_phase_c_field")
            .and_then(Value::as_str),
        Some("status_diagnostics.phase_c")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/status_diagnostics_validation_field")
            .and_then(Value::as_str),
        Some("status_diagnostics.validation")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/validation_plan_field")
            .and_then(Value::as_str),
        Some("validation_plan")
    );
    assert_eq!(
        data.pointer("/envelopes/bootstrap_view/throat_policy_field")
            .and_then(Value::as_str),
        Some("throat_policy")
    );
    assert_eq!(
        data.pointer("/envelopes/throat_policy_view/schema_version")
            .and_then(Value::as_str),
        Some("session.throat_policy.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/throat_policy_view/only_control_throat_field")
            .and_then(Value::as_str),
        Some("session_layer.only_control_throat")
    );
    assert_eq!(
        data.pointer("/envelopes/capture_policy_view/schema_version")
            .and_then(Value::as_str),
        Some("session.capture_policy.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/capture_policy_view/stale_classification_field")
            .and_then(Value::as_str),
        Some("stale_classification")
    );
    assert_eq!(
        data.pointer("/envelopes/capture_policy_view/freeze_classification_gate_field")
            .and_then(Value::as_str),
        Some("freeze_classification_gate")
    );
    assert_eq!(
        data.pointer("/envelopes/capture_policy_view/freeze_classification_gate_schema_version")
            .and_then(Value::as_str),
        Some("session.capture_freeze_classification_gate.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/self_heal_policy_view/schema_version")
            .and_then(Value::as_str),
        Some("session.self_heal_policy.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/self_heal_policy_view/maintenance_boundary_field")
            .and_then(Value::as_str),
        Some("maintenance_boundary")
    );
    assert_eq!(
        data.pointer("/envelopes/self_heal_plan_view/next_actions_field")
            .and_then(Value::as_str),
        Some("next_actions")
    );
    assert_eq!(
        data.pointer("/envelopes/self_heal_plan_view/execution_gate_field")
            .and_then(Value::as_str),
        Some("execution_gate")
    );
    assert_eq!(
        data.pointer("/envelopes/self_heal_plan_view/execution_gate_schema_version")
            .and_then(Value::as_str),
        Some("session.self_heal_execution_gate.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/status_view/queue_health_actions/1")
            .and_then(Value::as_str),
        Some("blocked_request_cancel_dry_run")
    );
    assert_eq!(
        data.pointer("/envelopes/status_view/queue_health_actions/5")
            .and_then(Value::as_str),
        Some("unclaimed_response_read")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/queues_field")
            .and_then(Value::as_str),
        Some("queues")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/queue_health_field")
            .and_then(Value::as_str),
        Some("queues.health")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/instances_field")
            .and_then(Value::as_str),
        Some("instances")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/instance_status_field")
            .and_then(Value::as_str),
        Some("instances.status")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/selected_instance_status_field")
            .and_then(Value::as_str),
        Some("instances.selected_status")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/selected_instance_missing_required_field")
            .and_then(Value::as_str),
        Some("instances.selected_missing_required")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/policy_summary_field")
            .and_then(Value::as_str),
        Some("policy_summary")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/policy_summary_schema_version")
            .and_then(Value::as_str),
        Some("session.readiness_policy_summary.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/diagnostics_summary_field")
            .and_then(Value::as_str),
        Some("diagnostics_summary")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/diagnostics_summary_schema_version")
            .and_then(Value::as_str),
        Some("session.readiness_diagnostics_summary.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/phase_c_summary_field")
            .and_then(Value::as_str),
        Some("diagnostics_summary.phase_c")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/phase_c_acceptance_gates_schema_version_field")
            .and_then(Value::as_str),
        Some("diagnostics_summary.phase_c.acceptance_gates_schema_version")
    );
    assert_eq!(
        data.pointer("/envelopes/readiness_view/phase_c_acceptance_gate_lane_count_field")
            .and_then(Value::as_str),
        Some("diagnostics_summary.phase_c.acceptance_gate_lane_count")
    );
    assert_eq!(
        data.pointer("/envelopes/connect_plan_view/schema_version")
            .and_then(Value::as_str),
        Some("session.connect_plan.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/connect_plan_view/stream_preflight_field")
            .and_then(Value::as_str),
        Some("stream_preflight")
    );
    assert_eq!(
        data.pointer("/envelopes/connect_plan_view/phase_c_preflight_field")
            .and_then(Value::as_str),
        Some("phase_c_preflight")
    );
    assert_eq!(
        data.pointer("/envelopes/connect_plan_view/phase_c_preflight_schema_version")
            .and_then(Value::as_str),
        Some("session.connect_phase_c_preflight.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/connect_plan_view/next_actions_field")
            .and_then(Value::as_str),
        Some("next_actions")
    );
    assert_eq!(
        data.pointer("/envelopes/connect_plan_view/does_not_start_listener")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/stream_plan_view/schema_version")
            .and_then(Value::as_str),
        Some("session.stream_plan.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/stream_plan_view/safe_to_open_stream_field")
            .and_then(Value::as_str),
        Some("safe_to_open_stream")
    );
    assert_eq!(
        data.pointer("/envelopes/stream_plan_view/next_actions_field")
            .and_then(Value::as_str),
        Some("next_actions")
    );
    assert_eq!(
        data.pointer("/envelopes/stream_plan_view/does_not_start_listener")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/stream_view/input_relay_preflight_command")
            .and_then(Value::as_str),
        Some("session command-check stream --input-event <action,args>")
    );
    assert_eq!(
        data.pointer("/envelopes/stream_view/input_relay_event_flags/1")
            .and_then(Value::as_str),
        Some("--input-event")
    );
    assert_eq!(
        data.pointer("/envelopes/queue_view/schema_version")
            .and_then(Value::as_str),
        Some("session.queue.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/queue_view/query")
            .and_then(Value::as_str),
        Some("session queue")
    );
    assert_eq!(
        data.pointer("/envelopes/queue_view/local_query_inspects_blocked_queue")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/envelopes/command_check_view/queue_gate_field")
            .and_then(Value::as_str),
        Some("queue_gate")
    );
    assert_eq!(
        data.pointer("/envelopes/command_check_view/instance_gate_field")
            .and_then(Value::as_str),
        Some("instance_gate")
    );
    assert_eq!(
        data.pointer("/envelopes/submit_plan_view/schema_version")
            .and_then(Value::as_str),
        Some("session.submit_plan.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/submit_plan_view/query")
            .and_then(Value::as_str),
        Some("session submit-plan <command...>")
    );
    assert_eq!(
        data.pointer("/envelopes/submit_plan_view/daemon_query")
            .and_then(Value::as_str),
        Some("session request submit-plan <command...>")
    );
    assert_eq!(
        data.pointer("/envelopes/submit_plan_view/preflight_summary_field")
            .and_then(Value::as_str),
        Some("preflight_summary")
    );
    assert_eq!(
        data.pointer("/envelopes/submit_plan_view/phase_c_execution_preflight_field")
            .and_then(Value::as_str),
        Some("phase_c_execution_preflight")
    );
    assert_eq!(
        data.pointer("/envelopes/submit_plan_view/phase_c_execution_preflight_schema_version")
            .and_then(Value::as_str),
        Some("session.submit_phase_c_execution_preflight.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/validation_plan_view/schema_version")
            .and_then(Value::as_str),
        Some("session.validation_plan.v0.1")
    );
    assert_eq!(
        data.pointer("/envelopes/validation_plan_view/deferred_code_field")
            .and_then(Value::as_str),
        Some("deferred_code")
    );
    assert_eq!(
        data.pointer("/failure_contract/untrusted_remote_endpoint_code")
            .and_then(Value::as_str),
        Some("trusted_remote_transport_blocked")
    );
}

#[test]
fn session_transport_is_offline_transport_contract() {
    let result = run_cli(["--json", "session", "transport"], true);
    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.get("schema_version").and_then(Value::as_str),
        Some("session.transport.v0.1")
    );
    assert_eq!(
        data.pointer("/channels/daemon_file_ipc/status")
            .and_then(Value::as_str),
        Some("available")
    );
    assert_eq!(
        data.pointer("/channels/trusted_remote/encryption_required")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.pointer("/channels/trusted_remote/auth_env/client_certificate")
            .and_then(Value::as_str),
        Some(TRUSTED_REMOTE_CLIENT_CERT_ENV)
    );
    assert_eq!(
        data.pointer("/channels/trusted_remote/preflight_command")
            .and_then(Value::as_str),
        Some("session transport check --endpoint <url>")
    );
    assert_eq!(
        data.pointer("/channels/trusted_remote/plan_command")
            .and_then(Value::as_str),
        Some("session transport plan [--endpoint <url>]")
    );
    assert_eq!(
        data.pointer("/safety/remote_transport_must_not_start_without_authentication")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn schema_pack_describes_current_supported_versions() {
    let result = run_cli(["--json", "schema", "pack"], true);
    assert_eq!(result.exit_code(), 0);
    let versions = result
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("schema_version")
        .and_then(Value::as_array)
        .expect("schema versions");
    assert!(versions.iter().any(|value| value.as_str() == Some("0.4")));
    assert!(versions.iter().any(|value| value.as_str() == Some("0.5")));
}

#[test]
fn top_level_record_capability_is_available() {
    let commands = command_capabilities();
    let record = commands
        .iter()
        .find(|command| command.get("command").and_then(Value::as_str) == Some("record"))
        .expect("record capability");
    assert_eq!(
        record.get("status").and_then(Value::as_str),
        Some("available")
    );
    assert_eq!(
        record
            .get("needs")
            .and_then(Value::as_array)
            .and_then(|needs| {
                needs
                    .iter()
                    .find(|need| need.as_str() == Some("offline"))
                    .and_then(Value::as_str)
            }),
        Some("offline")
    );
    for command_name in [
        "record start",
        "record status",
        "record stop",
        "record build-task",
        "session record start",
        "session record status",
        "session record stop",
        "session record build-task",
        "session stream",
        "session stream check",
        "session request stream check",
    ] {
        let command = commands
            .iter()
            .find(|command| command.get("command").and_then(Value::as_str) == Some(command_name))
            .unwrap_or_else(|| panic!("{command_name} capability"));
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
    }
    let stream = commands
        .iter()
        .find(|command| command.get("command").and_then(Value::as_str) == Some("stream"))
        .expect("stream capability");
    assert_eq!(
        stream.get("status").and_then(Value::as_str),
        Some("available")
    );
}

#[test]
fn session_response_capabilities_are_available() {
    let commands = command_capabilities();
    for command_name in [
        "session response",
        "session response get",
        "session response wait",
        "session request response",
        "session request response get",
        "session request response wait",
    ] {
        let command = commands
            .iter()
            .find(|command| command.get("command").and_then(Value::as_str) == Some(command_name))
            .unwrap_or_else(|| panic!("{command_name} capability"));
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
    }
}

#[test]
fn session_request_no_wait_capability_is_available() {
    let commands = command_capabilities();
    let command = commands
        .iter()
        .find(|command| {
            command.get("command").and_then(Value::as_str) == Some("session request --no-wait")
        })
        .expect("session request --no-wait capability");
    assert_eq!(
        command.get("status").and_then(Value::as_str),
        Some("available")
    );
}

#[test]
fn session_request_cancel_capability_is_available() {
    let commands = command_capabilities();
    let command = commands
        .iter()
        .find(|command| {
            command.get("command").and_then(Value::as_str) == Some("session request cancel")
        })
        .expect("session request cancel capability");
    assert_eq!(
        command.get("status").and_then(Value::as_str),
        Some("available")
    );
    assert_eq!(
        command.get("needs").and_then(Value::as_array).unwrap(),
        &vec![Value::String("offline".to_string())]
    );
}

#[test]
fn session_request_state_capabilities_are_available() {
    let commands = command_capabilities();
    for command_name in [
        "session request-state",
        "session request-state get",
        "session request-state wait",
        "session request-state list",
        "session request request-state",
        "session request request-state get",
        "session request request-state wait",
        "session request request-state list",
    ] {
        let command = commands
            .iter()
            .find(|command| command.get("command").and_then(Value::as_str) == Some(command_name))
            .unwrap_or_else(|| panic!("{command_name} capability"));
        assert_eq!(
            command.get("status").and_then(Value::as_str),
            Some("available")
        );
    }
}

#[test]
fn retired_session_and_lab_lease_authority_is_not_advertised() {
    let commands = command_capabilities();
    for name in [
        "session lease",
        "session lease list",
        "session lease touch",
        "session lease wait",
        "session request lease list",
        "session request lease touch",
        "session request lease wait",
        "lab lease",
        "lab lease list",
        "lab lease status",
        "lab lease touch",
        "lab lease wait",
        "lab preempt",
        "lab release",
    ] {
        assert!(
            commands.iter().all(|command| command["command"] != name),
            "retired Lab authority must not be advertised: {name}"
        );
    }
    assert!(commands.iter().any(|command| {
        command["command"] == "lab status" && command["needs"] == json!(["running_runtime"])
    }));
    assert!(commands.iter().any(|command| {
        command["command"] == "lab debug-package" && command["needs"] == json!(["running_runtime"])
    }));
}

#[test]
fn direct_touch_positionals_parse() {
    let tap = FlagArgs::parse(&["300".to_string(), "2".to_string()]).unwrap();
    assert_eq!(
        DirectTouchCommand::parse("tap", &tap).unwrap(),
        DirectTouchCommand::Tap { x: 300, y: 2 }
    );

    let swipe = FlagArgs::parse(&[
        "10".to_string(),
        "20".to_string(),
        "300".to_string(),
        "400".to_string(),
        "500".to_string(),
    ])
    .unwrap();
    assert_eq!(
        DirectTouchCommand::parse("swipe", &swipe).unwrap(),
        DirectTouchCommand::Swipe {
            x1: 10,
            y1: 20,
            x2: 300,
            y2: 400,
            duration_ms: 500
        }
    );

    let long_tap =
        FlagArgs::parse(&["100".to_string(), "200".to_string(), "900".to_string()]).unwrap();
    assert_eq!(
        DirectTouchCommand::parse("long-tap", &long_tap).unwrap(),
        DirectTouchCommand::LongTap {
            x: 100,
            y: 200,
            duration_ms: 900
        }
    );
}

#[test]
fn direct_touch_missing_args_are_usage_errors() {
    let flags = FlagArgs::parse(&["300".to_string()]).unwrap();
    let err = DirectTouchCommand::parse("tap", &flags).unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert_eq!(err.code, "validation_failed");
    assert!(err.message.contains("tap expects 2"));
}

#[test]
fn direct_input_positionals_parse() {
    let key = FlagArgs::parse(&["back".to_string()]).unwrap();
    assert_eq!(
        DirectInputCommand::parse("key", &key).unwrap(),
        DirectInputCommand::Key("4".to_string())
    );

    let text = FlagArgs::parse(&["hello".to_string(), "world".to_string()]).unwrap();
    assert_eq!(
        DirectInputCommand::parse("text", &text).unwrap(),
        DirectInputCommand::Text("hello world".to_string())
    );
}

#[test]
fn capture_static_page_same_hash_does_not_switch() {
    let decision = classify_capture_freshness(
        "same-frame",
        "same-frame",
        CaptureFreshnessExpectation::StaticPageAllowed,
    );

    assert_eq!(decision.status, CaptureFreshProbeStatus::StaticUnchanged);
    assert!(decision.ok);
    assert!(!decision.stale_suspected);
}

#[test]
fn capture_expected_change_stall_marks_stale_without_runtime_switch() {
    let decision = classify_capture_freshness(
        "same-frame",
        "same-frame",
        CaptureFreshnessExpectation::ExpectedChange,
    );

    assert_eq!(decision.status, CaptureFreshProbeStatus::StaleSuspected);
    assert!(!decision.ok);
    assert!(decision.stale_suspected);
}

#[test]
fn capture_diagnosis_recommends_fast_backends_before_restart_for_adb_stale() {
    let recovery = capture_diagnosis_recovery_json(
        CaptureFreshProbeStatus::StaleSuspected,
        CaptureBackendChoice::Adb,
    );
    assert_eq!(recovery.get("needed").and_then(Value::as_bool), Some(true));
    let recommendations = recovery
        .get("recommendations")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(
        recommendations[0].get("type").and_then(Value::as_str),
        Some("capture_backend")
    );
    assert_eq!(
        recommendations
            .last()
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str),
        Some("app_restart")
    );
}

#[test]
fn instance_health_status_reflects_capture_freshness() {
    assert_eq!(instance_health_status(None), "device_connected");
    assert_eq!(
        instance_health_status(Some(CaptureFreshProbeStatus::Fresh)),
        "healthy"
    );
    assert_eq!(
        instance_health_status(Some(CaptureFreshProbeStatus::StaticUnchanged)),
        "healthy_static"
    );
    assert_eq!(
        instance_health_status(Some(CaptureFreshProbeStatus::StaleSuspected)),
        "capture_stale_suspected"
    );
}

#[test]
fn instance_health_capture_diagnose_json_reports_recovery() {
    let report = CaptureFreshProbeReport {
        status: CaptureFreshProbeStatus::StaleSuspected,
        frame: None,
        attempts: vec![json!({
            "backend": "adb_screencap",
            "ok": false,
            "stage": "fresh_probe",
            "stale_suspected": true
        })],
        freshness: json!({
            "required": true,
            "fresh": false,
            "status": "stale_suspected"
        }),
    };

    let value = capture_fresh_probe_report_json(&report, CaptureBackendChoice::Adb);
    assert_eq!(
        value.get("status").and_then(Value::as_str),
        Some("stale_suspected")
    );
    assert_eq!(
        value.get("requested_backend").and_then(Value::as_str),
        Some("adb")
    );
    assert_eq!(
        value.pointer("/recovery/reason").and_then(Value::as_str),
        Some("stale_capture_suspected")
    );
    assert_eq!(
        value
            .pointer("/capture_backend_attempts/0/backend")
            .and_then(Value::as_str),
        Some("adb_screencap")
    );
}

#[test]
fn session_recover_stale_capture_plans_lighter_steps_before_restart() {
    let result = run_cli(
        [
            "--json",
            "--capture-backend",
            "adb",
            "session",
            "recover",
            "--stale-capture",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.get("mode").and_then(Value::as_str),
        Some("stale_capture_recovery")
    );
    assert_eq!(data.get("executed").and_then(Value::as_bool), Some(false));
    assert_eq!(
        data.pointer("/steps/0/type").and_then(Value::as_str),
        Some("fresh_probe")
    );
    assert_eq!(
        data.pointer("/steps/3/type").and_then(Value::as_str),
        Some("app_restart")
    );
    assert_eq!(
        data.pointer("/steps/3/requires_lease")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn stale_capture_recovery_json_reports_executed_fresh_diagnosis() {
    let report = CaptureFreshProbeReport {
        status: CaptureFreshProbeStatus::Fresh,
        frame: None,
        attempts: vec![json!({
            "backend": "nemu_ipc",
            "ok": true,
            "stage": "fresh_probe"
        })],
        freshness: json!({
            "required": true,
            "fresh": true,
            "status": "fresh"
        }),
    };

    let value = stale_capture_recovery_json(
        CaptureBackendChoice::Auto,
        Duration::from_millis(200),
        Some(&report),
    );

    assert_eq!(
        value.get("status").and_then(Value::as_str),
        Some("diagnosed_fresh")
    );
    assert_eq!(
        value.get("diagnosis_executed").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        value
            .pointer("/diagnosis/result/status")
            .and_then(Value::as_str),
        Some("fresh")
    );
    assert_eq!(
        value.pointer("/recovery/needed").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        value
            .pointer("/diagnosis/result/capture_backend_attempts/0/backend")
            .and_then(Value::as_str),
        Some("nemu_ipc")
    );
}

#[test]
fn direct_touch_commands_are_capability_registered() {
    let commands = command_capabilities();
    for command in ["tap", "swipe", "long-tap", "key", "text"] {
        let capability = commands
            .iter()
            .find(|value| value.get("command").and_then(Value::as_str) == Some(command))
            .unwrap_or_else(|| panic!("{command} capability missing"));
        assert_eq!(
            capability.get("status").and_then(Value::as_str),
            Some("available")
        );
        assert_eq!(
            capability.get("needs").and_then(Value::as_array).unwrap(),
            &vec![Value::String("device".to_string())]
        );
    }
    for command in [
        "session status",
        "session bootstrap",
        "session throat-policy",
        "session capture-policy",
        "session self-heal-policy",
        "session submit-plan",
        "session validation-plan",
        "session journal",
        "session events",
        "session events wait",
        "session contract",
        "session api",
        "session instance",
        "session instance list",
        "session instance app",
        "session instance app launch",
        "session instance app stop",
        "session instance app force-stop",
        "session instance app restart",
        "session app",
        "session app launch",
        "session app stop",
        "session app force-stop",
        "session app restart",
        "session capture",
        "session capture diagnose",
        "session recover --stale-capture",
        "session request status",
        "session request bootstrap",
        "session request throat-policy",
        "session request capture-policy",
        "session request self-heal-policy",
        "session request submit-plan",
        "session request validation-plan",
        "session request journal",
        "session request events",
        "session request events wait",
        "session request cancel",
        "session request contract",
        "session request api",
        "session request devices",
        "session request record",
        "session request capture",
        "session request capture-diagnose",
        "session request stream",
        "session request recognize",
        "session request detect-page",
        "session request current-page",
        "session request is-visible",
        "session request locate",
        "session request monitor-once",
        "session request instance list",
        "session request instance registry",
        "session request instance app",
        "session request app",
        "session request recover --stale-capture",
        "session request lab-run",
        "session request package-run",
        "session request operation-run",
        "ledger show",
        "ledger events",
        "ledger receipts",
        "ledger diagnose",
        "ledger evidence",
        "stream",
        "capture diagnose",
    ] {
        let capability = commands
            .iter()
            .find(|value| value.get("command").and_then(Value::as_str) == Some(command))
            .unwrap_or_else(|| panic!("{command} capability missing"));
        assert_eq!(
            capability.get("status").and_then(Value::as_str),
            Some("available")
        );
    }
    for command in [
        "session instance health",
        "session instance keep-alive",
        "session instance connect",
        "session instance reconnect",
        "session request instance health",
        "session request instance keep-alive",
        "session request instance connect",
        "session request instance reconnect",
    ] {
        let capability = commands
            .iter()
            .find(|value| value.get("command").and_then(Value::as_str) == Some(command))
            .unwrap_or_else(|| panic!("{command} retirement marker missing"));
        assert_eq!(
            capability.get("status").and_then(Value::as_str),
            Some("retired")
        );
        assert_eq!(
            capability.get("needs").and_then(Value::as_array).unwrap(),
            &vec![Value::String("offline".to_string())]
        );
    }
}

#[test]
fn retired_instance_commands_are_absent_from_live_contracts() {
    let global = GlobalOptions::default();
    let flags = FlagArgs::default();
    let contracts = [
        session_layer_capability_contract(),
        session_access_contract(),
        session_api_contract(),
        session_capture_policy_payload(&global, &flags, "session capture-policy").unwrap(),
        session_self_heal_policy_payload(&global, &flags, "session self-heal-policy").unwrap(),
        stale_capture_recovery_json(CaptureBackendChoice::Adb, Duration::from_millis(1), None),
    ];
    for contract in contracts {
        let text = serde_json::to_string(&contract).unwrap();
        for command in [
            "session instance health",
            "session instance keep-alive",
            "session instance connect",
            "session instance reconnect",
            "session request instance health",
            "session request instance keep-alive",
            "session request instance connect",
            "session request instance reconnect",
        ] {
            assert!(
                !text.contains(command),
                "retired command advertised: {command}"
            );
        }
    }
}

#[test]
fn package_validate_accepts_safe_zip() {
    let temp = TempDir::new().unwrap();
    let zip = temp.path().join("bundle.zip");
    write_test_zip(
        &zip,
        &[
            (
                "module/manifest.json",
                br#"{"schema_version":"0.2"}"#.as_slice(),
            ),
            (
                "module/operations/task/task.json",
                br#"{"id":"task"}"#.as_slice(),
            ),
            ("module/operations/resources.json", br#"{}"#.as_slice()),
        ],
    );
    let result = run_cli(
        [
            "--json",
            "package",
            "validate",
            "--zip",
            zip.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().expect("validation payload");
    assert_eq!(
        data.get("hash_source").and_then(Value::as_str),
        Some("self_computed_provenance_only")
    );
    assert_eq!(
        data.get("externally_verified").and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn package_validate_accepts_matching_external_hash() {
    let temp = TempDir::new().unwrap();
    let zip = temp.path().join("bundle.zip");
    write_test_zip(
        &zip,
        &[
            (
                "module/manifest.json",
                br#"{"schema_version":"0.2"}"#.as_slice(),
            ),
            (
                "module/operations/task/task.json",
                br#"{"id":"task"}"#.as_slice(),
            ),
            ("module/operations/resources.json", br#"{}"#.as_slice()),
        ],
    );
    let hash = format!("{:x}", Sha256::digest(fs::read(&zip).unwrap()));

    let result = run_cli(
        [
            "--json",
            "package",
            "validate",
            "--zip",
            zip.to_str().unwrap(),
            "--expected-sha256",
            &hash,
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("validation payload");
    assert_eq!(
        data.get("hash_source").and_then(Value::as_str),
        Some("externally_supplied")
    );
    assert_eq!(
        data.get("externally_verified").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        data.get("input_sha256").and_then(Value::as_str),
        Some(hash.as_str())
    );
}

#[test]
fn package_validate_rejects_mismatched_external_hash() {
    let temp = TempDir::new().unwrap();
    let zip = temp.path().join("bundle.zip");
    write_test_zip(
        &zip,
        &[
            (
                "module/manifest.json",
                br#"{"schema_version":"0.2"}"#.as_slice(),
            ),
            (
                "module/operations/task/task.json",
                br#"{"id":"task"}"#.as_slice(),
            ),
            ("module/operations/resources.json", br#"{}"#.as_slice()),
        ],
    );

    let result = run_cli(
        [
            "--json",
            "package",
            "validate",
            "--zip",
            zip.to_str().unwrap(),
            "--expected-sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2);
    assert!(result.envelope_json().contains("hash mismatch"));
}

#[test]
fn package_validate_rejects_bare_external_hash_flag() {
    let temp = TempDir::new().unwrap();
    let zip = temp.path().join("bundle.zip");
    write_test_zip(
        &zip,
        &[
            (
                "module/manifest.json",
                br#"{"schema_version":"0.2"}"#.as_slice(),
            ),
            (
                "module/operations/task/task.json",
                br#"{"id":"task"}"#.as_slice(),
            ),
            ("module/operations/resources.json", br#"{}"#.as_slice()),
        ],
    );

    let result = run_cli(
        [
            "--json",
            "package",
            "validate",
            "--zip",
            zip.to_str().unwrap(),
            "--expected-sha256",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2);
    assert!(
        result
            .envelope_json()
            .contains("requires an explicit SHA-256 value")
    );
}

#[test]
fn package_validate_reports_offline_projection_without_local_ledger() {
    let _guard = env_lock();
    set_missing_config_env();
    let temp = TempDir::new().unwrap();
    let run_root = temp.path().join("runs");
    let zip = temp.path().join("bundle.zip");
    write_test_zip(
        &zip,
        &[
            (
                "module/manifest.json",
                br#"{"schema_version":"0.2"}"#.as_slice(),
            ),
            (
                "module/operations/task/task.json",
                br#"{"id":"task"}"#.as_slice(),
            ),
            ("module/operations/resources.json", br#"{}"#.as_slice()),
        ],
    );

    let result = run_cli(
        [
            "--json",
            "--run-root",
            run_root.to_str().unwrap(),
            "package",
            "validate",
            "--zip",
            zip.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.pointer("/ledger_event/written")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        data.pointer("/ledger_event/reason").and_then(Value::as_str),
        Some("offline_resource_tooling_projection")
    );
    assert!(!run_root.exists());
}

#[test]
fn list_resource_kind_unknown_returns_usage_error() {
    let temp = TempDir::new().unwrap();

    let err = list_resource_kind(temp.path(), "future-kind").expect_err("unknown kind");

    assert_eq!(err.code, "validation_failed");
    assert!(err.message.contains("unknown list kind"));
}

#[test]
fn lab_run_capture_backend_flag_is_global_even_after_subcommand() {
    let invocation = parse_invocation(
        [
            "--json",
            "lab",
            "run",
            "--zip",
            "in.zip",
            "--capture-backend",
            "nemu_ipc",
            "--out",
            "out.zip",
        ],
        true,
    )
    .expect("invocation");

    assert_eq!(
        invocation.global.capture_backend,
        Some(CaptureBackendChoice::NemuIpc)
    );
    assert_eq!(invocation.command, ["lab", "run"]);
    assert_eq!(invocation.args, ["--zip", "in.zip", "--out", "out.zip"]);
}

#[test]
fn capture_backend_short_alias_is_global_even_after_subcommand() {
    let invocation = parse_invocation(
        [
            "--json",
            "capture",
            "--out",
            "frame.png",
            "--backend",
            "adb",
            "--require-fresh",
        ],
        true,
    )
    .expect("invocation");

    assert_eq!(
        invocation.global.capture_backend,
        Some(CaptureBackendChoice::Adb)
    );
    assert_eq!(invocation.command, ["capture"]);
    assert_eq!(invocation.args, ["--out", "frame.png", "--require-fresh"]);
}

#[test]
fn touch_backend_flag_is_global_even_after_subcommand() {
    let invocation = parse_invocation(
        [
            "--json",
            "tap",
            "10",
            "20",
            "--touch-backend",
            "adb_shell_input",
        ],
        true,
    )
    .expect("invocation");

    assert_eq!(
        invocation.global.touch_backend,
        Some(TouchBackendChoice::AdbShellInput)
    );
    assert_eq!(invocation.command, ["tap"]);
    assert_eq!(invocation.args, ["10", "20"]);
}

#[test]
fn help_lists_capture_backend_short_alias() {
    let help = help_data();
    assert!(
        help.get("global_options")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|option| option
                .as_str()
                .is_some_and(|text| text.starts_with("--backend ")))
    );
}

#[test]
fn help_lists_resource_convert_maa_tasks_option() {
    let help = help_data();
    let options = help
        .pointer("/command_options/resource convert")
        .and_then(Value::as_array)
        .expect("resource convert options");

    assert!(
        options
            .iter()
            .any(|option| option.as_str() == Some("--maa-tasks <dir>"))
    );
}

#[test]
fn help_lists_required_external_authoring_metadata() {
    let help = help_data();
    assert_eq!(
        help.pointer("/command_options/resource compile-maa/0")
            .and_then(Value::as_str),
        Some("--maa-tasks <dir>")
    );
    assert_eq!(
        help.pointer("/command_options/resource compile-maa/1")
            .and_then(Value::as_str),
        Some("--task <id> (repeatable with --facts)")
    );
    assert_eq!(
        help.pointer("/command_options/session record build-task/0")
            .and_then(Value::as_str),
        Some("--locale <locale>")
    );
}

#[test]
fn help_documents_recognize_target_shared_output_shape() {
    let help = help_data();
    let note = help
        .pointer("/compatibility_notes/recognize --target")
        .and_then(Value::as_str)
        .expect("recognize note");

    assert!(note.contains("width"));
    assert!(note.contains("height"));
    assert!(note.contains("matched_rect"));
}

#[test]
fn bare_instance_argument_is_used_as_adb_serial_without_config_entry() {
    let global = GlobalOptions {
        instance: Some("127.0.0.1:16416".to_string()),
        ..Default::default()
    };
    let (_adb_dir, config) = user_config_with_test_adb();
    let resolved = device_config(&global, &config).expect("device config");
    assert_eq!(resolved.target.serial.as_deref(), Some("127.0.0.1:16416"));
}

#[test]
fn device_config_uses_instance_capture_backend_default() {
    let global = GlobalOptions {
        instance: Some("ak-b".to_string()),
        ..Default::default()
    };
    let (_adb_dir, mut config) = user_config_with_test_adb();
    config.instances.insert(
        "ak-b".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16416".to_string()),
            capture_backend: Some("nemu_ipc".to_string()),
            ..Default::default()
        },
    );

    let resolved = device_config(&global, &config).expect("device config");

    assert_eq!(resolved.target.serial.as_deref(), Some("127.0.0.1:16416"));
    assert_eq!(resolved.capture_backend, CaptureBackendChoice::NemuIpc);
}

#[test]
fn device_config_cli_capture_backend_overrides_instance_default() {
    let global = GlobalOptions {
        instance: Some("ak-b".to_string()),
        capture_backend: Some(CaptureBackendChoice::Adb),
        ..Default::default()
    };
    let (_adb_dir, mut config) = user_config_with_test_adb();
    config.instances.insert(
        "ak-b".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16416".to_string()),
            capture_backend: Some("nemu_ipc".to_string()),
            ..Default::default()
        },
    );

    let resolved = device_config(&global, &config).expect("device config");

    assert_eq!(resolved.capture_backend, CaptureBackendChoice::Adb);
}

#[test]
fn device_config_cli_touch_backend_overrides_instance_default() {
    let global = GlobalOptions {
        instance: Some("ak-b".to_string()),
        touch_backend: Some(TouchBackendChoice::AdbShellInput),
        ..Default::default()
    };
    let (_adb_dir, mut config) = user_config_with_test_adb();
    config.instances.insert(
        "ak-b".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16416".to_string()),
            touch_backend: Some("maatouch".to_string()),
            ..Default::default()
        },
    );

    let resolved = device_config(&global, &config).expect("device config");

    assert_eq!(resolved.touch_backend, TouchBackendChoice::AdbShellInput);
}

#[test]
fn current_page_resolves_semantic_page() {
    let _guard = env_lock();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "current-page",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    assert_eq!(
        result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("page")
            .and_then(Value::as_str),
        Some("arknights/home")
    );
}

#[test]
fn tap_target_dry_run_requires_visible_target_and_returns_point() {
    let _guard = env_lock();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "tap-target",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
            "--dry-run",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let point = result.envelope.data.as_ref().unwrap().get("point").unwrap();
    assert_eq!(point.get("x").and_then(Value::as_i64), Some(12));
    assert_eq!(point.get("y").and_then(Value::as_i64), Some(23));
}

#[test]
fn navigate_dry_run_uses_navigation_graph() {
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "navigate",
            "--to",
            "target",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let route = result
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("route")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(route.len(), 1);
    assert_eq!(
        route[0].get("id").and_then(Value::as_str),
        Some("home_to_target")
    );
}

#[test]
fn local_ledger_query_commands_fail_loud() {
    let result = run_cli(["--json", "ledger", "show"], true);

    assert_eq!(result.exit_code(), 6, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().unwrap();
    assert_eq!(error.code, "local_ledger_retired");
    assert!(error.message.contains("lab watch or lab receipt"));
}

#[test]
fn navigate_blocks_destructive_overlap_by_default() {
    let temp = semantic_resource_root(true);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "navigate",
            "--to",
            "target",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 3);
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "navigation_destructive_overlap"
    );
}

#[test]
fn lab2_observe_reports_page_targets_actions_and_frame_path() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    let frame_out = temp.path().join("observe-frame.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "observe",
            "--scene",
            scene.to_str().unwrap(),
            "--targets",
            "home_button",
            "--with-frame",
            frame_out.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert!(data.get("req_id").and_then(Value::as_str).is_some());
    assert_eq!(
        data.get("page").and_then(Value::as_str),
        Some("arknights/home")
    );
    assert_eq!(
        data.get("backend").and_then(Value::as_str),
        Some("scene_file")
    );
    assert_eq!(
        data.get("targets")
            .and_then(Value::as_array)
            .and_then(|targets| targets.first())
            .and_then(|target| target.get("id"))
            .and_then(Value::as_str),
        Some("home_button")
    );
    assert_eq!(
        data.get("actions")
            .and_then(Value::as_array)
            .and_then(|actions| actions.first())
            .and_then(|action| action.get("id"))
            .and_then(Value::as_str),
        Some("home_to_target")
    );
    assert!(frame_out.exists());
    assert!(!result.envelope_json().contains("base64"));
}

#[test]
fn lab2_observe_projects_contained_page_elements() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
    }
    let temp = TempDir::new().unwrap();
    let pack = temp.path().join("pack.json");
    let pages = temp.path().join("pages.json");
    let navigation = temp.path().join("navigation.json");
    let scene = temp.path().join("scene.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    fs::write(
        temp.path().join("anchor.png"),
        encode_png(1, 1, [255, 0, 0]),
    )
    .unwrap();
    fs::write(&pack, serde_json::to_vec(&json!({
        "schema_version":"0.3", "coordinate_space":{"width":1,"height":1},
        "targets":[
            {"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
            {"type":"template","id":"visible","template_path":"anchor.png","region":{"x":0,"y":0,"width":1,"height":1},"threshold":0.8},
            {"type":"color","id":"alternative","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
            {"type":"color","id":"optional","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
            {"type":"color","id":"forbidden","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
        ]
    })).unwrap()).unwrap();
    fs::write(
        &pages,
        serde_json::to_vec(&json!({"schema_version":"0.3","pages":[{
            "id":"fixture/home","required":["anchor"],"any_of":[["visible","alternative"]],
            "optional":["optional"],"forbidden":["forbidden"]
        }]}))
        .unwrap(),
    )
    .unwrap();
    fs::write(&navigation, serde_json::to_vec(&json!({
        "navigation":[{"id":"open","from_page":"fixture/home","to_page":"fixture/next","source":"mapping/open","click":{"kind":"target_center","target_id":"visible"}}],
        "page_operations":[
            {"id":"claim","task_id":"task","page":"fixture/home","purpose":"Claim visible reward","click":{"kind":"rect","x":0,"y":0,"width":1,"height":1}},
            {"id":"color","task_id":"task","page":"fixture/home","purpose":"Color anchor","click":{"kind":"target","target_id":"anchor"}},
            {"id":"absent","task_id":"task","page":"fixture/home","purpose":"Absent option","click":{"kind":"target","target_id":"optional"}},
            {"id":"elsewhere","task_id":"task","page":"fixture/other","purpose":"Other page","click":{"kind":"point","point":[0,0]}}
        ],
        "destructive_actions":[{"id":"claim","click":{"kind":"target","target_id":"visible"}}],
        "control_points":[{"name":"back","point":[0,0]}]
    })).unwrap()).unwrap();
    let fixture = seal_semantic_fixture(temp, "fixture", "test", &pack, &pages, Some(&navigation));
    let result = run_semantic_cli(
        &fixture,
        [
            "--json",
            "observe",
            "--scene",
            scene.to_str().unwrap(),
            "--verbose",
        ],
        true,
    );
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let observation = &result.envelope.data.as_ref().unwrap()["observation"];
    let elements = observation["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[0]["role"], "navigate");
    assert_eq!(elements[0]["input"]["point"], json!({"x":0,"y":0}));
    assert_eq!(elements[0]["actionable"], true);
    assert_eq!(elements[1]["label"], "Claim visible reward");
    assert_eq!(elements[1]["role"], "page_op");
    assert_eq!(elements[1]["safety"], "unclassified");
    assert_eq!(elements[1]["actionable"], true);
    assert_eq!(elements[2]["recognized"], true);
    assert_eq!(elements[2]["actionable"], false);
    assert_eq!(
        elements[2]["blocked_reason"],
        "matched_template_rect_unavailable"
    );
    let control = &observation["unscoped_controls"][0];
    assert_eq!(control["scope"], "unscoped");
    assert_eq!(control["availability"], "unknown");
    assert_eq!(control["recognized"], false);
    assert_eq!(control["actionable"], true);
    let missing = observation["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 3);
    assert!(
        missing
            .iter()
            .any(|entry| entry["resource_id"] == "absent" && entry["recognized"] == false)
    );
    assert!(missing.iter().any(|entry| entry["id"] == "alternative"
        && entry["role"] == "any_of"
        && entry["group_satisfied"] == true));
    assert!(
        missing
            .iter()
            .any(|entry| entry["id"] == "optional" && entry["role"] == "optional")
    );
    assert!(!missing.iter().any(|entry| entry["id"] == "forbidden"));
    assert_eq!(observation["metrics"]["recognized_count"], 3);
    assert_eq!(observation["metrics"]["entry_count"], 7);
    assert_eq!(observation["page_window_completeness"], "unknown");
}

#[test]
fn lab2_observe_uses_unique_page_owner() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
    }
    for (matching, conflict) in [(true, false), (false, false), (true, true)] {
        let temp = TempDir::new().unwrap();
        let pack = temp.path().join("pack.json");
        let pages = temp.path().join("pages.json");
        let navigation = temp.path().join("navigation.json");
        let scene = temp.path().join("scene.png");
        fs::write(
            &scene,
            encode_png(1, 1, if matching { [255, 0, 0] } else { [0, 0, 255] }),
        )
        .unwrap();
        fs::write(&pack, serde_json::to_vec(&json!({
            "schema_version":"0.3", "coordinate_space":{"width":1,"height":1},
            "targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]
        })).unwrap()).unwrap();
        let mut definitions = vec![json!({"id":"fixture/home","required":["anchor"]})];
        if conflict {
            definitions.push(json!({"id":"fixture/also_home","required":["anchor"]}));
        }
        fs::write(
            &pages,
            serde_json::to_vec(&json!({"schema_version":"0.3","pages":definitions})).unwrap(),
        )
        .unwrap();
        fs::write(&navigation, br#"{"navigation":[]}"#).unwrap();
        let fixture =
            seal_semantic_fixture(temp, "fixture", "test", &pack, &pages, Some(&navigation));
        let result = run_semantic_cli(
            &fixture,
            ["--json", "observe", "--scene", scene.to_str().unwrap()],
            true,
        );
        if conflict {
            assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
            let error = result.envelope.error.as_ref().unwrap();
            assert_eq!(error.code, "page_recognition_conflict");
            assert!(result.envelope_json().contains("fixture/also_home"));
            assert!(result.envelope.data.is_none());
        } else {
            assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
            let data = result.envelope.data.as_ref().unwrap();
            assert_eq!(data["observation"]["matched"], matching);
            assert_eq!(data["observation"]["standby"], !matching);
            assert_eq!(
                data["observation"]["page"],
                if matching { "fixture/home" } else { "unknown" }
            );
            if !matching {
                assert!(
                    data["candidates"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
                );
                assert_eq!(data["recovery_hint"]["action"], "wake_safe_point");
                assert_eq!(data["observation"]["metrics"]["matched_page_count"], 0);
            }
        }
    }
}

#[test]
fn lab2_observe_bounds_observation_in_min() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
    }
    for oversized_label in [false, true] {
        let temp = TempDir::new().unwrap();
        let pack = temp.path().join("pack.json");
        let pages = temp.path().join("pages.json");
        let navigation = temp.path().join("navigation.json");
        let scene = temp.path().join("scene.png");
        fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
        fs::write(&pack, serde_json::to_vec(&json!({
            "schema_version":"0.3", "coordinate_space":{"width":1,"height":1},
            "targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]
        })).unwrap()).unwrap();
        fs::write(
            &pages,
            br#"{"schema_version":"0.3","pages":[{"id":"fixture/home","required":["anchor"]}]}"#,
        )
        .unwrap();
        let count = if oversized_label { 2 } else { 80 };
        let operations = (0..count).map(|index| json!({
            "id":format!("op-{index}"), "task_id":"task", "page":"fixture/home",
            "purpose":if oversized_label && index == 0 { "x".repeat(40 * 1024) } else { format!("Label {index}") },
            "click":{"kind":"point","point":[0,0]}
        })).collect::<Vec<_>>();
        fs::write(
            &navigation,
            serde_json::to_vec(&json!({"navigation":[],"page_operations":operations})).unwrap(),
        )
        .unwrap();
        let fixture =
            seal_semantic_fixture(temp, "fixture", "test", &pack, &pages, Some(&navigation));
        for verbosity in [None, Some("--verbose"), Some("--pretty")] {
            let mut args = vec!["--json", "observe", "--scene", scene.to_str().unwrap()];
            args.extend(verbosity);
            let result = run_semantic_cli(&fixture, args, true);
            assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
            let data = result.envelope.data.as_ref().unwrap();
            let observation = &data["observation"];
            let entries = observation["elements"].as_array().unwrap();
            assert!(!entries.is_empty());
            assert!(entries.len() <= 64);
            assert!(serde_json::to_vec(observation).unwrap().len() <= 32 * 1024);
            if verbosity.is_none() {
                assert!(serde_json::to_vec(data).unwrap().len() <= 2048);
            }
            assert_eq!(observation["metrics"]["recognized_count"], count);
            assert_eq!(observation["metrics"]["entry_count"], count);
            assert_eq!(observation["metrics"]["emitted_count"], entries.len());
            assert_eq!(observation["omitted_count"], count - entries.len());
            assert_eq!(observation["truncated"], true);
            assert_eq!(observation["page_window_completeness"], "unknown");
            assert_eq!(
                observation["metrics"]["sample_scope"],
                "single_offline_observation"
            );
            for (index, entry) in entries.iter().enumerate() {
                let declaration = if oversized_label { 1 } else { index };
                assert_eq!(entry["resource_id"], format!("op-{declaration}"));
                assert_eq!(entry["label"], format!("Label {declaration}"));
                assert_eq!(entry["role"], "page_op");
                assert_eq!(entry["availability"], "available");
                assert_eq!(entry["actionable"], true);
                assert_eq!(entry["safety"], "unclassified");
                assert!(entry["blocked_reason"].is_null());
                assert!(entry.get("ocr").is_none());
                assert!(entry.get("frame_pixels").is_none());
            }
        }
    }
}

#[test]
fn lab2_do_dry_run_reports_guard_and_actual_click() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(data.get("executed").and_then(Value::as_bool), Some(false));
    assert_eq!(
        data.get("actual_click")
            .and_then(|value| value.get("point"))
            .and_then(|value| value.get("x"))
            .and_then(Value::as_i64),
        Some(12)
    );
    assert_eq!(
        data.get("guard_result")
            .and_then(|value| value.get("passed"))
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn lab2_do_guard_miss_returns_actionable_error_details() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("target.png");
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 3);
    let error = result.envelope.error.as_ref().unwrap();
    assert_eq!(error.code, "target_not_visible");
    let details = error.details.as_ref().unwrap();
    assert!(details.get("req_id").and_then(Value::as_str).is_some());
    assert_eq!(
        details.get("error").and_then(Value::as_str),
        Some("resource_drift")
    );
    let hint = details
        .get("hint")
        .and_then(Value::as_str)
        .expect("resource drift hint");
    assert!(!hint.contains("retry"));
}

#[test]
fn lab2_ensure_is_idempotent_and_plans_routes() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let home = temp.path().join("home.png");
    fs::write(&home, encode_png(1, 1, [255, 0, 0])).unwrap();

    let idempotent = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "ensure",
            "home",
            "--scene",
            home.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(idempotent.exit_code(), 0);
    assert_eq!(
        idempotent
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("state")
            .and_then(Value::as_str),
        Some("already_at_target")
    );

    let planned = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "ensure",
            "target",
            "--scene",
            home.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(planned.exit_code(), 0);
    let route = planned
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("route")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(route.len(), 1);
    assert_eq!(
        route[0].get("id").and_then(Value::as_str),
        Some("home_to_target")
    );
}

#[test]
fn lab2_wait_reports_page_and_stable_target() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let page = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "wait",
            "--page",
            "arknights/home",
            "--scene",
            scene.to_str().unwrap(),
            "--timeout-ms",
            "100",
        ],
        true,
    );
    assert_eq!(page.exit_code(), 0);
    assert_eq!(
        page.envelope
            .data
            .as_ref()
            .unwrap()
            .get("state")
            .and_then(Value::as_str),
        Some("arrived")
    );

    let stable = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "wait",
            "--stable",
            "home_anchor",
            "--scene",
            scene.to_str().unwrap(),
            "--timeout-ms",
            "100",
        ],
        true,
    );
    assert_eq!(stable.exit_code(), 0);
    assert_eq!(
        stable
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("state")
            .and_then(Value::as_str),
        Some("stable")
    );
    assert!(
        stable
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("wf_id")
            .and_then(Value::as_str)
            .is_some()
    );
}

#[test]
fn lab2_capabilities_and_schema_report_compiled_contracts() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    let temp = tempfile::tempdir().unwrap();
    set_config_env(temp.path().join("config.json"));
    let mut config = UserConfig::default();
    config.instances.insert(
        "ak-live".to_string(),
        InstanceConfig {
            serial: Some("127.0.0.1:16416".to_string()),
            game: Some("ark".to_string()),
            server: Some("cn".to_string()),
            capture_backend: Some("adb".to_string()),
            touch_backend: Some("maatouch".to_string()),
            ..Default::default()
        },
    );
    write_user_config(&config).unwrap();

    let capabilities = run_cli(["--json", "capabilities"], true);

    assert_eq!(capabilities.exit_code(), 0);
    let data = capabilities.envelope.data.as_ref().unwrap();
    assert!(
        data.pointer("/lab2_cli/verbs")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|verb| verb.get("command").and_then(Value::as_str) == Some("do"))
    );
    assert!(
        data.pointer("/lab2_cli/engine_capabilities/template_matching/families")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|family| family.get("id").and_then(Value::as_str) == Some("ccoeff"))
    );
    assert!(
        data.pointer("/lab2_cli/engine_capabilities/template_matching/unsupported")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .any(|capability| {
                capability.get("id").and_then(Value::as_str) == Some("masked_template_match")
            })
    );
    assert_eq!(
        data.pointer("/lab2_cli/instances/0/id")
            .and_then(Value::as_str),
        Some("ak-live")
    );
    assert_eq!(
        data.pointer("/lab2_cli/recovery_transparency/event_type")
            .and_then(Value::as_str),
        Some("recovery.state.changed")
    );

    let do_schema = run_cli(["--json", "schema", "do"], true);
    assert_eq!(do_schema.exit_code(), 0);
    assert_eq!(
        do_schema
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("command")
            .and_then(Value::as_str),
        Some("do")
    );

    let receipt_schema = run_cli(["--json", "schema", "lab", "receipt"], true);
    assert_eq!(receipt_schema.exit_code(), 0);
    assert_eq!(
        receipt_schema
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("command")
            .and_then(Value::as_str),
        Some("lab receipt")
    );
}

#[test]
fn lab2_chain_acceptance_min_projection_and_error_shape_are_actionable() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let home = temp.path().join("home.png");
    let target = temp.path().join("target.png");
    fs::write(&home, encode_png(1, 1, [255, 0, 0])).unwrap();
    fs::write(&target, encode_png(1, 1, [0, 0, 255])).unwrap();

    let observe_started = Instant::now();
    let observe = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "observe",
            "--scene",
            home.to_str().unwrap(),
            "--targets",
            "home_button",
        ],
        true,
    );
    let observe_elapsed = observe_started.elapsed();

    assert_eq!(observe.exit_code(), 0);
    let data = observe.envelope.data.as_ref().unwrap();
    assert!(
        serde_json::to_string(data).unwrap().len() <= 1024,
        "min projection data exceeded 1 KiB: {}",
        serde_json::to_string(data).unwrap().len()
    );
    assert!(
        observe_elapsed < Duration::from_millis(300),
        "synthetic observe took {observe_elapsed:?}"
    );
    assert!(!observe.envelope_json().contains('\u{1b}'));

    let error = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            target.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(error.exit_code(), 3);
    let details = error
        .envelope
        .error
        .as_ref()
        .unwrap()
        .details
        .as_ref()
        .unwrap();
    assert!(details.get("req_id").and_then(Value::as_str).is_some());
    assert!(details.get("state").and_then(Value::as_str).is_some());
    assert!(details.get("hint").and_then(Value::as_str).is_some());
    assert!(!error.envelope_json().contains('\u{1b}'));
}

#[test]
fn lab2_do_destructive_overlap_requires_opt_in_and_allows_explicit_opt_in() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(true);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();

    let destructive_without_allow = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
            "--destructive",
        ],
        true,
    );
    assert_eq!(destructive_without_allow.exit_code(), 3);
    assert_eq!(
        destructive_without_allow
            .envelope
            .error
            .as_ref()
            .unwrap()
            .code,
        "destructive_action_requires_allow_destructive"
    );

    let allowed = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
            "--destructive",
            "--allow-destructive",
        ],
        true,
    );

    assert_eq!(allowed.exit_code(), 0, "{}", allowed.envelope_json());
    assert_eq!(
        allowed
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("executed")
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn lab2_evidence_lists_debug_evidence_refs() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = tempfile::tempdir().unwrap();
    let evidence_dir = temp.path().join("evidence").join("req-1");
    fs::create_dir_all(&evidence_dir).unwrap();
    fs::write(evidence_dir.join("frame-deadbeef.bin"), b"frame").unwrap();

    let result = run_cli(
        [
            "--json",
            "--run-root",
            temp.path().to_str().unwrap(),
            "lab",
            "evidence",
            "--id",
            "req-1",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    assert_eq!(
        result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("count")
            .and_then(Value::as_u64),
        Some(1)
    );
}

#[test]
fn lab2_observe_unknown_reports_candidates() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("unknown.png");
    fs::write(&scene, encode_png(1, 1, [12, 34, 56])).unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "observe",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(data.get("state").and_then(Value::as_str), Some("unknown"));
    assert_eq!(data.get("page").and_then(Value::as_str), Some("unknown"));
    assert!(
        data.get("candidates")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    );
    assert_eq!(
        data.pointer("/suspicion/reason").and_then(Value::as_str),
        Some("low_page_margin")
    );
}

#[test]
fn lab2_do_click_rect_follows_live_template_match_delta() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = template_drift_resource_root();
    let scene = temp.path().join("shifted.png");
    fs::write(
        &scene,
        encode_rgb_png(3, 1, &[[0, 0, 0], [255, 0, 0], [0, 0, 0]]),
    )
    .unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    if result.exit_code() != 0 {
        panic!("{}", result.envelope_json());
    }
    let click = result
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("actual_click")
        .unwrap();
    assert_eq!(
        click.get("kind").and_then(Value::as_str),
        Some("target_rect_center_live_match")
    );
    assert_eq!(click.pointer("/rect/x").and_then(Value::as_i64), Some(1));
    assert_eq!(
        click
            .pointer("/coordinate_derivation/matched_rect/x")
            .and_then(Value::as_i64),
        Some(1)
    );
}

#[test]
fn lab2_do_rejects_mixed_online_capture_and_offline_scene_before_touch() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
        env::remove_var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG");
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("target-drift.png");
    let touch_log = temp.path().join("fake-touch.json");
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
    unsafe {
        env::set_var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG", &touch_log);
    }

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--instance",
            "default",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
            "--capture",
            "--fields",
            "executed,device,actual_click,guard_result",
        ],
        true,
    );
    unsafe {
        env::remove_var("ACTINGCOMMAND_TEST_FAKE_TOUCH_LOG");
    }

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "validation_failed"
    );
    assert!(!touch_log.exists());
}

#[test]
fn lab2_synthetic_cross_game_pack_runs_core_verbs_without_game_flag() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = synthetic_game_resource_root();
    let scene = temp.path().join("synthetic-home.png");
    fs::write(&scene, encode_png(1, 1, [10, 20, 30])).unwrap();
    let pack = temp.path().join("synthetic.pack.json");
    let pages = temp.path().join("synthetic.pages.json");
    let navigation = temp.path().join("synthetic.navigation.json");
    let shared = [
        "--pack",
        pack.to_str().unwrap(),
        "--pack-root",
        temp.path().to_str().unwrap(),
        "--pages",
        pages.to_str().unwrap(),
        "--navigation",
        navigation.to_str().unwrap(),
    ];

    let observe = run_semantic_cli(
        &temp,
        ["--json", "observe"]
            .into_iter()
            .chain(shared.iter().copied())
            .chain(["--scene", scene.to_str().unwrap()])
            .collect::<Vec<_>>(),
        true,
    );
    assert_eq!(observe.exit_code(), 0, "{}", observe.envelope_json());
    assert_eq!(
        observe
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("page")
            .and_then(Value::as_str),
        Some("synthetic/home")
    );

    let do_result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "do",
            "synthetic_button",
            "--pack",
            pack.to_str().unwrap(),
            "--pack-root",
            temp.path().to_str().unwrap(),
            "--pages",
            pages.to_str().unwrap(),
            "--navigation",
            navigation.to_str().unwrap(),
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(do_result.exit_code(), 0, "{}", do_result.envelope_json());

    let ensure = run_semantic_cli(
        &temp,
        [
            "--json",
            "--dry-run",
            "ensure",
            "synthetic/target",
            "--pack",
            pack.to_str().unwrap(),
            "--pack-root",
            temp.path().to_str().unwrap(),
            "--pages",
            pages.to_str().unwrap(),
            "--navigation",
            navigation.to_str().unwrap(),
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(ensure.exit_code(), 0, "{}", ensure.envelope_json());

    let wait = run_semantic_cli(
        &temp,
        [
            "--json",
            "wait",
            "--page",
            "synthetic/home",
            "--pack",
            pack.to_str().unwrap(),
            "--pack-root",
            temp.path().to_str().unwrap(),
            "--pages",
            pages.to_str().unwrap(),
            "--navigation",
            navigation.to_str().unwrap(),
            "--scene",
            scene.to_str().unwrap(),
            "--timeout-ms",
            "100",
        ],
        true,
    );
    assert_eq!(wait.exit_code(), 0, "{}", wait.envelope_json());
}

#[test]
fn lab2_observe_uses_delayed_stub_capture_and_stays_under_budget() {
    let _guard = env_lock();
    let _app_env = set_isolated_app_env();
    unsafe {
        set_missing_config_env();
        env::remove_var(SESSION_STATE_ENV);
    }
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let started = Instant::now();
    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "observe",
            "--scene",
            scene.to_str().unwrap(),
            "--test-capture-delay-ms",
            "25",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert!(started.elapsed() < Duration::from_millis(300));
    assert_eq!(
        result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("backend")
            .and_then(Value::as_str),
        Some("test_stub_capture")
    );
}

#[test]
fn session_recover_standby_dry_run_uses_wake_control_point() {
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("standby.png");
    fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

    let result = run_cli(
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "arknights",
            "--server",
            "cn",
            "session",
            "recover",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let steps = result
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("steps")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(steps[0].get("type").and_then(Value::as_str), Some("wake"));
    let point = steps[0]
        .get("control_point")
        .and_then(|value| value.get("input"))
        .and_then(|value| value.get("point"))
        .unwrap();
    assert_eq!(point.get("x").and_then(Value::as_i64), Some(3));
    assert_eq!(point.get("y").and_then(Value::as_i64), Some(4));
}

#[test]
fn session_recover_dry_run_plans_route_to_home() {
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("target.png");
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

    let result = run_cli(
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "arknights",
            "--server",
            "cn",
            "session",
            "recover",
            "--to",
            "home",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    let route = result
        .envelope
        .data
        .as_ref()
        .unwrap()
        .get("route")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(
        route[0].get("id").and_then(Value::as_str),
        Some("target_to_home")
    );
}

#[test]
fn session_recover_real_execution_requires_capture() {
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("target.png");
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();

    let result = run_cli(
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "arknights",
            "--server",
            "cn",
            "session",
            "recover",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2);
    assert!(
        result
            .envelope
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("requires --capture")
    );
}

#[test]
fn session_recover_startup_login_dry_run_reads_resource_file() {
    let temp = semantic_resource_root(false);
    write_startup_login_resource(temp.path());
    let scene = temp.path().join("standby.png");
    fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

    let result = run_cli(
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "arknights",
            "--server",
            "cn",
            "session",
            "recover",
            "--startup-login",
            "--to",
            "home",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(data.get("status").and_then(Value::as_str), Some("planned"));
    assert_eq!(
        data.pointer("/startup_login/actions_per_round/0/input/point/x")
            .and_then(Value::as_i64),
        Some(1205)
    );
    assert_eq!(
        data.pointer("/startup_login/actions_per_round/1/input/point/y")
            .and_then(Value::as_i64),
        Some(360)
    );
    assert_eq!(
        data.get("safety_gate").and_then(Value::as_str),
        Some("maintenance_login_only")
    );
}

#[test]
fn session_recover_startup_login_missing_resource_is_fatal() {
    let temp = semantic_resource_root(false);
    let scene = temp.path().join("standby.png");
    fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

    let result = run_cli(
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "arknights",
            "--server",
            "cn",
            "session",
            "recover",
            "--startup-login",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 3);
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "startup_login_resource_missing"
    );
}

#[test]
fn session_recover_startup_login_missing_coordinate_is_fatal() {
    let temp = semantic_resource_root(false);
    fs::write(
        temp.path().join("STARTUP-LOGIN.md"),
        "# startup\n| 推进/点击继续 | (640, 360) |\n",
    )
    .unwrap();
    let scene = temp.path().join("standby.png");
    fs::write(&scene, encode_png(1, 1, [1, 1, 1])).unwrap();

    let result = run_cli(
        [
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "arknights",
            "--server",
            "cn",
            "session",
            "recover",
            "--startup-login",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 3);
    assert_eq!(
        result.envelope.error.as_ref().unwrap().code,
        "startup_login_coordinate_missing"
    );
}

#[test]
fn locate_template_returns_coordinates() {
    let temp = TempDir::new().unwrap();
    let scene = temp.path().join("scene.png");
    let template = temp.path().join("template.png");
    fs::write(&scene, encode_png(1, 1, [7, 8, 9])).unwrap();
    fs::write(&template, encode_png(1, 1, [7, 8, 9])).unwrap();

    let result = run_cli(
        [
            "--json",
            "locate",
            template.to_str().unwrap(),
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0);
    assert_eq!(
        result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("x")
            .and_then(Value::as_i64),
        Some(0)
    );
}

fn write_test_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(content).unwrap();
    }
    zip.finish().unwrap();
}

fn write_startup_login_resource(root: &Path) {
    fs::write(
        root.join("STARTUP-LOGIN.md"),
        "# startup\n| **弹窗关闭 ×** | **(1205, 67)** |\n| 推进/点击继续 | (640, 360) |\n",
    )
    .unwrap();
}

fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
    for _y in 0..height {
        scanlines.push(0);
        for _x in 0..width {
            scanlines.extend_from_slice(&color);
        }
    }

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);

    let mut zlib = vec![0x78, 0x01];
    write_uncompressed_deflate(&mut zlib, &scanlines);
    zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn encode_rgb_png(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<u8> {
    assert_eq!(pixels.len(), (width * height) as usize);
    let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
    for row in pixels.chunks(width as usize) {
        scanlines.push(0);
        for pixel in row {
            scanlines.extend_from_slice(pixel);
        }
    }

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);

    let mut zlib = vec![0x78, 0x01];
    write_uncompressed_deflate(&mut zlib, &scanlines);
    zlib.extend_from_slice(&adler32(&scanlines).to_be_bytes());
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_uncompressed_deflate(out: &mut Vec<u8>, data: &[u8]) {
    for (index, chunk) in data.chunks(65_535).enumerate() {
        let is_last = index == data.len().div_ceil(65_535) - 1;
        out.push(u8::from(is_last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(kind.len() + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in data {
        a = (a + u32::from(*byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
