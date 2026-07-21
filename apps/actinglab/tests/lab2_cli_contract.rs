// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

struct Lab2Fixture {
    temp: TempDir,
    package: PathBuf,
    expected_sha256: String,
}

impl Lab2Fixture {
    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn command(&self, args: &[&str], local_app_data: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_actinglab"));
        command.args(args).args([
            "--zip",
            self.package.to_str().expect("package path"),
            "--expected-sha256",
            &self.expected_sha256,
        ]);
        configure_command(&mut command, local_app_data);
        command.output().expect("actinglab child process")
    }
}

fn configure_command(command: &mut Command, local_app_data: &Path) {
    command
        .env("LOCALAPPDATA", local_app_data)
        .env("APPDATA", local_app_data)
        .env_remove("ACTINGLAB_SESSION_STATE_DIR")
        .env_remove("ACTINGLAB_CONFIG_PATH")
        .env_remove("ACTINGCOMMAND_RUNTIME_STATE_ROOT");
}

fn write_fixture() -> Lab2Fixture {
    let temp = TempDir::new().expect("tempdir");
    let recognition = temp.path().join("recognition");
    let navigation = temp.path().join("navigation");
    fs::create_dir_all(&recognition).expect("recognition dir");
    fs::create_dir_all(&navigation).expect("navigation dir");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":0,"y":0,"width":1,"height":1}}
            ]
        }"#,
    )
    .expect("recognition pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        r#"{"schema_version":"0.3","pages":[{"id":"arknights/home","required":["home_anchor"]}]}"#,
    )
    .expect("page set");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        r#"{"schema_version":"0.3","game":"arknights","server":"cn","navigation":[],"destructive_actions":[]}"#,
    )
    .expect("navigation graph");
    let navigation_path = fs::read_dir(&navigation)
        .expect("navigation directory")
        .next()
        .expect("navigation entry")
        .expect("navigation directory entry")
        .path();
    let navigation_value: Value =
        serde_json::from_slice(&fs::read(navigation_path).expect("navigation resource"))
            .expect("navigation JSON");
    let game = navigation_value["game"].as_str().expect("game");
    let server = navigation_value["server"].as_str().expect("server");
    let control = serde_json::to_vec(&json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "lab2.cli.contract",
        "execution_mode": "recognize_only",
        "game": game,
        "server": server,
        "resolution": {"width": 1, "height": 1},
        "entry_task_id": "task",
        "allow_placeholder_coords": true
    }))
    .expect("control JSON");
    let operation = serde_json::to_vec(&json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": game,
        "server_scope": [server],
        "goal": "Lab2 CLI contract fixture",
        "coordinate_space": {"width": 1, "height": 1},
        "operations": [{
            "id": "direct_home_button",
            "purpose": "Lab2 typed target closure",
            "from": format!("{game}/home"),
            "to": Value::Null,
            "click": {"kind":"rect","x":0,"y":0,"width":1,"height":1},
            "guard": {
                "page_id": format!("{game}/home"),
                "target_id": "home_button",
                "expected_rect": {"x":0,"y":0,"width":1,"height":1},
                "color_probe": "home_button"
            }
        }]
    }))
    .expect("operation JSON");

    let package = temp.path().join("semantic.zip");
    let file = File::create(&package).expect("package file");
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in [
        ("control.json", control.as_slice()),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#.as_slice(),
        ),
        ("resources/operations/task/task.json", operation.as_slice()),
    ] {
        zip.start_file(name, options).expect("package entry");
        zip.write_all(bytes).expect("package bytes");
    }
    for (source, destination) in [
        (
            recognition.join("arknights.cn.pack.json"),
            "resources/recognition/arknights.cn.pack.json",
        ),
        (
            recognition.join("arknights.cn.pages.json"),
            "resources/recognition/arknights.cn.pages.json",
        ),
        (
            navigation.join("arknights.cn.navigation.json"),
            "resources/navigation/arknights.cn.navigation.json",
        ),
    ] {
        zip.start_file(destination, options)
            .expect("resource entry");
        zip.write_all(&fs::read(source).expect("resource bytes"))
            .expect("resource package bytes");
    }
    zip.finish().expect("finish package");
    let expected_sha256 = format!(
        "{:x}",
        Sha256::digest(fs::read(&package).expect("package bytes"))
    );
    Lab2Fixture {
        temp,
        package,
        expected_sha256,
    }
}

fn parse_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout JSON")
}

#[test]
fn retired_arbitrator_fails_without_creating_local_state() {
    let local = TempDir::new().expect("local app data");
    let mut command = Command::new(env!("CARGO_BIN_EXE_actinglab"));
    command.args(["--json", "lab", "arbitrator", "status"]);
    configure_command(&mut command, local.path());
    let output = command.output().expect("actinglab child process");
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(
        parse_json(&output)["error"]["code"],
        "legacy_lab2_arbitrator_retired"
    );
    assert!(!local.path().join("ActingCommand/actinglab/lab2").exists());
}

#[test]
fn offline_scene_do_is_pure_repeatable_and_state_free() {
    let fixture = write_fixture();
    let local = TempDir::new().expect("local app data");
    let scene = fixture.path().join("home.png");
    fs::write(&scene, encode_png(1, 1, [255, 0, 0])).expect("scene");
    let args = [
        "--json",
        "--dry-run",
        "--instance",
        "node.a",
        "do",
        "home_button",
        "--scene",
        scene.to_str().expect("scene path"),
    ];
    for _ in 0..2 {
        let output = fixture.command(&args, local.path());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let response = parse_json(&output);
        assert_eq!(response["data"]["executed"], false);
        assert_eq!(
            response["data"]["arbitration"]["authority"],
            "isolated_offline"
        );
        assert_eq!(response["data"]["device"]["mode"], "isolated_offline");
    }
    assert!(!local.path().join("ActingCommand/actinglab/lab2").exists());
}

#[test]
fn offline_mode_requires_an_explicit_scene_and_never_falls_back_to_device_io() {
    let fixture = write_fixture();
    let local = TempDir::new().expect("local app data");
    let output = fixture.command(&["--json", "--dry-run", "do", "home_button"], local.path());
    assert_eq!(output.status.code(), Some(2));
    let response = parse_json(&output);
    assert_eq!(response["error"]["code"], "validation_failed");
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("require --scene")
    );
    assert!(!local.path().join("ActingCommand/actinglab/lab2").exists());
}

#[test]
fn child_process_stdout_remains_one_json_envelope() {
    let local = TempDir::new().expect("local app data");
    let mut command = Command::new(env!("CARGO_BIN_EXE_actinglab"));
    command.args(["--json", "lab", "arbitrator", "status"]);
    configure_command(&mut command, local.path());
    let output = command.output().expect("actinglab child process");
    assert_eq!(output.stdout.first(), Some(&b'{'));
    let _: Value = parse_json(&output);
}

fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut row = Vec::with_capacity((width * 3 + 1) as usize);
    row.push(0);
    for _ in 0..width {
        row.extend_from_slice(&color);
    }
    let mut scanlines = Vec::with_capacity((width * height * 3 + height) as usize);
    for _ in 0..height {
        scanlines.extend_from_slice(&row);
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
        out.push(u8::from(index == data.len().div_ceil(65_535) - 1));
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
