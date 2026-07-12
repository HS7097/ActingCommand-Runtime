// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{IdentifierIssuer, InstanceId};
use actingcommand_device::{
    CaptureBackend, CaptureBackendName, DeviceError, DeviceResult, Frame, InputBackend, PixelFormat,
};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

const GOLDEN_PATH: &str = "tests/golden/expected.json";
const RECORD_ENV: &str = "ACTINGLAB_RECORD_GOLDENS";

#[derive(Debug, Clone, Copy)]
struct CaseSpec {
    name: &'static str,
    command: &'static str,
    expected_kind: ExpectedKind,
    preparation: Preparation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedKind {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy)]
enum Preparation {
    None,
    Detect,
    DetectThenStale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct GoldenCase {
    name: String,
    command: String,
    exit_code: i32,
    envelope: Value,
}

const CASES: &[CaseSpec] = &[
    case("recognize_success", "recognize", ExpectedKind::Success),
    case("recognize_failure", "recognize", ExpectedKind::Failure),
    case("detect_page_success", "detect-page", ExpectedKind::Success),
    case("detect_page_failure", "detect-page", ExpectedKind::Failure),
    case(
        "current_page_success",
        "current-page",
        ExpectedKind::Success,
    ),
    case(
        "current_page_failure",
        "current-page",
        ExpectedKind::Failure,
    ),
    case("is_visible_success", "is-visible", ExpectedKind::Success),
    case("is_visible_failure", "is-visible", ExpectedKind::Failure),
    case("tap_target_success", "tap-target", ExpectedKind::Success),
    case("tap_target_failure", "tap-target", ExpectedKind::Failure),
    case("navigate_success", "navigate", ExpectedKind::Success),
    case("navigate_failure", "navigate", ExpectedKind::Failure),
    case(
        "package_validate_success",
        "package validate",
        ExpectedKind::Success,
    ),
    case(
        "package_validate_failure",
        "package validate",
        ExpectedKind::Failure,
    ),
    case(
        "package_build_task_success",
        "package build-task",
        ExpectedKind::Success,
    ),
    case(
        "package_build_task_failure",
        "package build-task",
        ExpectedKind::Failure,
    ),
    case(
        "lab_validate_success",
        "lab validate",
        ExpectedKind::Success,
    ),
    case(
        "lab_validate_failure",
        "lab validate",
        ExpectedKind::Failure,
    ),
    case("lab_run_success", "lab run", ExpectedKind::Success),
    case("lab_run_failure", "lab run", ExpectedKind::Failure),
    case("detect_success", "detect", ExpectedKind::Success),
    case("detect_failure", "detect", ExpectedKind::Failure),
    prepared_case(
        "env_resolve_success",
        "env resolve",
        ExpectedKind::Success,
        Preparation::Detect,
    ),
    case("env_resolve_failure", "env resolve", ExpectedKind::Failure),
    prepared_case(
        "env_status_success",
        "env status",
        ExpectedKind::Success,
        Preparation::Detect,
    ),
    prepared_case(
        "env_status_failure",
        "env status",
        ExpectedKind::Failure,
        Preparation::DetectThenStale,
    ),
    case("observe_success", "observe", ExpectedKind::Success),
    case("observe_failure", "observe", ExpectedKind::Failure),
    case("do_success", "do", ExpectedKind::Success),
    case("do_failure", "do", ExpectedKind::Failure),
];

const fn case(name: &'static str, command: &'static str, expected_kind: ExpectedKind) -> CaseSpec {
    prepared_case(name, command, expected_kind, Preparation::None)
}

const fn prepared_case(
    name: &'static str,
    command: &'static str,
    expected_kind: ExpectedKind,
    preparation: Preparation,
) -> CaseSpec {
    CaseSpec {
        name,
        command,
        expected_kind,
        preparation,
    }
}

#[test]
fn protocol_goldens_match_current_cli() {
    let actual = capture_cases();
    if env::var_os(RECORD_ENV).is_some() {
        write_goldens(&actual);
        return;
    }

    let expected = read_goldens();
    assert_eq!(
        expected.len(),
        CASES.len(),
        "golden case count must cover the complete A1 matrix"
    );
    assert_eq!(actual, expected);
}

#[test]
fn matrix_has_fifteen_commands_with_success_and_failure_paths() {
    let mut commands = BTreeMap::<&str, Vec<ExpectedKind>>::new();
    for case in CASES {
        commands
            .entry(case.command)
            .or_default()
            .push(case.expected_kind);
    }

    assert_eq!(commands.len(), 15);
    for (command, kinds) in commands {
        assert!(
            kinds.contains(&ExpectedKind::Success),
            "{command} is missing a success path"
        );
        assert!(
            kinds.contains(&ExpectedKind::Failure),
            "{command} is missing a failure path"
        );
    }
}

#[test]
fn normalizer_replaces_only_dynamic_protocol_fields() {
    let temp = TempDir::new().expect("temp");
    let mut value = json!({
        "req_id": "req_123",
        "reco_id": "reco_456",
        "detector_id": "detect_resolution",
        "generated_at_unix_ms": 123,
        "instance_id": "envinst_abcdef",
        "source_result": "detect_resolution@123",
        "input_sha256": "0123456789abcdef",
        "path": temp.path().join("result.json").display().to_string(),
        "schema_version": "0.2",
        "confidence": 0.9876543
    });

    normalize_value(&mut value, temp.path(), None);

    assert_eq!(value["req_id"], "<REQ_ID>");
    assert_eq!(value["reco_id"], "<RECO_ID>");
    assert_eq!(value["generated_at_unix_ms"], "<TIME>");
    assert_eq!(value["instance_id"], "<IID>");
    assert_eq!(value["source_result"], "detect_resolution@<TIME>");
    assert_eq!(value["input_sha256"], "<INPUT_SHA256>");
    assert_eq!(value["path"], "<PATH>");
    assert_eq!(value["detector_id"], "detect_resolution");
    assert_eq!(value["schema_version"], "0.2");
    assert_eq!(value["confidence"], 0.9876543);
}

fn capture_cases() -> Vec<GoldenCase> {
    CASES
        .iter()
        .map(|case| {
            let fixture = Fixture::new(case.name);
            fixture.prepare(case.preparation);
            let output = fixture.run(&fixture.args(case.name));
            assert_protocol_channels(case, &output);
            let mut envelope = parse_single_envelope(&output.stdout, case.name);
            assert_expected_kind(case, &output, &envelope);
            normalize_value(&mut envelope, fixture.root(), None);
            GoldenCase {
                name: case.name.to_string(),
                command: case.command.to_string(),
                exit_code: output.status.code().unwrap_or(-1),
                envelope: canonicalize(envelope),
            }
        })
        .collect()
}

fn assert_expected_kind(case: &CaseSpec, output: &Output, envelope: &Value) {
    let ok = envelope.get("ok").and_then(Value::as_bool) == Some(true);
    match case.expected_kind {
        ExpectedKind::Success => assert!(
            output.status.success() && ok,
            "{} was expected to succeed: {}",
            case.name,
            String::from_utf8_lossy(&output.stdout)
        ),
        ExpectedKind::Failure => {
            let semantic_failure = envelope
                .pointer("/data/status")
                .and_then(Value::as_str)
                .is_some_and(|status| matches!(status, "stale" | "needs_detection"));
            assert!(
                !output.status.success() || !ok || semantic_failure,
                "{} was expected to exercise a failure path: {}",
                case.name,
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}

fn assert_protocol_channels(case: &CaseSpec, output: &Output) {
    assert!(
        !output.stdout.is_empty(),
        "{} produced empty stdout",
        case.name
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\"schema_version\"")
            && !stderr.contains("\"runtime_version\"")
            && !stderr.contains("\"command\""),
        "{} leaked protocol data to stderr: {stderr}",
        case.name
    );
}

fn parse_single_envelope(stdout: &[u8], case: &str) -> Value {
    let mut stream = serde_json::Deserializer::from_slice(stdout).into_iter::<Value>();
    let value = stream
        .next()
        .unwrap_or_else(|| panic!("{case} produced no JSON envelope"))
        .unwrap_or_else(|err| panic!("{case} produced partial or invalid JSON: {err}"));
    assert!(
        stream.next().is_none(),
        "{case} produced more than one JSON value on stdout"
    );
    value
}

fn normalize_value(value: &mut Value, root: &Path, key: Option<&str>) {
    match value {
        Value::Object(object) => {
            for (field, child) in object {
                normalize_value(child, root, Some(field));
            }
        }
        Value::Array(values) => {
            for child in values {
                normalize_value(child, root, key);
            }
        }
        Value::String(text) => normalize_string(text, root, key),
        Value::Number(_) if key.is_some_and(|field| field.ends_with("_unix_ms")) => {
            *value = Value::String("<TIME>".to_string());
        }
        Value::Number(_)
            if key.is_some_and(|field| {
                field.ends_with("_at_ms") || matches!(field, "holder_pid")
            }) =>
        {
            *value = Value::String(
                if key == Some("holder_pid") {
                    "<PID>"
                } else {
                    "<TIME>"
                }
                .to_string(),
            );
        }
        Value::Number(_)
            if key.is_some_and(|field| {
                matches!(field, "total_before_bytes" | "total_after_bytes")
            }) =>
        {
            *value = Value::String("<BYTES>".to_string());
        }
        _ => {}
    }
}

fn normalize_string(text: &mut String, root: &Path, key: Option<&str>) {
    match key {
        Some("req_id") => {
            *text = "<REQ_ID>".to_string();
            return;
        }
        Some("reco_id") => {
            *text = "<RECO_ID>".to_string();
            return;
        }
        Some("run_id") => {
            *text = "<RUN_ID>".to_string();
            return;
        }
        Some("action_id") => {
            *text = "<ACTION_ID>".to_string();
            return;
        }
        Some("lease_id") => {
            *text = "<LEASE_ID>".to_string();
            return;
        }
        Some("output_zip_sha256") => {
            *text = "<OUTPUT_SHA256>".to_string();
            return;
        }
        Some("input_sha256") => {
            *text = "<INPUT_SHA256>".to_string();
            return;
        }
        Some("instance_id") if text.starts_with("envinst_") => {
            *text = "<IID>".to_string();
            return;
        }
        Some(field) if path_field(field) && Path::new(text).is_absolute() => {
            *text = "<PATH>".to_string();
            return;
        }
        _ => {}
    }

    if text.starts_with("envinst_") {
        *text = "<IID>".to_string();
        return;
    }
    if key == Some("source_result")
        && let Some((prefix, timestamp)) = text.rsplit_once('@')
        && timestamp
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        *text = format!("{prefix}@<TIME>");
        return;
    }
    let root_text = root.display().to_string();
    let slash_root = root_text.replace('\\', "/");
    *text = text
        .replace(&root_text, "<PATH>")
        .replace(&slash_root, "<PATH>");
    replace_embedded_instance_id(text);
}

fn replace_embedded_instance_id(text: &mut String) {
    let Some(start) = text.find("envinst_") else {
        return;
    };
    let suffix = &text[start + "envinst_".len()..];
    let id_len = suffix
        .chars()
        .take_while(|character| character.is_ascii_lowercase() || ('2'..='7').contains(character))
        .count();
    if id_len == 0 {
        return;
    }
    text.replace_range(start..start + "envinst_".len() + id_len, "<IID>");
}

fn path_field(field: &str) -> bool {
    matches!(
        field,
        "path"
            | "zip"
            | "out"
            | "repo"
            | "resource_root"
            | "result_zip"
            | "run_dir"
            | "result_path"
            | "ledger_path"
            | "manifest"
            | "operation"
            | "pack"
            | "pages"
            | "navigation"
            | "source_result"
    ) || field.ends_with("_path")
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect::<Map<_, _>>())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

fn read_goldens() -> Vec<GoldenCase> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH);
    let bytes = fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "failed to read {}: {err}; run scripts/actinglab/record-goldens.ps1 explicitly",
            path.display()
        )
    });
    serde_json::from_slice(&bytes).expect("static golden expectations must be valid JSON")
}

