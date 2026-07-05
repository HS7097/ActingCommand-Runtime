// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use tempfile::TempDir;

fn actinglab_binary() -> &'static str {
    env!("CARGO_BIN_EXE_actinglab")
}

fn write_lab2_resource_root() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("recognition")).unwrap();
    fs::create_dir(temp.path().join("navigation")).unwrap();
    fs::write(
        temp.path().join("recognition").join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
            ]
        }"#,
    )
    .unwrap();
    fs::write(
        temp.path()
            .join("recognition")
            .join("arknights.cn.pages.json"),
        r#"{
            "schema_version":"0.3",
            "pages":[{"id":"arknights/home","required":["home_anchor"]}]
        }"#,
    )
    .unwrap();
    fs::write(
        temp.path()
            .join("navigation")
            .join("arknights.cn.navigation.json"),
        r#"{
            "schema_version":"0.3",
            "game":"arknights",
            "server":"cn",
            "navigation":[],
            "destructive_actions":[]
        }"#,
    )
    .unwrap();
    temp
}

fn run_actinglab(args: &[&str], local_app_data: Option<&Path>) -> Output {
    let mut command = Command::new(actinglab_binary());
    command.args(args);
    if let Some(path) = local_app_data {
        command.env("LOCALAPPDATA", path);
    }
    command
        .output()
        .expect("actinglab child process should run")
}

fn run_actinglab_owned(args: &[String], local_app_data: Option<&Path>) -> Output {
    let mut command = Command::new(actinglab_binary());
    command.args(args);
    if let Some(path) = local_app_data {
        command.env("LOCALAPPDATA", path);
    }
    command
        .output()
        .expect("actinglab child process should run")
}

fn spawn_actinglab(args: &[String], local_app_data: Option<&Path>) -> Child {
    let mut command = Command::new(actinglab_binary());
    command.args(args);
    if let Some(path) = local_app_data {
        command.env("LOCALAPPDATA", path);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command
        .spawn()
        .expect("actinglab child process should spawn")
}

fn parse_stdout_json(output: &Output) -> Value {
    serde_json::from_slice::<Value>(&output.stdout).expect("stdout should parse as JSON")
}

fn response_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn error_req_id(value: &Value) -> &str {
    value
        .pointer("/error/details/req_id")
        .or_else(|| value.pointer("/error/details/details/req_id"))
        .and_then(Value::as_str)
        .expect("error req_id")
}

fn assert_receipt_has_dispatch_and_receipt(run_root: &Path, req_id: &str, local_app_data: &Path) {
    let receipt = run_actinglab(
        &[
            "--json",
            "--run-root",
            run_root.to_str().unwrap(),
            "lab",
            "receipt",
            "--req",
            req_id,
        ],
        Some(local_app_data),
    );
    assert!(
        receipt.status.success(),
        "{}",
        String::from_utf8_lossy(&receipt.stdout)
    );
    let receipt_json = parse_stdout_json(&receipt);
    let records = response_data(&receipt_json)
        .get("records")
        .and_then(Value::as_array)
        .expect("records");
    assert!(
        records
            .iter()
            .any(|record| { record.get("kind").and_then(Value::as_str) == Some("dispatch") })
    );
    assert!(
        records
            .iter()
            .any(|record| { record.get("kind").and_then(Value::as_str) == Some("receipt") })
    );
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

#[test]
fn lab2_child_process_stdout_starts_with_json_object() {
    let temp = write_lab2_resource_root();
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let output = run_actinglab(
        &[
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
        None,
    );

    assert!(output.status.success());
    assert_eq!(output.stdout.first(), Some(&b'{'));
    assert!(output.stderr.is_empty());
    let parsed = parse_stdout_json(&output);
    assert_eq!(parsed.get("ok").and_then(Value::as_bool), Some(true));
}

#[test]
fn lab2_arbitrator_defaults_to_persistent_local_app_data_path() {
    let local = TempDir::new().unwrap();
    let acquire = run_actinglab(
        &[
            "--json",
            "--game",
            "ark",
            "lab",
            "arbitrator",
            "acquire",
            "--instance",
            "ak",
            "--verb",
            "do",
        ],
        Some(local.path()),
    );
    assert!(acquire.status.success());
    let acquire_json = parse_stdout_json(&acquire);
    assert_eq!(acquire_json.get("ok").and_then(Value::as_bool), Some(true));

    let status = run_actinglab(
        &[
            "--json",
            "--game",
            "ark",
            "lab",
            "arbitrator",
            "status",
            "--instance",
            "ak",
        ],
        Some(local.path()),
    );
    assert!(status.status.success());
    let status_json = parse_stdout_json(&status);
    let data = response_data(&status_json);
    assert_eq!(
        data.pointer("/arbitration/holder/instance")
            .and_then(Value::as_str),
        Some("ak")
    );
    let state_file = data
        .get("state_file")
        .and_then(Value::as_str)
        .expect("state file");
    assert!(state_file.ends_with("lab2-arbitrator-state.json"));
    assert!(Path::new(state_file).is_file());
}

#[test]
fn lab2_bare_do_uses_short_lease_without_self_locking_next_call() {
    let local = TempDir::new().unwrap();
    let temp = write_lab2_resource_root();
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let args = [
        "--json",
        "--dry-run",
        "--resource-root",
        temp.path().to_str().unwrap(),
        "--game",
        "ark",
        "--instance",
        "ak",
        "do",
        "home_button",
        "--scene",
        scene.to_str().unwrap(),
    ];

    let first = run_actinglab(&args, Some(local.path()));
    let second = run_actinglab(&args, Some(local.path()));

    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stdout)
    );
    let status = run_actinglab(
        &[
            "--json",
            "--game",
            "ark",
            "lab",
            "arbitrator",
            "status",
            "--instance",
            "ak",
        ],
        Some(local.path()),
    );
    let status_json = parse_stdout_json(&status);
    assert!(
        response_data(&status_json)
            .pointer("/arbitration/holder")
            .is_none()
    );
}

#[test]
fn lab2_explicit_arbitrator_lease_can_be_reused_and_blocks_third_party() {
    let local = TempDir::new().unwrap();
    let temp = write_lab2_resource_root();
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let acquire = run_actinglab(
        &[
            "--json",
            "--game",
            "ark",
            "lab",
            "arbitrator",
            "acquire",
            "--instance",
            "ak",
            "--verb",
            "do",
        ],
        Some(local.path()),
    );
    assert!(acquire.status.success());
    let acquire_json = parse_stdout_json(&acquire);
    let lease_id = response_data(&acquire_json)
        .pointer("/arbitration/details/lease/lease_id")
        .and_then(Value::as_str)
        .expect("lease id")
        .to_string();
    let do_args = [
        "--json",
        "--dry-run",
        "--resource-root",
        temp.path().to_str().unwrap(),
        "--game",
        "ark",
        "--instance",
        "ak",
        "do",
        "home_button",
        "--scene",
        scene.to_str().unwrap(),
        "--lease-id",
        lease_id.as_str(),
    ];

    let first = run_actinglab(&do_args, Some(local.path()));
    let second = run_actinglab(&do_args, Some(local.path()));
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stdout)
    );

    let blocked = run_actinglab(
        &[
            "--json",
            "--dry-run",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "--instance",
            "ak",
            "do",
            "home_button",
            "--scene",
            scene.to_str().unwrap(),
        ],
        Some(local.path()),
    );
    assert!(!blocked.status.success());
    let blocked_json = parse_stdout_json(&blocked);
    assert_eq!(blocked_json.get("ok").and_then(Value::as_bool), Some(false));
    assert!(String::from_utf8_lossy(&blocked.stdout).contains("lease_held"));
}

