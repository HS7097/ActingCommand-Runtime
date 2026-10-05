// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_policy::{CatalogDocumentSource, CatalogSources, compile_catalog};
use actingcommand_resource_tooling::OperationParser;
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "fourth-game";
const SERVER: &str = "test-shard";
const LOCALE: &str = "x-fixture";

#[test]
fn external_neutral_game_metadata_parses_and_schedules() {
    let temp = TempDir::new().expect("temp dir");
    let resource_root = temp.path().join("external-resources");
    write_external_resource_fixture(&resource_root);

    let parser = OperationParser::load(&resource_root, None, None, None).unwrap();
    assert_eq!(parser.game, GAME);
    assert_eq!(parser.server, SERVER);
    assert_eq!(parser.locale, LOCALE);
    let outputs = parser.build_all().unwrap();
    assert_eq!(outputs.pack["game"], GAME);
    assert_eq!(outputs.pack["server"], SERVER);

    let catalog = compile_catalog(&neutral_catalog_sources())
        .expect("compile scheduling catalog for external neutral game");
    assert!(catalog.summary().counts.tasks > 0);
}

// Specification 1: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5551203604
#[test]
fn fields_v1_neutral_declaration_and_reference_closure() {
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
        OperationParser::load(&root, None, None, None)
            .map_err(Box::new)
            .and_then(|parser| parser.build_all().map_err(Box::new))
            .map_err(Box::new)
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
    let outputs = OperationParser::load(&root, None, None, None)
        .unwrap()
        .build_all()
        .unwrap();
    assert!(
        outputs.pack["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target["id"] == "ocr/name")
    );
}

// Defect: https://github.com/HS7097/ActingCommand-Workflow/issues/269#issuecomment-5553542252
#[test]
fn zero_input_fields_source_and_admission_boundaries() {
    use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedContainedTask};
    use actingcommand_pack_containment::Sha256Hash;
    use actingcommand_resource_tooling::{PackageValidateRequest, validate_package};
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::FileOptions};
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
        let outputs = OperationParser::load(&root, None, None, None)?.build_all()?;
        let mut entries = std::collections::BTreeMap::from([
            ("control.json", serde_json::to_vec(&json!({"schema_version":"Lab-1y.control.v1","package_id":"neutral.fields","execution_mode":mode,
                "game":GAME,"server":SERVER,"resolution":{"width":1280,"height":720},"entry_task_id":"return_home"})).unwrap()),
            ("resources/operations/return_home/task.json", serde_json::to_vec(value).unwrap()),
            ("resources/operations/return_home/assets/HOME.png", one_pixel_png().to_vec()),
            ("resources/operations/resources.json", fs::read(root.join("operations/resources.json")).unwrap()),
            ("resources/recognition/fourth-game.test-shard.pack.json", serde_json::to_vec(&outputs.pack).unwrap()),
            ("resources/recognition/fourth-game.test-shard.pages.json", serde_json::to_vec(&outputs.pages).unwrap()),
            ("resources/navigation/fourth-game.test-shard.navigation.json", serde_json::to_vec(&outputs.navigation).unwrap()),
        ]);
        let files = entries.iter().filter_map(|(path, bytes)| path.strip_prefix("resources/").map(|path| json!({"path":path,"sha256":format!("sha256:{}", Sha256Hash::digest(bytes))}))).collect::<Vec<_>>();
        entries.insert(
            "resources/manifest.json",
            serde_json::to_vec(
                &json!({"schema_version":"0.3","entry_task_id":"return_home","files":files}),
            )
            .unwrap(),
        );
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, bytes) in entries {
            writer.start_file(path, FileOptions::default()).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        PreparedContainedTask::load(
            "neutral-fields",
            &bytes,
            ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(&bytes).to_string()).unwrap(),
        )
        .map_err(|error| actingcommand_contract::LabError::package_invalid(error.to_string()))?;
        fs::write(temp.path().join(format!("{name}.zip")), &bytes).unwrap();
        Ok::<_, Box<actingcommand_contract::LabError>>(())
    };
    build(&task, "navigable_route", "fields").expect("zero-input source and Runtime admission");
    let bytes = fs::read(temp.path().join("fields.zip")).unwrap();
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
        selection: None,
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
