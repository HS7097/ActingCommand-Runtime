// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_policy::{CatalogDocumentSource, CatalogSources, compile_catalog};
use actingcommand_resource_tooling::{
    AuthoringEnvironmentSnapshot, DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, PackageBuildTaskRequest,
    PackageEnvOptions, PackageSource, ResourceConvertRequest, open_published_package,
    prepare_package_build_task, resource_convert,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "fourth-game";
const SERVER: &str = "test-shard";
const LOCALE: &str = "x-fixture";

#[test]
fn external_neutral_game_metadata_converts_schedules_and_packages() {
    let temp = TempDir::new().expect("temp dir");
    let resource_root = temp.path().join("external-resources");
    write_external_resource_fixture(&resource_root);

    let converted = resource_convert(ResourceConvertRequest {
        repo: resource_root.clone(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })
    .expect("convert external neutral-game metadata");
    assert_eq!(converted.game, GAME);
    assert_eq!(converted.server, SERVER);
    assert_eq!(converted.locale, LOCALE);

    let catalog = compile_catalog(&neutral_catalog_sources())
        .expect("compile scheduling catalog for external neutral game");
    assert!(catalog.summary().counts.tasks > 0);

    let out = temp.path().join("neutral-game.zip");
    let prepared = prepare_package_build_task(PackageBuildTaskRequest {
        source: PackageSource::Local(resource_root),
        temporary_root: temp.path().join("remote-source"),
        task_id: "return_home".to_string(),
        game: None,
        server: None,
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: None,
        include_recovery: false,
        out: out.clone(),
        dry_run: false,
        max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        env: PackageEnvOptions::default(),
    })
    .expect("prepare neutral-game package from external metadata");
    assert_eq!(prepared.game(), GAME);
    assert_eq!(prepared.server(), SERVER);
    let package = prepared
        .build(&AuthoringEnvironmentSnapshot::default())
        .expect("build neutral-game package");
    assert_eq!(package.game, GAME);
    assert_eq!(package.server, SERVER);
    let published = open_published_package(&out).expect("open published neutral-game package");
    assert!(published.path().is_file());
    published.close().expect("close published package");
}

// Specification 1: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551203604
#[test]
fn fields_v1_neutral_declaration_and_package_closure() {
    use sha2::{Digest, Sha256};
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("resources");
    write_external_resource_fixture(&root);
    let path = root.join("operations/return_home/task.json");
    let mut task: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let dictionary = serde_json::to_vec(&json!({"schema_version":"actingcommand.ocr-truth-set.v2",
        "items":["TokenA"],"aliases":[{"observed":"short","canonical":"TokenA"}]}))
    .unwrap();
    let hash = format!("{:x}", Sha256::digest(&dictionary));
    fs::write(
        root.join("operations/return_home/dictionary.json"),
        &dictionary,
    )
    .unwrap();
    task["schema_version"] = json!("0.8");
    task["scheduling_outcome"] = json!({"mappings":[{"outcome_key":"fields_recorded",
        "effect":"no_designated_effect","terminal_pages":["home"]}]});
    task["ocr_targets"] = json!([{"id":"ocr/name","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}},
        "languages":["en"],"timeout_ms":1000,"match_mode":"exact","expected":["unused"],
        "case_sensitive":false,"minimum_confidence":0.0,"model_ref":"PP-OCRv6_medium","model_sha256":"a".repeat(64)}]);
    task["post_admission_ocr"] = json!({"mode":"fields_v1","page_ids":["home"],
        "fields":[{"id":"name","group":"item","target_id":"ocr/name","required":true,
            "privacy":"public","trim":"whitespace_v1","value":{"type":"dictionary_entry",
                "dictionary":{"path":"dictionary.json","sha256":hash}}}],
        "limits":{"max_frames":2,"max_items":8,"max_string_bytes":64,"max_total_bytes":4096,"max_truth_entries":8},
        "outcome_key":"fields_recorded"});
    let convert = |value: &serde_json::Value| {
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        resource_convert(ResourceConvertRequest {
            repo: root.clone(),
            game: None,
            server: None,
            locale: None,
            maa_tasks_root: None,
            dry_run: true,
        })
    };
    convert(&task).expect("0.8 source declaration");
    for case in 0..9 {
        let mut invalid = task.clone();
        match case {
            0 => invalid["schema_version"] = json!("0.7"),
            1 => invalid["post_admission_ocr"]["comparison"] = json!("exact_set_v1"),
            2 => invalid["post_admission_ocr"]["fields"][0]["target_id"] = json!("ocr/missing"),
            3 => invalid["post_admission_ocr"]["fields"][0]["group"] = json!(""),
            4 => {
                invalid["post_admission_ocr"]["fields"][0]["value"] =
                    json!({"type":"unsigned_integer","min":9,"max":2})
            }
            5 => {
                invalid["post_admission_ocr"]["fields"][0]["value"]["dictionary"]["sha256"] =
                    json!("0".repeat(64))
            }
            6 => {
                invalid["post_admission_ocr"]["fields"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("privacy");
            }
            7 => invalid["post_admission_ocr"]["fields"][0]["value"]["type"] = json!("unknown"),
            _ => invalid["post_admission_ocr"]["mode"] = json!("fields_v2"),
        }
        assert!(convert(&invalid).is_err(), "case {case}");
    }
    let mut legacy = task.clone();
    legacy["schema_version"] = json!("0.7");
    legacy["post_admission_ocr"] = json!({"page_id":"home","target_id":"ocr/name",
        "truth_set":{"path":"dictionary.json","sha256":hash},"normalization":"trim_lowercase_v1",
        "comparison":"exact_set_v1","limits":task["post_admission_ocr"]["limits"],"outcome_key":"fields_recorded"});
    convert(&legacy).expect("0.7 set remains accepted");
    fs::write(&path, serde_json::to_vec(&task).unwrap()).unwrap();
    resource_convert(ResourceConvertRequest {
        repo: root.clone(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })
    .unwrap();
    let out = temp.path().join("fields.zip");
    let prepared = prepare_package_build_task(PackageBuildTaskRequest {
        source: PackageSource::Local(root),
        temporary_root: temp.path().join("source"),
        task_id: "return_home".into(),
        game: None,
        server: None,
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: None,
        include_recovery: false,
        out: out.clone(),
        dry_run: false,
        max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        env: PackageEnvOptions::default(),
    })
    .unwrap();
    prepared
        .build(&AuthoringEnvironmentSnapshot::default())
        .expect("hash-bound fields package closure");
    let published = open_published_package(&out).unwrap();
    assert!(published.path().is_file());
    published.close().unwrap();
}

// Defect: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5553542252
#[test]
fn zero_input_fields_build_and_declaration_boundaries() {
    use actingcommand_resource_tooling::{PackageValidateRequest, validate_package};
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("resources");
    write_external_resource_fixture(&root);
    let path = root.join("operations/return_home/task.json");
    let mut task: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    task["schema_version"] = json!("0.8");
    task["operations"] = json!([]);
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
    let build = |value: &serde_json::Value, mode: &str, name: &str| {
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
            execution_mode: Some(mode.into()),
            resolution: None,
            include_recovery: false,
            out: temp.path().join(format!("{name}.zip")),
            dry_run: false,
            max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
            env: PackageEnvOptions::default(),
        })?
        .build(&AuthoringEnvironmentSnapshot::default())
    };
    build(&task, "navigable_route", "fields").expect("official zero-input build-task");
    let bytes = open_published_package(&temp.path().join("fields.zip"))
        .unwrap()
        .read_all()
        .unwrap();
    let validated = validate_package(PackageValidateRequest {
        zip_path: temp.path().join("fields.zip"),
        include_entries: true,
        expected_input_sha256: Some(actingcommand_pack_containment::Sha256Hash::digest(&bytes)),
    })
    .expect("hash-bound package validation");
    assert_eq!(validated.status, "valid");
    assert!(validated.externally_verified);
    for case in 0..11 {
        let mut invalid = task.clone();
        let mut mode = "navigable_route";
        match case {
            0 => {
                invalid.as_object_mut().unwrap().remove("target_page");
            }
            1 => invalid["target_page"] = json!("other"),
            2 => invalid["post_admission_ocr"]["page_ids"] = json!(["other"]),
            3 => invalid["scheduling_outcome"]["mappings"][0]["terminal_pages"] = json!(["other"]),
            4 => {
                invalid
                    .as_object_mut()
                    .unwrap()
                    .remove("post_admission_ocr");
            }
            5 => {
                invalid
                    .as_object_mut()
                    .unwrap()
                    .remove("scheduling_outcome");
            }
            6 => invalid["post_admission_ocr"]["fields"][0]["target_id"] = json!("ocr/missing"),
            7 => invalid["scheduling_outcome"]["designated_operation"] = json!("missing"),
            8 => mode = "recognize_only",
            9 => {
                invalid["schema_version"] = json!("0.6");
                invalid
                    .as_object_mut()
                    .unwrap()
                    .remove("post_admission_ocr");
                invalid
                    .as_object_mut()
                    .unwrap()
                    .remove("scheduling_outcome");
                invalid.as_object_mut().unwrap().remove("ocr_targets");
            }
            _ => invalid["recovery"] = json!({"task_id":"return_home","max_attempts":1}),
        }
        assert!(
            build(&invalid, mode, &format!("invalid-{case}")).is_err(),
            "case {case}"
        );
    }
}

// Specification: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5554462313
#[test]
fn explicit_postcondition_budget_build_and_runtime_admission_agree() {
    use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedContainedTask};
    use actingcommand_pack_containment::Sha256Hash;
    use std::io::{Cursor, Read, Write};
    use zip::{ZipArchive, ZipWriter, write::FileOptions};

    let temp = TempDir::new().unwrap();
    let root = temp.path().join("resources");
    write_external_resource_fixture(&root);
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

fn write_external_resource_fixture(root: &Path) {
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
    fs::write(
        root.join("operations/return_home/assets/HOME.png"),
        one_pixel_png(),
    )
    .expect("write template");
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
}

fn neutral_catalog_sources() -> CatalogSources {
    CatalogSources {
        tasks: catalog_source(
            "tasks.json",
            include_bytes!("../../../contracts/scheduling/examples/h1-neutral-activity/tasks.json"),
        ),
        pools: catalog_source(
            "pools.json",
            include_bytes!("../../../contracts/scheduling/examples/h1-neutral-activity/pools.json"),
        ),
        activity: catalog_source(
            "activity.json",
            &replace_neutral_game(include_bytes!(
                "../../../contracts/scheduling/examples/h1-neutral-activity/activity.json"
            )),
        ),
        timeline: catalog_source(
            "timeline.json",
            &replace_neutral_game(include_bytes!(
                "../../../contracts/scheduling/examples/h1-neutral-activity/timeline.json"
            )),
        ),
    }
}

fn replace_neutral_game(source: &[u8]) -> Vec<u8> {
    String::from_utf8(source.to_vec())
        .expect("catalog utf-8")
        .replace("neutral-game", GAME)
        .into_bytes()
}

fn catalog_source(name: &str, bytes: &[u8]) -> CatalogDocumentSource {
    CatalogDocumentSource::new(format!("memory://neutral-game/{name}"), bytes.to_vec())
}

fn one_pixel_png() -> &'static [u8] {
    &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9,
        251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}
