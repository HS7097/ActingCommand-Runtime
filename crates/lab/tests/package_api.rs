// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_lab::{
    PackageBuildTaskRequest, PackageBuildTaskResponse, PackageEnvOptions, PackageResolution,
    PackageSource, PackageValidateRequest, PackageValidationResponse, ResourceConvertRequest,
    ResourceConvertResponse,
};
use serde::Serialize;
use std::path::PathBuf;

fn assert_serializable<T: Serialize>() {}

#[test]
fn package_family_exposes_typed_requests_and_responses() {
    let _validate = PackageValidateRequest {
        zip_path: PathBuf::from("bundle.zip"),
        include_entries: false,
        expected_input_sha256: None,
    };
    let _build = PackageBuildTaskRequest {
        source: PackageSource::Local(PathBuf::from("resources")),
        temporary_root: PathBuf::from("target/tmp"),
        task_id: "task".to_string(),
        game: Some("arknights".to_string()),
        server: Some("cn".to_string()),
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: Some(PackageResolution {
            width: 1280,
            height: 720,
        }),
        include_recovery: false,
        out: PathBuf::from("task.zip"),
        dry_run: true,
        max_buffered_payload_bytes: actingcommand_lab::DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        env: PackageEnvOptions::default(),
    };
    let _convert = ResourceConvertRequest {
        repo: PathBuf::from("resources"),
        game: Some("arknights".to_string()),
        server: Some("cn".to_string()),
        locale: Some("zh-CN".to_string()),
        maa_tasks_root: None,
        dry_run: true,
    };

    assert_serializable::<PackageValidationResponse>();
    assert_serializable::<PackageBuildTaskResponse>();
    assert_serializable::<ResourceConvertResponse>();
}

