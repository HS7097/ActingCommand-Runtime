// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use zip::{ZipArchive, ZipWriter, write::FileOptions};

#[test]
fn package_dry_run_binds_inputs_and_writes_a_deterministic_offline_bundle() {
    let fixture = TestFixture::new(PackageOptions::default(), home_frame(true));
    let first = fixture.run(&[], "first.result.zip");
    assert_success(&first, "would_click");

    let first_path = fixture.temp.path().join("first.result.zip");
    let record = read_result_record(&first_path);
    assert_eq!(
        record["schema_version"],
        "actingcommand.offline-simulation.v1"
    );
    assert_eq!(record["mode"], "offline_simulation");
    assert_eq!(record["executed"], false);
    assert_eq!(record["production_global_ledger_written"], false);
    assert_eq!(record["package_sha256"], fixture.package_sha256);
    let decision_fingerprint = record["decision_fingerprint"].as_str().unwrap();
    assert_eq!(decision_fingerprint.len(), 64);
    assert!(
        decision_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        record["simulation"]["decision_fingerprint"],
        decision_fingerprint
    );
    assert_eq!(
        envelope_data(&first)["decision_fingerprint"],
        decision_fingerprint
    );
    assert_eq!(
        record["simulation"]["package_sha256"],
        fixture.package_sha256
    );
    assert_eq!(record["simulation"]["entry_count"], 6);
    assert_eq!(record["simulation"]["task_count"], 1);
    assert_eq!(record["simulation"]["capture_count"], 1);
    assert_eq!(record["loaded"]["validation"]["status"], "valid");
    assert_eq!(record["loaded"]["task_count"], 1);
    assert_eq!(record["loaded"]["entries"].as_array().unwrap().len(), 6);
    assert_eq!(
        record["loaded"]["validation"]["resources"]["navigation"],
        "resources/navigation/neutral.test.navigation.json"
    );
    let runtime_head = record["runtime_head"].as_str().unwrap();
    assert_eq!(runtime_head.len(), 40);
    assert!(runtime_head.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let second = fixture.run(&[], "second.result.zip");
    assert_success(&second, "would_click");
    let second_path = fixture.temp.path().join("second.result.zip");
    assert_eq!(
        fs::read(first_path).unwrap(),
        fs::read(second_path).unwrap()
    );
    assert_eq!(
        envelope_data(&first)["result_zip_sha256"],
        envelope_data(&second)["result_zip_sha256"]
    );

    let decorated_hash = format!("sha256:{}", fixture.package_sha256.to_uppercase());
    let decorated = fixture.run_with_hash(&decorated_hash, &[], "decorated.result.zip");
    assert_success(&decorated, "would_click");
    let decorated_record = read_result_record(&fixture.temp.path().join("decorated.result.zip"));
    assert_eq!(decorated_record["package_sha256"], fixture.package_sha256);
    assert_eq!(
        decorated_record["decision_fingerprint"],
        record["decision_fingerprint"]
    );
    assert_eq!(
        fs::read(fixture.temp.path().join("first.result.zip")).unwrap(),
        fs::read(fixture.temp.path().join("decorated.result.zip")).unwrap()
    );
}

#[test]
fn package_dry_run_reports_complete_no_op_and_recovery_closure() {
    let complete = TestFixture::new(PackageOptions::default(), terminal_frame());
    assert_success(&complete.run(&[], "complete.zip"), "would_complete");

    let no_op = TestFixture::new(
        PackageOptions {
            execution_mode: "recognize_only",
            ..PackageOptions::default()
        },
        home_frame(true),
    );
    assert_success(&no_op.run(&[], "noop.zip"), "no_op");

    let recovery = TestFixture::new(
        PackageOptions {
            recovery: true,
            include_recovery_task: true,
            ..PackageOptions::default()
        },
        home_frame(true),
    );
    let output = recovery.run(&[], "recovery.zip");
    assert_success(&output, "would_click");
    let record = read_result_record(&recovery.temp.path().join("recovery.zip"));
    assert_eq!(record["loaded"]["task_count"], 2);
    assert_eq!(record["simulation"]["task_count"], 2);
}

#[test]
fn package_dry_run_fails_loud_for_recognition_and_guard_failures() {
    let unknown = TestFixture::new(PackageOptions::default(), solid_frame([0, 0, 0], [0, 0, 0]));
    assert_refusal_receipt(
        &unknown.run(&[], "unknown.zip"),
        &unknown.temp.path().join("unknown.zip"),
        "contained_task_page_unknown",
    );

    let conflict = TestFixture::new(
        PackageOptions {
            conflicting_page: true,
            ..PackageOptions::default()
        },
        home_frame(true),
    );
    assert_refusal_receipt(
        &conflict.run(&[], "conflict.zip"),
        &conflict.temp.path().join("conflict.zip"),
        "contained_task_recognition_conflict",
    );

    let guard = TestFixture::new(PackageOptions::default(), home_frame(false));
    assert_refusal_receipt(
        &guard.run(&[], "guard.zip"),
        &guard.temp.path().join("guard.zip"),
        "contained_task_guard_refused",
    );

    let mismatch = TestFixture::new(PackageOptions::default(), solid_frame_1x1([255, 0, 0]));
    assert_refusal_receipt(
        &mismatch.run(&[], "mismatch.zip"),
        &mismatch.temp.path().join("mismatch.zip"),
        "contained_task_frame_resolution_mismatch",
    );
}

#[test]
fn package_dry_run_rejects_invalid_package_inputs() {
    let corrupt = TestFixture::from_bytes(b"not a zip".to_vec(), home_frame(true));
    assert_failure(&corrupt.run(&[], "corrupt.zip"));

    let wrong_hash = TestFixture::new(PackageOptions::default(), home_frame(true));
    let output = wrong_hash.run_with_hash(&"0".repeat(64), &[], "wrong-hash.zip");
    assert_failure(&output);

    for (name, options) in [
        (
            "old-schema",
            PackageOptions {
                control_schema: "Lab-1y.control.v0",
                ..PackageOptions::default()
            },
        ),
        (
            "dangling-resource",
            PackageOptions {
                dangling_resource: true,
                ..PackageOptions::default()
            },
        ),
        (
            "missing-guard",
            PackageOptions {
                include_guard: false,
                ..PackageOptions::default()
            },
        ),
        (
            "missing-recovery",
            PackageOptions {
                recovery: true,
                ..PackageOptions::default()
            },
        ),
    ] {
        let fixture = TestFixture::new(options, home_frame(true));
        assert_failure(&fixture.run(&[], &format!("{name}.zip")));
    }

    let unsupported = TestFixture::new(
        PackageOptions {
            click_kind: "unsupported",
            ..PackageOptions::default()
        },
        home_frame(true),
    );
    assert_error_code(
        &unsupported.run(&[], "unsupported-primitive.zip"),
        "package_invalid",
    );
}

#[test]
fn package_dry_run_rejects_missing_inputs_and_device_scope() {
    let temp = TempDir::new().unwrap();
    let missing_zip = run_actinglab(&temp, ["--json", "package", "dry-run"]);
    assert_failure(&missing_zip);

    let fixture = TestFixture::new(PackageOptions::default(), home_frame(true));
    let missing_hash = run_actinglab(
        &fixture.temp,
        [
            OsString::from("--json"),
            OsString::from("package"),
            OsString::from("dry-run"),
            OsString::from("--zip"),
            fixture.package_path.as_os_str().to_owned(),
            OsString::from("--fixture"),
            fixture.fixture_path.as_os_str().to_owned(),
            OsString::from("--out"),
            OsString::from("missing-hash.zip"),
        ],
    );
    assert_failure(&missing_hash);

    let missing_fixture = run_actinglab(
        &fixture.temp,
        [
            OsString::from("--json"),
            OsString::from("package"),
            OsString::from("dry-run"),
            OsString::from("--zip"),
            fixture.package_path.as_os_str().to_owned(),
            OsString::from("--expected-sha256"),
            OsString::from(&fixture.package_sha256),
            OsString::from("--out"),
            OsString::from("missing-fixture.zip"),
        ],
    );
    assert_error_code(&missing_fixture, "offline_fixture_missing");

    for (name, extra) in [
        ("instance", vec!["--instance", "neutral-instance"]),
        ("instances-empty", vec!["--instances", ""]),
        ("capture", vec!["--capture-backend", "adb"]),
        ("input", vec!["--touch-backend", "maatouch"]),
        ("runtime", vec!["--runtime-endpoint", "http://127.0.0.1:9"]),
        ("global-dry-run", vec!["--dry-run"]),
    ] {
        let output = fixture.run(&extra, &format!("{name}.zip"));
        assert_error_code(&output, "offline_device_scope_forbidden");
    }
    let mut capability_attempt = fixture.args_with_hash(&fixture.package_sha256, "unsupported.zip");
    capability_attempt.extend([OsString::from("--send-input"), OsString::from("true")]);
    assert_error_code(
        &run_actinglab(&fixture.temp, capability_attempt),
        "offline_device_scope_forbidden",
    );

    let alias_dir = fixture.temp.path().join("alias");
    fs::create_dir(&alias_dir).unwrap();
    let colliding_out = alias_dir.join("..").join("package.zip");
    let output = run_actinglab(
        &fixture.temp,
        [
            OsString::from("--json"),
            OsString::from("package"),
            OsString::from("dry-run"),
            OsString::from("--zip"),
            fixture.package_path.as_os_str().to_owned(),
            OsString::from("--expected-sha256"),
            OsString::from(&fixture.package_sha256),
            OsString::from("--fixture"),
            fixture.fixture_path.as_os_str().to_owned(),
            OsString::from("--out"),
            colliding_out.into_os_string(),
        ],
    );
    assert_error_code(&output, "offline_output_conflicts_with_input");
}

#[test]
fn package_dry_run_cannot_be_false_greened_by_version() {
    let fixture = TestFixture::new(PackageOptions::default(), home_frame(true));
    for (name, version_index) in [("before", 1usize), ("between", 2), ("after", 3)] {
        let out_name = format!("version-{name}.zip");
        let out = fixture.temp.path().join(&out_name);
        let mut args = fixture.args_with_hash(&fixture.package_sha256, &out_name);
        args.insert(version_index, OsString::from("--version"));
        let output = run_actinglab(&fixture.temp, args);
        assert!(!output.status.success());
        let result = envelope(&output);
        assert_eq!(result["ok"], false);
        assert_eq!(result["command"], "package dry-run");
        assert_eq!(result["error"]["code"], "validation_failed");
        assert!(!out.exists(), "{} unexpectedly created", out.display());
    }
}

#[test]
fn package_dry_run_rejects_empty_values_and_duplicate_singletons() {
    let fixture = TestFixture::new(PackageOptions::default(), home_frame(true));

    for (name, flag, value) in [
        ("duplicate-zip", "--zip", fixture.package_path.as_os_str()),
        (
            "duplicate-expected",
            "--expected-sha256",
            OsStr::new(&fixture.package_sha256),
        ),
        ("duplicate-out", "--out", OsStr::new("duplicate-out.zip")),
    ] {
        let mut args = fixture.args_with_hash(&fixture.package_sha256, &format!("{name}.zip"));
        args.extend([OsString::from(flag), value.to_owned()]);
        assert_error_code(
            &run_actinglab(&fixture.temp, args),
            "offline_argument_invalid",
        );
    }

    let mut empty_out = fixture.args_with_hash(&fixture.package_sha256, "empty-out.zip");
    *empty_out.last_mut().unwrap() = OsString::new();
    assert_error_code(
        &run_actinglab(&fixture.temp, empty_out),
        "offline_argument_invalid",
    );
}

#[test]
fn package_dry_run_never_overwrites_existing_or_hard_linked_inputs() {
    let fixture = TestFixture::new(PackageOptions::default(), home_frame(true));
    let package_before = fs::read(&fixture.package_path).unwrap();
    let fixture_before = fs::read(&fixture.fixture_path).unwrap();

    for (name, input) in [
        ("package-alias.zip", &fixture.package_path),
        ("fixture-alias.png", &fixture.fixture_path),
    ] {
        let alias = fixture.temp.path().join(name);
        fs::hard_link(input, &alias).unwrap();
        let output = fixture.run(&[], name);
        assert_error_code(&output, "offline_output_already_exists");
    }
    assert_eq!(fs::read(&fixture.package_path).unwrap(), package_before);
    assert_eq!(fs::read(&fixture.fixture_path).unwrap(), fixture_before);

    let first = fixture.run(&[], "existing-result.zip");
    assert_success(&first, "would_click");
    let existing = fixture.temp.path().join("existing-result.zip");
    let existing_before = fs::read(&existing).unwrap();
    let second = fixture.run(&[], "existing-result.zip");
    assert_error_code(&second, "offline_output_already_exists");
    assert_eq!(fs::read(existing).unwrap(), existing_before);
}

#[test]
fn production_entry_boundaries_remain_explicit() {
    let temp = TempDir::new().unwrap();
    let lab = run_actinglab(&temp, ["--json", "--dry-run", "lab", "run"]);
    assert_error_code(&lab, "explicit_offline_entry_required");

    let fixture = TestFixture::new(PackageOptions::default(), home_frame(true));
    let package_run = run_actinglab(
        &fixture.temp,
        [
            OsString::from("--json"),
            OsString::from("--instance"),
            OsString::from("neutral-instance"),
            OsString::from("package"),
            OsString::from("run"),
            OsString::from("--zip"),
            fixture.package_path.as_os_str().to_owned(),
        ],
    );
    assert_error_code(&package_run, "lab_lease_required");

    let operation_run = run_actinglab(&temp, ["--json", "operation", "run"]);
    assert_error_code(&operation_run, "lab_lease_required");

    let capabilities = run_actinglab(&temp, ["--json", "capabilities"]);
    assert!(capabilities.status.success());
    let capability_data = envelope_data(&capabilities);
    let command = capability_data["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == "package dry-run")
        .unwrap();
    assert_eq!(command["needs"], json!(["offline"]));
    assert_eq!(command["status"], "available");
    assert_eq!(command["executed"], false);

    let lab_run = capability(&capability_data, "lab run");
    assert_eq!(lab_run["needs"], json!(["device"]));
    assert_eq!(lab_run["status"], "available");

    let operation_run = capability(&capability_data, "operation run");
    assert_eq!(
        operation_run["needs"],
        json!(["running_runtime", "device", "lab_lease"])
    );
    assert_eq!(operation_run["status"], "blocked_until_lab_lease");
}

struct TestFixture {
    temp: TempDir,
    package_path: PathBuf,
    fixture_path: PathBuf,
    package_sha256: String,
}

impl TestFixture {
    fn new(options: PackageOptions, fixture_png: Vec<u8>) -> Self {
        Self::from_bytes(package(options), fixture_png)
    }

    fn from_bytes(package_bytes: Vec<u8>, fixture_png: Vec<u8>) -> Self {
        let temp = TempDir::new().unwrap();
        let package_path = temp.path().join("package.zip");
        let fixture_path = temp.path().join("fixture.png");
        fs::write(&package_path, &package_bytes).unwrap();
        fs::write(&fixture_path, fixture_png).unwrap();
        Self {
            temp,
            package_path,
            fixture_path,
            package_sha256: sha256_hex(&package_bytes),
        }
    }

    fn run(&self, extra: &[&str], out_name: &str) -> Output {
        self.run_with_hash(&self.package_sha256, extra, out_name)
    }

    fn run_with_hash(&self, expected: &str, extra: &[&str], out_name: &str) -> Output {
        let mut args = extra.iter().map(OsString::from).collect::<Vec<_>>();
        args.extend(self.args_with_hash(expected, out_name));
        run_actinglab(&self.temp, args)
    }

    fn args_with_hash(&self, expected: &str, out_name: &str) -> Vec<OsString> {
        let out = self.temp.path().join(out_name);
        vec![
            OsString::from("--json"),
            OsString::from("package"),
            OsString::from("dry-run"),
            OsString::from("--zip"),
            self.package_path.as_os_str().to_owned(),
            OsString::from("--expected-sha256"),
            OsString::from(expected),
            OsString::from("--fixture"),
            self.fixture_path.as_os_str().to_owned(),
            OsString::from("--out"),
            out.as_os_str().to_owned(),
        ]
    }
}

fn run_actinglab<I, S>(temp: &TempDir, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(args)
        .env("ACTINGLAB_CONFIG_PATH", temp.path().join("config.json"))
        .output()
        .expect("run actinglab")
}

fn envelope(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse CLI JSON: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn envelope_data(output: &Output) -> Value {
    envelope(output)["data"].clone()
}

fn capability<'a>(data: &'a Value, command: &str) -> &'a Value {
    data["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == command)
        .unwrap()
}

fn assert_success(output: &Output, decision_status: &str) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = envelope(output);
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"]["status"], "offline_simulation");
    assert_eq!(envelope["data"]["executed"], false);
    assert_eq!(envelope["data"]["decision"]["status"], decision_status);
    assert_eq!(envelope["data"]["production_global_ledger_written"], false);
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "expected failure, stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = envelope(output);
    assert_eq!(envelope["ok"], false);
    assert!(
        envelope["error"]["code"]
            .as_str()
            .is_some_and(|code| !code.is_empty()),
        "failure did not expose a stable error code: {envelope}"
    );
}

fn assert_error_code(output: &Output, expected: &str) {
    assert_failure(output);
    assert_eq!(envelope(output)["error"]["code"], expected);
}

fn assert_refusal_receipt(output: &Output, path: &Path, expected: &str) {
    assert_error_code(output, expected);
    let response = envelope(output);
    let details = &response["error"]["details"];
    assert_eq!(details["status"], "refused");
    assert_eq!(details["decision"]["status"], "refused");
    assert_eq!(details["decision"]["code"], expected);
    assert_eq!(details["executed"], false);
    assert_eq!(details["production_global_ledger_written"], false);
    let fingerprint = details["decision_fingerprint"]
        .as_str()
        .expect("refusal decision fingerprint");
    assert_eq!(fingerprint.len(), 64);
    assert!(fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let record = read_result_record(path);
    assert_eq!(record["decision_fingerprint"], fingerprint);
    assert_eq!(record["simulation"]["decision_fingerprint"], fingerprint);
    assert_eq!(record["simulation"]["decision"]["status"], "refused");
    assert_eq!(record["simulation"]["decision"]["code"], expected);
    assert_eq!(record["executed"], false);
    assert_eq!(record["production_global_ledger_written"], false);
}

fn read_result_record(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 1);
    let mut entry = archive.by_name("offline-simulation.json").unwrap();
    let mut json = String::new();
    entry.read_to_string(&mut json).unwrap();
    serde_json::from_str(&json).unwrap()
}

#[derive(Clone, Copy)]
struct PackageOptions {
    control_schema: &'static str,
    execution_mode: &'static str,
    click_kind: &'static str,
    include_guard: bool,
    conflicting_page: bool,
    dangling_resource: bool,
    recovery: bool,
    include_recovery_task: bool,
}

impl Default for PackageOptions {
    fn default() -> Self {
        Self {
            control_schema: "Lab-1y.control.v1",
            execution_mode: "navigable_route",
            click_kind: "point",
            include_guard: true,
            conflicting_page: false,
            dangling_resource: false,
            recovery: false,
            include_recovery_task: false,
        }
    }
}

fn package(options: PackageOptions) -> Vec<u8> {
    let control = json!({
        "schema_version": options.control_schema,
        "package_id": "neutral.semantic.task",
        "execution_mode": options.execution_mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10,
        "timeout_ms": 100,
        "max_steps": 2
    });
    let mut operation = json!({
        "id": "open_terminal",
        "purpose": "exercise shared planning semantics",
        "from": "home",
        "to": "terminal",
        "click": {"kind": options.click_kind, "x": 1, "y": 0}
    });
    if options.include_guard {
        operation["guard"] = json!({
            "page_id": "home",
            "target_id": "guard/ready",
            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
            "color_probe": "guard/ready"
        });
    }
    if options.dangling_resource {
        operation["verify_template"] = json!("assets/missing.png");
    }
    let mut task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "goal": "neutral fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [operation]
    });
    if options.recovery {
        task["recovery"] = json!({"kind": "return_home", "task_id": "return_home"});
    }
    let mut pages = vec![
        json!({"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]}),
        json!({"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}),
    ];
    if options.conflicting_page {
        pages.push(
            json!({"id":"neutral/duplicate","required":["page/home"],"optional":[],"forbidden":[]}),
        );
    }
    let mut entries = vec![
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version":"0.3","entry_task_id":"task"}),
        ),
        ("resources/operations/task/task.json", task),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                    {"type":"color","id":"guard/ready","region":{"x":1,"y":0,"width":1,"height":1},"expected":[0,255,0]}
                ]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({"schema_version":"0.3","pages":pages}),
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "navigation": [],
                "destructive_actions": []
            }),
        ),
    ];
    if options.include_recovery_task {
        entries.push((
            "resources/operations/return_home/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "return_home",
                "game": "neutral",
                "server_scope": ["test"],
                "goal": "recovery closure fixture",
                "coordinate_space": {"width": 2, "height": 1},
                "target_page": "home",
                "operations": [{
                    "id": "return_home",
                    "purpose": "prove referenced task closure",
                    "from": "terminal",
                    "to": "home",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true
                }]
            }),
        ));
    }
    zip_entries(&entries)
}

fn zip_entries(entries: &[(&str, Value)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in entries {
        zip.start_file(*path, options).unwrap();
        serde_json::to_writer(&mut zip, value).unwrap();
        zip.write_all(b"\n").unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn home_frame(guard_passes: bool) -> Vec<u8> {
    solid_frame(
        [255, 0, 0],
        if guard_passes { [0, 255, 0] } else { [0, 0, 0] },
    )
}

fn terminal_frame() -> Vec<u8> {
    solid_frame([0, 0, 255], [0, 0, 0])
}

fn solid_frame(left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
    frame_png(2, 1, [left, right].concat())
}

fn solid_frame_1x1(color: [u8; 3]) -> Vec<u8> {
    frame_png(1, 1, color.to_vec())
}

fn frame_png(width: u32, height: u32, pixels: Vec<u8>) -> Vec<u8> {
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .unwrap()
    .png_for_artifact()
    .unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
