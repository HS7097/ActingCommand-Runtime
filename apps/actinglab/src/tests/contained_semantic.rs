// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn detect_page_returns_standby_when_no_page_matches() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let pack = temp.path().join("pack.json");
    let pages = temp.path().join("pages.json");
    let scene = temp.path().join("scene.png");
    fs::write(
        &pack,
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[{"type":"color","id":"home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]
        }"#,
    )
    .unwrap();
    fs::write(
        &pages,
        r#"{"schema_version":"0.3","pages":[{"id":"home","required":["home"]}]}"#,
    )
    .unwrap();
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
    let temp = seal_semantic_fixture(temp, "fixture", "test", &pack, &pages, None);
    let result = run_semantic_cli(
        &temp,
        ["--json", "detect-page", "--scene", scene.to_str().unwrap()],
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
        Some("standby")
    );
}

#[test]
fn detect_page_uses_verified_bundle_when_loose_root_is_also_present() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let recognition = temp.path().join("recognition");
    fs::create_dir(&recognition).unwrap();
    let pack = recognition.join("arknights.cn.pack.json");
    let pages = recognition.join("arknights.cn.pages.json");
    let scene = temp.path().join("scene.png");
    fs::write(
        &pack,
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[{"type":"color","id":"home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]
        }"#,
    )
    .unwrap();
    fs::write(
        &pages,
        r#"{"schema_version":"0.3","pages":[{"id":"home","required":["home"]}]}"#,
    )
    .unwrap();
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
    let temp = seal_semantic_fixture(temp, "arknights", "cn", &pack, &pages, None);
    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "ark",
            "detect-page",
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
        Some("standby")
    );
}

#[test]
fn detect_page_ignores_reorganized_loose_root_after_bundle_admission() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    let ours = repo.join("ours");
    let recognition = ours.join("recognition");
    let operations = ours.join("operations");
    fs::create_dir_all(&recognition).unwrap();
    fs::create_dir_all(&operations).unwrap();
    let pack = recognition.join("arknights.cn.pack.json");
    let pages = recognition.join("arknights.cn.pages.json");
    let scene = temp.path().join("scene.png");
    fs::write(
        &pack,
        r#"{
            "schema_version":"0.3",
            "coordinate_space":{"width":1,"height":1},
            "targets":[{"type":"color","id":"home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}]
        }"#,
    )
    .unwrap();
    fs::write(
        &pages,
        r#"{"schema_version":"0.3","pages":[{"id":"home","required":["home"]}]}"#,
    )
    .unwrap();
    fs::write(&scene, encode_png(1, 1, [0, 0, 255])).unwrap();
    let temp = seal_semantic_fixture(temp, "arknights", "cn", &pack, &pages, None);
    fs::write(&pack, b"not-json").unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            repo.to_str().unwrap(),
            "--game",
            "ark",
            "detect-page",
            "--scene",
            scene.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 0, "{:?}", result.envelope.error);
    assert_eq!(
        result
            .envelope
            .data
            .as_ref()
            .unwrap()
            .get("page")
            .and_then(Value::as_str),
        Some("home")
    );
}

#[test]
fn semantic_command_rejects_loose_only_resources_before_capture() {
    let temp = semantic_resource_root(false);
    let result = run_cli(
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "observe",
            "--capture",
            "--state-dir",
            temp.path().join("state").to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().unwrap();
    assert_eq!(error.code, "package_invalid");
    assert!(
        error
            .message
            .contains("loose resource roots are not executable")
    );
}

#[test]
fn semantic_command_rejects_self_computed_hash_source_before_capture() {
    let temp = semantic_resource_root(false);
    let result = run_cli(
        [
            "--json",
            "observe",
            "--capture",
            "--state-dir",
            temp.path().join("state").to_str().unwrap(),
            "--zip",
            temp.zip.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().unwrap();
    assert_eq!(error.code, "package_invalid");
    assert!(error.message.contains("externally supplied"));
}

#[test]
fn semantic_command_rejects_external_hash_mismatch_before_scene_load() {
    let temp = semantic_resource_root(false);
    let result = run_cli(
        [
            "--json",
            "--dry-run",
            "do",
            "home_button",
            "--scene",
            temp.path().join("missing.png").to_str().unwrap(),
            "--state-dir",
            temp.path().join("state").to_str().unwrap(),
            "--zip",
            temp.zip.to_str().unwrap(),
            "--expected-sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().unwrap();
    assert_eq!(error.code, "package_invalid");
    assert!(error.message.contains("hash mismatch"));
}