// Specification: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5554462313
#[test]
fn explicit_postcondition_budget_build_and_runtime_admission_agree() {
    use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedContainedTask};
    use actingcommand_pack_containment::Sha256Hash;
    use actingcommand_resource_tooling::{
        AuthoringEnvironmentSnapshot, DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, open_published_package,
        prepare_package_build_task, resource_convert,
    };
    use serde_json::json;
    use std::fs;
    use std::io::{Cursor, Read, Write};
    use tempfile::TempDir;
    use zip::{ZipArchive, ZipWriter, write::FileOptions};

    let temp = TempDir::new().unwrap();
    let root = temp.path().join("resources");
    const GAME: &str = "fourth-game";
    const SERVER: &str = "test-shard";
    const LOCALE: &str = "x-fixture";
    let png: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9,
        251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fs::create_dir_all(root.join("operations/return_home/assets")).expect("operation assets");
    fs::create_dir_all(root.join("navigation")).expect("navigation directory");
    fs::write(
        root.join("operations/resources.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "1.0",
            "resources": [],
            "resource_count": 0
        }))
        .expect("resources json"),
    )
    .expect("write resources");
    fs::write(root.join("operations/return_home/assets/HOME.png"), png).expect("write template");
    fs::write(
        root.join("operations/return_home/task.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.3",
            "task_id": "return_home",
            "game": GAME,
            "server_scope": [SERVER],
            "locale": LOCALE,
            "goal": "external neutral fixture",
            "coordinate_space": {"width": 1280, "height": 720},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
            "anchors": [{
                "id": "home",
                "template": "assets/HOME.png",
                "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 30, "height": 30}},
                "threshold": 0.8,
                "color_check": null
            }],
            "entry_page": "home",
            "target_page": "home",
            "operations": [{
                "id": "home_noop",
                "purpose": "neutral fixture",
                "from": "home",
                "to": null,
                "click": {"kind": "point", "x": 1, "y": 1},
                "verify_template": null,
                "guard": {
                    "page_id": "home",
                    "target_id": "page/home",
                    "expected_rect": {"x": 1, "y": 1, "width": 1, "height": 1},
                    "verify_template": "assets/HOME.png"
                },
                "consumes": [],
                "produces": []
            }]
        }))
        .expect("task json"),
    )
    .expect("write task");
    let path = root.join("operations/return_home/task.json");
    let mut task: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    task["schema_version"] = json!("0.8");
    task["operations"][0]["expect_after"] = json!({"page_id":"home","interval_ms":5000});
    task["scheduling_outcome"] = json!({"mappings":[{"outcome_key":"fields_recorded",
        "effect":"no_designated_effect","terminal_pages":["home"]}]});
    task["ocr_targets"] = json!([{"id":"ocr/count","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},
        "languages":["en"],"timeout_ms":1000,"match_mode":"exact","expected":["unused"],
        "case_sensitive":false,"minimum_confidence":0.0,"model_ref":"PP-OCRv6_medium","model_sha256":"a".repeat(64)}]);
    task["post_admission_ocr"] = json!({"mode":"fields_v1","page_ids":["home"],
        "fields":[{"id":"count","group":"snapshot","target_id":"ocr/count","required":true,
            "privacy":"public","trim":"whitespace_v1","value":{"type":"unsigned_integer","min":0,"max":u64::MAX}}],
        "limits":{"max_frames":1,"max_items":1,"max_string_bytes":64,"max_total_bytes":4096,"max_truth_entries":1},
        "outcome_key":"fields_recorded"});
    let build = |value: &serde_json::Value, name: &str| {
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        resource_convert(ResourceConvertRequest {
            repo: root.clone(),
            game: None,
            server: None,
            locale: None,
            maa_tasks_root: None,
            dry_run: false,
        })?;
        prepare_package_build_task(PackageBuildTaskRequest {
            source: PackageSource::Local(root.clone()),
            temporary_root: temp.path().join("source"),
            task_id: "return_home".into(),
            game: None,
            server: None,
            locale: None,
            package_id: None,
            execution_mode: Some("navigable_route".into()),
            resolution: None,
            include_recovery: false,
            out: temp.path().join(format!("{name}.zip")),
            dry_run: false,
            max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
            env: PackageEnvOptions::default(),
        })?
        .build(&AuthoringEnvironmentSnapshot::default())
    };
    let admit = |bytes: &[u8]| {
        PreparedContainedTask::load(
            "neutral-wait",
            bytes,
            ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(bytes).to_string()).unwrap(),
        )
    };
    let mut package = Vec::new();
    for timeout in [None, Some(480_000), Some(600_000)] {
        let name = format!("wait-{timeout:?}");
        if let Some(timeout) = timeout {
            task["operations"][0]["expect_after"]["timeout_ms"] = json!(timeout);
        }
        build(&task, &name).expect("official bounded build");
        package = open_published_package(&temp.path().join(format!("{name}.zip")))
            .unwrap()
            .read_all()
            .unwrap();
        admit(&package).expect("exact built bytes pass Runtime admission");
    }
    for timeout in [0, 600_001, u64::MAX] {
        task["operations"][0]["expect_after"]["timeout_ms"] = json!(timeout);
        let error = build(&task, &format!("invalid-{timeout}")).unwrap_err();
        assert!(
            error
                .message
                .contains("expect_after.timeout_ms must be in 1..=600000"),
            "{error:?}"
        );
    }

    // Mutate the admitted package in memory and hash each exact candidate before admission.
    let mut archive = ZipArchive::new(Cursor::new(package)).unwrap();
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        entries.push((entry.name().to_owned(), bytes));
    }
    assert_eq!(
        entries
            .iter()
            .filter(|(name, _)| name.ends_with("/return_home/task.json"))
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|(name, _)| name == "control.json")
            .count(),
        1
    );
    for (field, value, accepted) in [
        ("expect", 480_000, true),
        ("expect", 600_000, true),
        ("expect", 0, false),
        ("expect", 600_001, false),
        ("step", 60_000, true),
        ("step", 60_001, false),
        ("step", 0, false),
        ("delay", 5_000, true),
        ("delay", 5_001, false),
        ("delay", 0, false),
        ("interval", 5_000, true),
        ("interval", 5_001, false),
        ("interval", 0, false),
    ] {
        let mut candidate_entries = entries.clone();
        for (name, bytes) in &mut candidate_entries {
            if name == "control.json" && field == "step" {
                let mut control: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                control["step_timeout_ms"] = json!(value);
                *bytes = serde_json::to_vec(&control).unwrap();
            } else if name.ends_with("/return_home/task.json") && field != "step" {
                let mut task: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                let operation = &mut task["operations"][0];
                match field {
                    "expect" => operation["expect_after"]["timeout_ms"] = json!(value),
                    "delay" => operation["post_delay_ms"] = json!(value),
                    _ => operation["expect_after"]["interval_ms"] = json!(value),
                }
                *bytes = serde_json::to_vec(&task).unwrap();
            }
        }
        let manifest_index = candidate_entries
            .iter()
            .position(|(name, _)| name == "resources/manifest.json")
            .unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&candidate_entries[manifest_index].1).unwrap();
        for file in manifest["files"].as_array_mut().unwrap() {
            let path = format!("resources/{}", file["path"].as_str().unwrap());
            let (_, bytes) = candidate_entries
                .iter()
                .find(|(name, _)| name == &path)
                .unwrap();
            file["sha256"] = json!(format!("sha256:{}", Sha256Hash::digest(bytes)));
        }
        candidate_entries[manifest_index].1 = serde_json::to_vec(&manifest).unwrap();
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in &candidate_entries {
            writer.start_file(name, FileOptions::default()).unwrap();
            writer.write_all(bytes).unwrap();
        }
        let candidate = writer.finish().unwrap().into_inner();
        let result = admit(&candidate);
        assert_eq!(
            result.is_ok(),
            accepted,
            "{field}={value}: {:?}",
            result.as_ref().err()
        );
        if !accepted {
            let code = if field == "delay" {
                "contained_task_operation_invalid"
            } else {
                "contained_task_control_invalid"
            };
            assert_eq!(result.err().expect("rejected bound").code(), code);
        }
    }
}