fn write_goldens(cases: &[GoldenCase]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH);
    let mut bytes = serde_json::to_vec_pretty(cases).expect("serialize goldens");
    bytes.push(b'\n');
    fs::write(&path, bytes).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

struct Fixture {
    temp: TempDir,
    resource_root: PathBuf,
    package_repo: PathBuf,
    state_root: PathBuf,
    runtime_root: PathBuf,
    run_root: PathBuf,
    red_scene: PathBuf,
    blue_scene: PathBuf,
    missing_scene: PathBuf,
    invalid_pages: PathBuf,
    safe_package: PathBuf,
    bad_hash_package: PathBuf,
    lab_package: PathBuf,
    fake_adb: PathBuf,
    _runtime_host: Option<RuntimeHost>,
}

impl Fixture {
    fn new(case: &str) -> Self {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        let resource_root = root.join("resources");
        let package_repo = root.join("package-repo");
        let state_root = root.join("state");
        let runtime_root = state_root.join("runtime");
        let run_root = root.join("runs");
        let red_scene = root.join("red.png");
        let blue_scene = root.join("blue.png");
        let lab_scene = root.join("lab-scene.png");
        let missing_scene = root.join("missing.png");
        let invalid_pages = root.join("invalid-pages.json");
        let safe_package = root.join("safe.zip");
        let bad_hash_package = root.join("bad-hash.zip");
        let lab_package = root.join("lab.zip");

        fs::create_dir_all(&state_root).expect("state root");
        fs::create_dir_all(&run_root).expect("run root");
        fs::write(&red_scene, encode_png(1, 1, [255, 0, 0])).expect("red scene");
        fs::write(&blue_scene, encode_png(1, 1, [0, 0, 255])).expect("blue scene");
        fs::write(&lab_scene, encode_png(2, 2, [255, 0, 0])).expect("lab scene");
        fs::write(&invalid_pages, b"not-json").expect("invalid pages");
        write_semantic_resources(&resource_root);
        write_package_repo(&package_repo);
        write_safe_packages(&safe_package, &bad_hash_package);
        write_lab_package(&lab_package, &encode_png(2, 2, [255, 0, 0]));
        let fake_adb = write_fake_adb(root, &lab_scene, case);
        let runtime_host = (case == "lab_run_success").then(|| {
            let instance_id = *IdentifierIssuer::new()
                .expect("identifier issuer")
                .mint_instance_id()
                .expect("instance id")
                .transport();
            RuntimeHost::start(
                RuntimeHostConfig::new(&runtime_root, b"actinglab-golden-runtime"),
                Arc::new(GoldenRuntimeProvider { instance_id }),
            )
            .expect("golden Runtime host")
        });

        Self {
            temp,
            resource_root,
            package_repo,
            state_root,
            runtime_root,
            run_root,
            red_scene,
            blue_scene,
            missing_scene,
            invalid_pages,
            safe_package,
            bad_hash_package,
            lab_package,
            fake_adb,
            _runtime_host: runtime_host,
        }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn prepare(&self, preparation: Preparation) {
        match preparation {
            Preparation::None => {}
            Preparation::Detect | Preparation::DetectThenStale => {
                let output = self.run(&self.detect_args("detect_resolution"));
                assert_eq!(
                    output.status.code(),
                    Some(0),
                    "env preparation failed: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
                if matches!(preparation, Preparation::DetectThenStale) {
                    let catalog = self.resource_root.join("env-detection/detections.json");
                    let mut value: Value =
                        serde_json::from_slice(&fs::read(&catalog).expect("catalog"))
                            .expect("catalog JSON");
                    value["detections"][0]["version"] = json!("2");
                    fs::write(
                        catalog,
                        serde_json::to_vec_pretty(&value).expect("stale catalog"),
                    )
                    .expect("write stale catalog");
                }
            }
        }
    }

    fn run(&self, args: &[OsString]) -> Output {
        let binary = env!("CARGO_BIN_EXE_actinglab");
        Command::new(binary)
            .args(args)
            .current_dir(self.root())
            .env(
                "ACTINGLAB_CONFIG_PATH",
                self.root().join("missing-config.json"),
            )
            .env("LOCALAPPDATA", &self.state_root)
            .env("APPDATA", &self.state_root)
            .env(
                "ACTINGLAB_SESSION_STATE_DIR",
                self.state_root.join("session"),
            )
            .env("ACTINGCOMMAND_ADB_PATH", &self.fake_adb)
            .env("ACTINGCOMMAND_RUNTIME_STATE_ROOT", &self.runtime_root)
            .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
            .env_remove("ACTINGLAB_TRUSTED_REMOTE_TOKEN")
            .env_remove("ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT")
            .output()
            .expect("run actinglab")
    }

    fn args(&self, name: &str) -> Vec<OsString> {
        let common = || {
            vec![
                os("--json"),
                os("--resource-root"),
                self.resource_root.clone().into_os_string(),
                os("--run-root"),
                self.run_root.clone().into_os_string(),
                os("--game"),
                os("ark"),
                os("--server"),
                os("cn"),
            ]
        };
        let scene = |base: &mut Vec<OsString>, path: &Path| {
            base.push(os("--scene"));
            base.push(path.as_os_str().to_os_string());
        };
        let mut args = common();

        match name {
            "recognize_success" => {
                args.extend([os("recognize"), os("--target"), os("home_anchor")]);
                scene(&mut args, &self.red_scene);
            }
            "recognize_failure" => args.push(os("recognize")),
            "detect_page_success" => {
                args.push(os("detect-page"));
                scene(&mut args, &self.red_scene);
            }
            "detect_page_failure" => {
                args.extend([
                    os("detect-page"),
                    os("--pack"),
                    self.resource_root
                        .join("recognition/arknights.cn.pack.json")
                        .into_os_string(),
                    os("--pack-root"),
                    self.resource_root.clone().into_os_string(),
                    os("--pages"),
                ]);
                args.push(self.invalid_pages.clone().into_os_string());
                scene(&mut args, &self.red_scene);
            }
            "current_page_success" => {
                args.push(os("current-page"));
                scene(&mut args, &self.red_scene);
            }
            "current_page_failure" => {
                args.push(os("current-page"));
                scene(&mut args, &self.missing_scene);
            }
            "is_visible_success" => {
                args.extend([os("is-visible"), os("home_button")]);
                scene(&mut args, &self.red_scene);
            }
            "is_visible_failure" => args.push(os("is-visible")),
            "tap_target_success" => {
                args.extend([os("--dry-run"), os("tap-target"), os("home_button")]);
                scene(&mut args, &self.red_scene);
            }
            "tap_target_failure" => {
                args.extend([os("--dry-run"), os("tap-target"), os("home_button")]);
                scene(&mut args, &self.blue_scene);
            }
            "navigate_success" => {
                args.extend([os("--dry-run"), os("navigate"), os("--to"), os("target")]);
                scene(&mut args, &self.red_scene);
            }
            "navigate_failure" => {
                args.extend([os("--dry-run"), os("navigate"), os("--to"), os("missing")]);
                scene(&mut args, &self.red_scene);
            }
            "package_validate_success" => {
                args.extend([os("package"), os("validate"), os("--zip")]);
                args.push(self.safe_package.clone().into_os_string());
            }
            "package_validate_failure" => {
                args.extend([os("package"), os("validate"), os("--zip")]);
                args.push(self.bad_hash_package.clone().into_os_string());
            }
            "package_build_task_success" => {
                args.extend([os("--dry-run"), os("package"), os("build-task")]);
                args.extend([os("--repo"), self.package_repo.clone().into_os_string()]);
                args.extend([os("--task"), os("operator_task")]);
                args.extend([os("--out"), self.root().join("task.zip").into_os_string()]);
            }
            "package_build_task_failure" => {
                args.extend([os("--dry-run"), os("package"), os("build-task")]);
                args.extend([os("--repo"), self.package_repo.clone().into_os_string()]);
                args.extend([os("--task"), os("missing_task")]);
                args.extend([os("--out"), self.root().join("task.zip").into_os_string()]);
            }
            "lab_validate_success" => {
                args.extend([os("lab"), os("validate"), os("--zip")]);
                args.push(self.lab_package.clone().into_os_string());
            }
            "lab_validate_failure" => {
                args.extend([os("lab"), os("validate"), os("--zip")]);
                args.push(self.safe_package.clone().into_os_string());
            }
            "lab_run_success" => {
                let expected = sha256_file(&self.lab_package);
                args.extend([
                    os("--instance"),
                    os("fixture:5555"),
                    os("--capture-backend"),
                    os("adb"),
                    os("lab"),
                    os("run"),
                    os("--zip"),
                    self.lab_package.clone().into_os_string(),
                    os("--expected-sha256"),
                    os(&expected),
                    os("--out"),
                    self.root().join("lab-result.zip").into_os_string(),
                ]);
            }
            "lab_run_failure" => args.extend([os("lab"), os("run")]),
            "detect_success" => return self.detect_args("detect_resolution"),
            "detect_failure" => return self.detect_args("missing_detector"),
            "env_resolve_success" => {
                args.extend([
                    os("--instance"),
                    os("fixture:5555"),
                    os("env"),
                    os("resolve"),
                    os("--task"),
                    os("detect_resolution"),
                    os("--key"),
                    os("resolution"),
                ]);
            }
            "env_resolve_failure" => {
                args.extend([
                    os("--instance"),
                    os("fixture:5555"),
                    os("env"),
                    os("resolve"),
                    os("--task"),
                    os("detect_resolution"),
                    os("--key"),
                    os("resolution"),
                ]);
            }
            "env_status_success" | "env_status_failure" => {
                args.extend([
                    os("--instance"),
                    os("fixture:5555"),
                    os("env"),
                    os("status"),
                    os("--task"),
                    os("detect_resolution"),
                ]);
            }
            "observe_success" => {
                args.extend([
                    os("observe"),
                    os("--targets"),
                    os("home_button"),
                    os("--with-frame"),
                    self.root().join("observe.png").into_os_string(),
                ]);
                scene(&mut args, &self.red_scene);
            }
            "observe_failure" => {
                args.push(os("observe"));
                scene(&mut args, &self.missing_scene);
            }
            "do_success" => {
                args.extend([os("--dry-run"), os("do"), os("home_button")]);
                scene(&mut args, &self.red_scene);
            }
            "do_failure" => {
                args.extend([os("--dry-run"), os("do"), os("home_button")]);
                scene(&mut args, &self.blue_scene);
            }
            other => panic!("unknown golden case {other}"),
        }
        args
    }

    fn detect_args(&self, detector: &str) -> Vec<OsString> {
        vec![
            os("--json"),
            os("--resource-root"),
            self.resource_root.clone().into_os_string(),
            os("--run-root"),
            self.run_root.clone().into_os_string(),
            os("--game"),
            os("ark"),
            os("--server"),
            os("cn"),
            os("--instance"),
            os("fixture:5555"),
            os("detect"),
            os("--task"),
            os(detector),
            os("--scene"),
            self.red_scene.clone().into_os_string(),
        ]
    }
}

struct GoldenRuntimeProvider {
    instance_id: InstanceId,
}

struct GoldenCapture;

impl CaptureBackend for GoldenCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        Frame::from_pixels(
            2,
            2,
            vec![255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }
}

impl ExecutionBackendProvider for GoldenRuntimeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        vec!["fixture:5555".to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == "fixture:5555")
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<sealed-golden>"))
    }

    fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
        Err(DeviceError::fatal(
            "recognize-only golden run must not open input",
        ))
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
        if instance_alias != "fixture:5555" {
            return Err(DeviceError::fatal("unexpected golden instance"));
        }
        Ok(Box::new(GoldenCapture))
    }
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).expect("read package for external expected hash");
    format!("{:x}", Sha256::digest(bytes))
}

fn os(value: &str) -> OsString {
    OsString::from(value)
}

fn write_semantic_resources(root: &Path) {
    let recognition = root.join("recognition");
    let navigation = root.join("navigation");
    let env_detection = root.join("env-detection");
    fs::create_dir_all(&recognition).expect("recognition");
    fs::create_dir_all(&navigation).expect("navigation");
    fs::create_dir_all(&env_detection).expect("env detection");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"target_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
            ]
        }"#,
    )
    .expect("pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[
                {"id":"arknights/home","required":["home_anchor"]},
                {"id":"arknights/target","required":["target_anchor"]}
            ]
        }"#,
    )
    .expect("pages");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"arknights",
            "server":"cn",
            "navigation":[{
                "id":"home_to_target",
                "from_page":"arknights/home",
                "to_page":"arknights/target",
                "click":{"kind":"rect","x":10,"y":20,"width":4,"height":6}
            }],
            "destructive_actions":[]
        }"#,
    )
    .expect("navigation");
    fs::write(
        env_detection.join("detections.json"),
        r#"{
            "schema_version":"env-detections.v1",
            "detections":[{
                "id":"detect_resolution",
                "version":"1",
                "game_id":"arknights",
                "server_id":"cn",
                "resource_pack_id":"golden-pack",
                "keys":[{
                    "key":"resolution",
                    "min_confidence":1.0,
                    "stale_below_confidence":1.0,
                    "ttl_ms":null,
                    "allowed_values":["1x1"],
                    "candidates":[{"value":"1x1","width":1,"height":1,"source":"golden-scene"}]
                }]
            }]
        }"#,
    )
    .expect("detections");
}