#[test]
fn lab2_concurrent_bare_do_blocks_same_instance_writer_during_short_lease() {
    let local = TempDir::new().unwrap();
    let temp = write_lab2_resource_root();
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let args = vec![
        "--json".to_string(),
        "--dry-run".to_string(),
        "--resource-root".to_string(),
        temp.path().display().to_string(),
        "--game".to_string(),
        "ark".to_string(),
        "--instance".to_string(),
        "ak".to_string(),
        "do".to_string(),
        "home_button".to_string(),
        "--scene".to_string(),
        scene.display().to_string(),
        "--test-capture-delay-ms".to_string(),
        "700".to_string(),
    ];

    let first = spawn_actinglab(&args, Some(local.path()));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let second = run_actinglab_owned(&args, Some(local.path()));
    let first = first.wait_with_output().expect("first output");
    let first_ok = first.status.success();
    let second_ok = second.status.success();

    assert_ne!(first_ok, second_ok);
    let blocked = if first_ok { &second } else { &first };
    assert!(String::from_utf8_lossy(&blocked.stdout).contains("lease_held"));
}

#[test]
fn lab2_post_admit_failures_write_receipts_for_all_cli_verbs() {
    let local = TempDir::new().unwrap();
    let temp = write_lab2_resource_root();
    let run_root = temp.path().join("run");
    let scene = temp.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).unwrap();
    let missing_root = temp.path().join("missing-resource-root");

    let cases: Vec<Vec<String>> = vec![
        vec![
            "--json".into(),
            "--run-root".into(),
            run_root.display().to_string(),
            "--resource-root".into(),
            missing_root.display().to_string(),
            "--game".into(),
            "ark".into(),
            "observe".into(),
            "--scene".into(),
            scene.display().to_string(),
        ],
        vec![
            "--json".into(),
            "--run-root".into(),
            run_root.display().to_string(),
            "--resource-root".into(),
            temp.path().display().to_string(),
            "--game".into(),
            "ark".into(),
            "wait".into(),
        ],
        vec![
            "--json".into(),
            "--run-root".into(),
            run_root.display().to_string(),
            "--resource-root".into(),
            temp.path().display().to_string(),
            "--game".into(),
            "ark".into(),
            "--instance".into(),
            "ak".into(),
            "do".into(),
            "missing_target".into(),
            "--dry-run".into(),
            "--scene".into(),
            scene.display().to_string(),
        ],
        vec![
            "--json".into(),
            "--run-root".into(),
            run_root.display().to_string(),
            "--resource-root".into(),
            temp.path().display().to_string(),
            "--game".into(),
            "ark".into(),
            "--instance".into(),
            "ak".into(),
            "ensure".into(),
            "arknights/archive".into(),
            "--dry-run".into(),
            "--scene".into(),
            scene.display().to_string(),
        ],
    ];

    for args in cases {
        let output = run_actinglab_owned(&args, Some(local.path()));
        assert!(
            !output.status.success(),
            "case unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value = parse_stdout_json(&output);
        let req_id = error_req_id(&value);
        assert_receipt_has_dispatch_and_receipt(&run_root, req_id, local.path());
    }
}
