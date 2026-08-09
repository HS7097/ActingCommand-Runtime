// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

const RED_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

#[test]
fn official_readonly_page_commands_report_every_matching_page() {
    let fixture = Fixture::new();

    for command in ["current-page", "detect-page"] {
        let output = fixture.run(command);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{command} must fail closed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
        assert_eq!(envelope["ok"], false);
        assert!(envelope.get("data").is_none() || envelope["data"].is_null());
        assert_eq!(envelope["error"]["code"], "page_recognition_conflict");
        let matched_pages = if command == "detect-page" {
            &envelope["error"]["details"]["details"]["matched_pages"]
        } else {
            &envelope["error"]["details"]["matched_pages"]
        };
        assert_eq!(
            matched_pages,
            &json!(["fixture/home", "fixture/also_home"]),
            "{command} envelope: {envelope:#}"
        );
    }
}

struct Fixture {
    _temp: TempDir,
    root: std::path::PathBuf,
    resource_root: std::path::PathBuf,
    run_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    scene: std::path::PathBuf,
    package: std::path::PathBuf,
    package_sha256: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().to_path_buf();
        let resource_root = root.join("resources");
        let run_root = root.join("runs");
        let state_root = root.join("state");
        let scene = root.join("red.png");
        let package = root.join("semantic.zip");

        fs::create_dir_all(&run_root).expect("run root");
        fs::create_dir_all(&state_root).expect("state root");
        fs::write(&scene, RED_PNG).expect("scene");
        write_resources(&resource_root);
        write_package(&package, &resource_root);
        let package_sha256 = format!(
            "{:x}",
            Sha256::digest(fs::read(&package).expect("package bytes"))
        );

        Self {
            _temp: temp,
            root,
            resource_root,
            run_root,
            state_root,
            scene,
            package,
            package_sha256,
        }
    }

    fn run(&self, command: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_actinglab"))
            .args([
                "--json",
                "--resource-root",
                self.resource_root.to_str().expect("resource root UTF-8"),
                "--run-root",
                self.run_root.to_str().expect("run root UTF-8"),
                "--game",
                "arknights",
                "--server",
                "cn",
                command,
                "--scene",
                self.scene.to_str().expect("scene UTF-8"),
                "--zip",
                self.package.to_str().expect("package UTF-8"),
                "--expected-sha256",
                &self.package_sha256,
            ])
            .current_dir(&self.root)
            .env(
                "ACTINGLAB_CONFIG_PATH",
                self.root.join("missing-config.json"),
            )
            .env("LOCALAPPDATA", &self.state_root)
            .env("APPDATA", &self.state_root)
            .env(
                "ACTINGLAB_SESSION_STATE_DIR",
                self.state_root.join("session"),
            )
            .env(
                "ACTINGCOMMAND_RUNTIME_STATE_ROOT",
                self.state_root.join("runtime"),
            )
            .env_remove("ACTINGLAB_REQUIRE_SESSION_DAEMON")
            .output()
            .expect("run actinglab")
    }
}

fn write_resources(root: &Path) {
    let recognition = root.join("recognition");
    let navigation = root.join("navigation");
    fs::create_dir_all(&recognition).expect("recognition root");
    fs::create_dir_all(&navigation).expect("navigation root");
    fs::write(
        recognition.join("arknights.cn.pack.json"),
        br#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[
                {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"also_home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}
            ]
        }"#,
    )
    .expect("pack");
    fs::write(
        recognition.join("arknights.cn.pages.json"),
        br#"{
            "schema_version":"0.3",
            "pages":[
                {"id":"fixture/home","required":["home_anchor"]},
                {"id":"fixture/also_home","required":["also_home_anchor"]}
            ]
        }"#,
    )
    .expect("pages");
    fs::write(
        navigation.join("arknights.cn.navigation.json"),
        br#"{
            "schema_version":"0.3",
            "game":"arknights",
            "server":"cn",
            "navigation":[],
            "destructive_actions":[]
        }"#,
    )
    .expect("navigation");
}

fn write_package(path: &Path, root: &Path) {
    let pack = fs::read(root.join("recognition/arknights.cn.pack.json")).expect("pack");
    let pages = fs::read(root.join("recognition/arknights.cn.pages.json")).expect("pages");
    let navigation =
        fs::read(root.join("navigation/arknights.cn.navigation.json")).expect("navigation");
    write_zip(
        path,
        &[
            (
                "control.json",
                br#"{"game":"arknights","server":"cn","entry_task_id":"task"}"#,
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
            ),
            ("resources/operations/task/task.json", br#"{}"#),
            ("resources/recognition/arknights.cn.pack.json", &pack),
            ("resources/recognition/arknights.cn.pages.json", &pages),
            (
                "resources/navigation/arknights.cn.navigation.json",
                &navigation,
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