fn write_package_repo(root: &Path) {
    let assets = root.join("operations/operator_task/assets");
    fs::create_dir_all(&assets).expect("package assets");
    fs::create_dir_all(root.join("navigation")).expect("package navigation");
    fs::write(
        root.join("operations/resources.json"),
        r#"{"schema_version":"1.0","resources":[],"resource_count":0}"#,
    )
    .expect("package resources");
    fs::write(assets.join("HOME.png"), encode_png(1, 1, [255, 0, 0])).expect("package asset");
    fs::write(
        root.join("operations/operator_task/task.json"),
        r#"{
            "schema_version":"0.3",
            "task_id":"operator_task",
            "game":"arknights",
            "server_scope":["cn"],
            "goal":"golden fixture",
            "coordinate_space":{"width":1280,"height":720},
            "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
            "anchors":[{"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},"threshold":0.8,"color_check":null}],
            "entry_page":"home",
            "target_page":"home",
            "operations":[{
                "id":"noop",
                "purpose":"golden fixture",
                "from":"home",
                "to":null,
                "click":{"kind":"rect","x":1,"y":1,"width":1,"height":1},
                "verify_template":null,
                "unguarded_trusted_coordinate":true,
                "consumes":[],
                "produces":[]
            }]
        }"#,
    )
    .expect("package task");
    fs::write(
        root.join("navigation/arknights.cn.navigation.json"),
        r#"{"schema_version":"0.3","control_points":[{"name":"home","point":[1,1]}]}"#,
    )
    .expect("package navigation file");
}

fn write_safe_packages(safe: &Path, bad_hash: &Path) {
    write_zip(
        safe,
        &[
            ("module/manifest.json", br#"{"schema_version":"0.2"}"#),
            ("module/operations/task/task.json", br#"{"id":"task"}"#),
            ("module/operations/resources.json", br#"{}"#),
        ],
    );
    write_zip(
        bad_hash,
        &[
            (
                "module/manifest.json",
                br#"{"hashes":{"operations/resources.json":"sha256:0000"}}"#,
            ),
            ("module/operations/task/task.json", br#"{}"#),
            ("module/operations/resources.json", br#"{}"#),
        ],
    );
}

fn write_lab_package(path: &Path, scene: &[u8]) {
    write_zip(
        path,
        &[
            (
                "control.json",
                br#"{
                    "schema_version":"Lab-1y.control.v1",
                    "package_id":"golden.task",
                    "execution_mode":"recognize_only",
                    "game":"arknights",
                    "server":"cn",
                    "resolution":{"width":2,"height":2},
                    "entry_task_id":"task"
                }"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            (
                "resources/operations/task/task.json",
                br#"{
                    "schema_version":"0.3",
                    "task_id":"task",
                    "game":"arknights",
                    "server_scope":["cn"],
                    "goal":"golden fixture",
                    "coordinate_space":{"width":2,"height":2},
                    "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                    "anchors":[{"id":"home","template":"assets/HOME.png"}],
                    "entry_page":"home",
                    "target_page":"home",
                    "operations":[{
                        "id":"noop",
                        "purpose":"golden fixture",
                        "from":"home",
                        "to":null,
                        "click":{"kind":"point","x":1,"y":1},
                        "verify_template":null,
                        "unguarded_trusted_coordinate":true,
                        "consumes":[],
                        "produces":[]
                    }]
                }"#,
            ),
            ("resources/operations/task/assets/HOME.png", scene),
            (
                "resources/recognition/arknights.cn.pack.json",
                br#"{
                    "schema_version":"0.3",
                    "game":"arknights",
                    "server":"cn",
                    "coordinate_space":{"width":2,"height":2},
                    "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                    "targets":[{
                        "type":"template",
                        "id":"page/home",
                        "template_path":"operations/task/assets/HOME.png",
                        "region":{"x":0,"y":0,"width":2,"height":2},
                        "threshold":0.9
                    }]
                }"#,
            ),
            (
                "resources/recognition/arknights.cn.pages.json",
                br#"{"schema_version":"0.3","pages":[{"id":"arknights/home","required":["page/home"],"optional":[],"forbidden":[]}]}"#,
            ),
        ],
    );
}

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("zip file");
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in files {
        zip.start_file(*name, options).expect("zip entry");
        zip.write_all(content).expect("zip content");
    }
    zip.finish().expect("finish zip");
}

