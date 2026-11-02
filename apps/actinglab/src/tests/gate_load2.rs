// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #407 LOAD-2 (register §1.258; design `rd-1031/check-speed/model-loading.md` §6, §7
// LOAD-2; coordinator brief `brief-load-2.md`, scope items 1 and 2):
//
// - `package check` of a pack that carries a loading copy answers the loading set the Runtime
//   compiled from it, `loading: {pages, max_ms}`, its pages in the copy's order without the game;
//   a pack without one answers as before, without the member.
// - `resource validate` takes a game root's `loading.json` (family `standard_pack_loading`) and a
//   pack's loading copy (family `pack_loading`) when the copy is the one the root file makes for
//   the pack, and refuses a stale copy as `resource_declaration_invalid` naming the copy, the
//   problem `loading_pack_stale` and the pack.
//
// The pack is the neutral content directory of the package-check gates (2 x 1); the loading target
// `loading/bar` is a template of its whole frame, black then white.

use super::*;
use crate::package_tool::gate_390_tool1::neutral_package;
use actingcommand_resource_tooling::{PackageLoadingMode, PackageLoadingRequest, package_loading};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// The package id the neutral content directory declares.
const PACKAGE_ID: &str = "neutral.edit.source";
const COPY: &str = "resources/loading.json";

fn bar_png() -> Vec<u8> {
    encode_rgb_png(2, 1, &[[0, 0, 0], [255, 255, 255]])
}

fn bar() -> Value {
    json!({"id": "loading/bar", "template": "loading/bar.png",
           "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 2, "height": 1}},
           "threshold": 0.8})
}

fn pretty(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("fixture JSON");
    bytes.push(b'\n');
    bytes
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
    fs::write(path, bytes).expect("fixture file");
}

fn package_check(package: &Path) -> CliResult {
    run_cli(
        [
            "--json",
            "package",
            "check",
            "--package",
            package.to_str().expect("a UTF-8 path"),
        ],
        true,
    )
}

#[test]
fn package_check_lists_the_effective_loading_set() {
    let temp = TempDir::new().expect("a temporary directory");
    let plain = temp.path().join("plain");
    neutral_package(&plain);
    let result = package_check(&plain);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the answer");
    assert!(data.get("loading").is_none(), "{}", result.envelope_json());

    let carrying = temp.path().join("carrying");
    neutral_package(&carrying);
    write(
        &carrying.join(COPY),
        &pretty(&json!({
            "schema_version": "actingcommand.pack-loading.v1",
            "coordinate_space": {"width": 2, "height": 1},
            "max_ms": 45000,
            "anchors": [bar()],
            "pages": [
                {"id": "transition", "required": ["loading/bar"]},
                {"id": "download", "required": ["loading/bar"]}
            ]
        })),
    );
    write(&carrying.join("resources/loading/bar.png"), &bar_png());
    let result = package_check(&carrying);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the answer");
    assert_eq!(data["status"], "admitted", "{}", result.envelope_json());
    assert_eq!(
        data["loading"],
        json!({"pages": ["transition", "download"], "max_ms": 45000}),
        "{}",
        result.envelope_json()
    );
}

#[test]
fn resource_validate_accepts_a_current_loading_copy_and_names_a_stale_one() {
    let temp = TempDir::new().expect("a temporary directory");
    let repo = temp.path().join("repo");
    write(
        &repo.join("loading.json"),
        &pretty(&json!({
            "schema_version": "actingcommand.loading.v1",
            "game": "neutral",
            "coordinate_space": {"width": 2, "height": 1},
            "max_ms": 45000,
            "anchors": [bar()],
            "pages": [{"id": "transition", "required": ["loading/bar"]}]
        })),
    );
    write(&repo.join("loading/bar.png"), &bar_png());
    let pack = repo.join("packs").join(PACKAGE_ID);
    neutral_package(&pack);
    package_loading(PackageLoadingRequest {
        root_file: repo.join("loading.json"),
        packs_root: repo.join("packs"),
        mode: PackageLoadingMode::Write,
    })
    .unwrap_or_else(|error| panic!("package loading --write: {}", error.message));
    let copy = format!("packs/{PACKAGE_ID}/{COPY}");
    let validate = |paths: &[&str]| {
        let mut args = vec![
            "--json".to_owned(),
            "resource".to_owned(),
            "validate".to_owned(),
            "--repo".to_owned(),
            repo.to_str().expect("a UTF-8 path").to_owned(),
        ];
        for path in paths {
            args.push("--changed-path".to_owned());
            args.push((*path).to_owned());
        }
        run_cli(args, true)
    };

    let result = validate(&["loading.json", copy.as_str()]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the validation");
    assert_eq!(data["status"], "valid");
    assert_eq!(
        data["entries"],
        json!([
            {"path": "loading.json", "status": "valid", "family": "standard_pack_loading"},
            {"path": copy, "status": "valid", "family": "pack_loading"}
        ]),
        "{}",
        result.envelope_json()
    );

    // The copy of another allowance than the root file's.
    let mut stale: Value =
        serde_json::from_slice(&fs::read(pack.join(COPY)).expect("the copy")).expect("copy JSON");
    stale["max_ms"] = json!(60000);
    write(&pack.join(COPY), &pretty(&stale));
    let result = validate(&[copy.as_str()]);
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().expect("a refusal");
    assert_eq!(error.code, "resource_declaration_invalid");
    let details = error.details.as_ref().expect("its details");
    assert_eq!(details["declaration_file"], json!(copy));
    assert_eq!(details["standard_pack_problem"], "loading_pack_stale");
    assert_eq!(details["package_id"], PACKAGE_ID);
}
