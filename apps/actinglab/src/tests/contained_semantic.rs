// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn dangerous_semantic_contract_preserves_legacy_rejections_without_expansion() {
    for value in ["gacha_route", "ranked_pvp_entry"] {
        let error = reject_dangerous_semantic_id("fixture", value).unwrap_err();
        assert_eq!(error.class, ErrorKind::SafetyBlocked);
        assert_eq!(error.code, "semantic_action_requires_destructive_opt_in");
    }
    for value in ["random_draw_route", "competitive_entry"] {
        reject_dangerous_semantic_id("fixture", value).unwrap();
    }
    let payload = session_self_heal_policy_payload(
        &GlobalOptions::default(),
        &FlagArgs::default(),
        "session self-heal-policy",
    )
    .unwrap();
    assert_eq!(
        payload.pointer("/maintenance_boundary/pvp_or_exercise_allowed"),
        Some(&Value::Bool(false))
    );
    assert!(
        payload
            .pointer("/maintenance_boundary/competitive_or_exercise_allowed")
            .is_none()
    );
}

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
    let pack = recognition.join("sample.local.pack.json");
    let pages = recognition.join("sample.local.pages.json");
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
    let temp = seal_semantic_fixture(temp, "sample", "local", &pack, &pages, None);
    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            temp.path().to_str().unwrap(),
            "--game",
            "sample",
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
    let pack = recognition.join("sample.local.pack.json");
    let pages = recognition.join("sample.local.pages.json");
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
    let temp = seal_semantic_fixture(temp, "sample", "local", &pack, &pages, None);
    fs::write(&pack, b"not-json").unwrap();

    let result = run_semantic_cli(
        &temp,
        [
            "--json",
            "--resource-root",
            repo.to_str().unwrap(),
            "--game",
            "sample",
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
