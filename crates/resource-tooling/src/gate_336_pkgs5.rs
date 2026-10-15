// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S5, model M-PKG v1.1 §4.6 S5-T1: `package bundle` lays out each pack in
// `packs/<package_id>/` and writes the bundle index v4, whose packs carry no digest; two pack
// directories that declare one `package_id` are refused. The packs are text-only neutral source
// packs, each with one task whose only page is recognized by a color probe.

use crate::{PackageBundleRequest, package_bundle};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const INDEX_INVALID: &str = "package_bundle_index_invalid";

// One pack source directory of game `neutral`, server `test`, declaring `package_id`.
fn write_pack(directory: &Path, package_id: &str) {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task"
    });
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "neutral bundle fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [{
            "id": "home_color",
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": [255, 0, 0]
        }],
        "page_rules": {"home": {"required": ["home_color"]}},
        "entry_page": "home",
        "target_page": "home",
        "operations": []
    });
    for (relative, bytes) in [
        (
            "control.json",
            serde_json::to_vec_pretty(&control).expect("control JSON"),
        ),
        (
            "resources/operations/resources.json",
            br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#.to_vec(),
        ),
        (
            "resources/operations/task/task.json",
            serde_json::to_vec_pretty(&task).expect("task JSON"),
        ),
    ] {
        let path = relative
            .split('/')
            .fold(directory.to_path_buf(), |parent, part| parent.join(part));
        fs::create_dir_all(path.parent().expect("file parent")).expect("pack directory");
        fs::write(path, bytes).expect("pack file");
    }
}

// The applications table of game `neutral` with its server `test`, and one pack source
// directory per `(directory name, package_id)`.
fn inputs(packs: &[(&str, &str)]) -> (TempDir, PackageBundleRequest) {
    let temp = TempDir::new().expect("temporary directory");
    let applications = temp.path().join("applications.json");
    fs::write(
        &applications,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": "neutral",
            "servers": {"test": {
                "application_id": "com.example.neutral",
                "label": "Test"
            }}
        }))
        .expect("applications JSON"),
    )
    .expect("applications table");
    let packs_root = temp.path().join("sources");
    for (directory, package_id) in packs {
        write_pack(&packs_root.join(directory), package_id);
    }
    let request = PackageBundleRequest {
        applications,
        packs_root,
        out: temp.path().join("bundle"),
        source: None,
        maintenance: None,
    };
    (temp, request)
}

// Every object member name of `value`, at any depth.
fn member_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                names.push(name.clone());
                member_names(member, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                member_names(item, names);
            }
        }
        _ => {}
    }
}

// S5-T1: each pack is copied into `packs/<package_id>/` and indexed as bundle v4 by its
// `package_id`, whatever its source directory is called; neither the answer nor the written
// `bundle.json` carries a digest member.
#[test]
fn s5_t1_package_bundle_lays_out_packs_by_package_id_in_bundle_v4_without_a_digest() {
    let (temp, request) = inputs(&[
        ("first", "neutral.test.first"),
        ("second", "neutral.test.second"),
    ]);
    let out = request.out.clone();
    let response =
        package_bundle(request).unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(response.status, "written");

    let written: Value =
        serde_json::from_slice(&fs::read(out.join("bundle.json")).expect("bundle.json"))
            .expect("bundle JSON");
    assert_eq!(written["schema_version"], "actingcommand.bundle.v4");
    assert_eq!(
        written["packs"],
        json!([
            {"package_id": "neutral.test.first", "server": "test", "entry_task_id": "task",
             "path": "packs/neutral.test.first", "file_count": 3,
             "byte_count": written["packs"][0]["byte_count"]},
            {"package_id": "neutral.test.second", "server": "test", "entry_task_id": "task",
             "path": "packs/neutral.test.second", "file_count": 3,
             "byte_count": written["packs"][1]["byte_count"]}
        ])
    );
    for package_id in ["neutral.test.first", "neutral.test.second"] {
        let directory = out.join("packs").join(package_id);
        assert!(directory.join("control.json").is_file(), "{package_id}");
        assert!(
            directory
                .join("resources/operations/task/task.json")
                .is_file(),
            "{package_id}"
        );
    }
    let packs = fs::read_dir(out.join("packs"))
        .expect("packs directory")
        .map(|entry| entry.expect("pack entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(packs.len(), 2, "{packs:?}");

    let mut names = Vec::new();
    member_names(&written, &mut names);
    member_names(
        &serde_json::to_value(&response).expect("answer JSON"),
        &mut names,
    );
    for name in &names {
        assert!(
            !name.contains("digest") && !name.contains("sha256"),
            "a member names a digest: {name}"
        );
    }
    drop(temp);
}

// S5-T1: two pack source directories that declare one `package_id` are refused; the bundle
// holds one pack per `package_id`, and `--out` is not written.
#[test]
fn s5_t1_two_packs_declaring_one_package_id_are_refused() {
    let (_temp, request) = inputs(&[
        ("first", "neutral.test.task"),
        ("second", "neutral.test.task"),
    ]);
    let out = request.out.clone();
    let error = package_bundle(request).expect_err("one package_id twice is refused");
    assert_eq!(error.code, INDEX_INVALID, "{}", error.message);
    assert!(
        error.message.contains("neutral.test.task"),
        "{}",
        error.message
    );
    assert!(!out.exists(), "--out was written");
}