fn write_fake_adb(root: &Path, scene: &Path, case: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let path = root.join(format!("fake-adb-{case}.cmd"));
        let scene = scene.display().to_string();
        let script = format!(
            "@echo off\r\nset args=%*\r\necho %args% | findstr /C:\"connect \" >nul && (echo connected& exit /b 0)\r\necho %args% | findstr /C:\"get-state\" >nul && (echo device& exit /b 0)\r\necho %args% | findstr /C:\"exec-out screencap -p\" >nul && (type \"{scene}\"& exit /b 0)\r\necho %args% | findstr /C:\"shell wm size\" >nul && (echo Physical size: 1x1& exit /b 0)\r\nexit /b 1\r\n"
        );
        fs::write(&path, script).expect("fake adb");
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(format!("fake-adb-{case}"));
        let scene = scene.display().to_string();
        let script = format!(
            "#!/bin/sh\ncase \"$*\" in\n  *\"connect \"*) echo connected ;;\n  *\"get-state\"*) echo device ;;\n  *\"exec-out screencap -p\"*) cat \"{scene}\" ;;\n  *\"shell wm size\"*) echo 'Physical size: 1x1' ;;\n  *) exit 1 ;;\nesac\n"
        );
        fs::write(&path, script).expect("fake adb");
        let mut permissions = fs::metadata(&path)
            .expect("fake adb metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("fake adb permissions");
        path
    }
}

fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
    for _ in 0..height {
        scanlines.extend_from_slice(&[0]);
        for _ in 0..width {
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
