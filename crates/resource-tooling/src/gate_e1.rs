// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-1 G10 (exec key 33), the bundle and business-catalog half:
// - `package bundle` (`contracts/package-reference.md`, "Content-directory tools and the bundle
//   index"): an existing `--out` or `<out>.part` is refused with `package_bundle_out_exists`; a
//   failure leaves `<out>.part` in place and names it, and nothing is removed;
// - the offline business catalog (`contracts/business-identity-consensus.md`, "One static
//   catalog"): deterministic output; an unknown field, a duplicate row ID and an unclosed pool
//   reference are errors.
// Requirement basis: `rd-1009/tests/req-basis-el.md` section E-1, rows "G10 bundle" and "G10
// business catalog". The plan's `package digest` rows are not written here: the frozen model
// M-PKG v1.1 (board #336, option 丙) retires `package digest` (slice S5) and the digest-named
// directory refusal `content_directory_name_mismatch` (slice S1, row S1-T1), so their required
// state is "gone", not the rows the plan lists.

use crate::{
    CompiledBusinessCatalog, PackageBundleRequest, compile_business_catalog, package_bundle,
};
use actingcommand_contract::LabResult;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const OUT_EXISTS: &str = "package_bundle_out_exists";
const PACK_ID: &str = "neutral.test.task";

// The inputs of one `package bundle` call in a temporary directory: an applications table of game
// `neutral` whose server `test` names `default_package_id`, and a packs root holding one
// text-only source pack, `neutral.test.task` of that game and server.
struct BundleInputs(TempDir);

impl BundleInputs {
    fn new(default_package_id: &str) -> Self {
        let inputs = Self(TempDir::new().expect("temporary directory"));
        let applications = json!({
            "schema_version": "actingcommand.applications.v1",
            "game": "neutral",
            "servers": {"test": {
                "application_id": "com.example.neutral",
                "label": "Test",
                "default_package_id": default_package_id
            }}
        });
        fs::write(
            inputs.applications(),
            serde_json::to_vec_pretty(&applications).expect("applications JSON"),
        )
        .expect("applications table");
        write_pack(&inputs.packs_root().join("task"));
        inputs
    }

    fn applications(&self) -> PathBuf {
        self.0.path().join("applications.json")
    }

    fn packs_root(&self) -> PathBuf {
        self.0.path().join("packs")
    }

    fn out(&self) -> PathBuf {
        self.0.path().join("bundle")
    }

    fn part(&self) -> PathBuf {
        self.0.path().join("bundle.part")
    }

    fn request(&self) -> PackageBundleRequest {
        PackageBundleRequest {
            applications: self.applications(),
            packs_root: self.packs_root(),
            out: self.out(),
            source: None,
            maintenance: None,
            root_files: Default::default(),
        }
    }

    // The inputs are still there, unchanged in place.
    fn assert_inputs_kept(&self) {
        assert!(self.applications().is_file(), "the applications table");
        for file in [
            "control.json",
            "resources/operations/resources.json",
            "resources/operations/task/task.json",
        ] {
            let path = file
                .split('/')
                .fold(self.packs_root().join("task"), |parent, part| {
                    parent.join(part)
                });
            assert!(path.is_file(), "{}", path.display());
        }
    }
}

// One pack source directory: its control, its resource table and one task whose only page is
// recognized by a color probe.
fn write_pack(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": PACK_ID,
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
    let files = [
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
    ];
    for (relative, bytes) in files {
        let path = relative
            .split('/')
            .fold(directory.to_path_buf(), |parent, part| parent.join(part));
        fs::create_dir_all(path.parent().expect("file parent")).expect("pack directory");
        fs::write(path, bytes).expect("pack file");
    }
}

// package-reference.md "Content-directory tools and the bundle index": "`--out` and
// `<out>.part` must not exist (`package_bundle_out_exists`)". An earlier bundle at `--out` is
// refused and kept as it was.
#[test]
fn gate_e1_package_bundle_refuses_an_existing_out() {
    let inputs = BundleInputs::new(PACK_ID);
    fs::create_dir(inputs.out()).expect("an earlier output");
    fs::write(inputs.out().join("bundle.json"), b"{}").expect("an earlier output file");

    let error = package_bundle(inputs.request()).expect_err("an existing --out is refused");
    assert_eq!(error.code, OUT_EXISTS, "{}", error.message);
    assert_eq!(
        fs::read(inputs.out().join("bundle.json")).expect("the earlier output is kept"),
        b"{}"
    );
    inputs.assert_inputs_kept();
}

// package-reference.md "Content-directory tools and the bundle index": the same rule for
// `<out>.part`, which an earlier failed call leaves in place. It is refused and kept as it was,
// and no `--out` is written.
#[test]
fn gate_e1_package_bundle_refuses_an_existing_part_directory() {
    let inputs = BundleInputs::new(PACK_ID);
    fs::create_dir(inputs.part()).expect("an earlier incomplete output");
    fs::write(inputs.part().join("applications.json"), b"{}").expect("an earlier output file");

    let error = package_bundle(inputs.request()).expect_err("an existing <out>.part is refused");
    assert_eq!(error.code, OUT_EXISTS, "{}", error.message);
    assert_eq!(
        fs::read(inputs.part().join("applications.json")).expect("the earlier output is kept"),
        b"{}"
    );
    assert!(!inputs.out().exists(), "no --out is written");
    inputs.assert_inputs_kept();
}

// package-reference.md "Content-directory tools and the bundle index": "The output is written to
// `<out>.part` and renamed to `--out` only when complete ... A failure leaves `<out>.part` in
// place and names it; nothing is removed." The applications table names a default package that
// no pack provides, so the call fails once the output was begun.
#[test]
fn gate_e1_package_bundle_failure_leaves_the_part_directory_and_names_it() {
    let inputs = BundleInputs::new("neutral.test.missing");

    let error = package_bundle(inputs.request()).expect_err("the default package is missing");
    assert!(inputs.part().is_dir(), "<out>.part stays in place");
    assert!(!inputs.out().exists(), "no --out is written");
    let part = inputs.part().display().to_string();
    assert!(
        error.message.contains(&part),
        "the failure names {part}: {}",
        error.message
    );
    inputs.assert_inputs_kept();
}

// A business catalog of two rows over two pools (business-identity-consensus.md "One static
// catalog" and its reward shape).
fn catalog() -> Value {
    let source = json!({"uri": "https://example.invalid/source", "date": "2026-10-03"});
    json!({
        "schema_version": "actingcommand.business-catalog.v1",
        "catalog_id": "neutral-catalog",
        "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                        "minimum_confidence_milli": 800},
        "pools": {"material": 10, "token": 5},
        "entries": [
            {"id": "first", "names": {"test": ["First item", "First"]}, "duration_seconds": 60,
             "costs": [{"pool_id": "token", "amount": 1}],
             "rewards": [{"pool_id": "material", "quantity_milli": 1000, "probability_milli": 400,
                          "batches": 1, "confidence_milli": 700,
                          "observation_source": "self_reported"}],
             "source": source},
            {"id": "second", "names": {"test": ["Second item"]}, "preference_milli": 250,
             "source": source}
        ]
    })
}

fn compile(value: &Value) -> LabResult<CompiledBusinessCatalog> {
    compile_business_catalog(
        &serde_json::to_vec(value).expect("catalog JSON"),
        "test",
        "business_id",
    )
}

// business-identity-consensus.md "One static catalog": `resource catalog` "returns
// deterministic authoring JSON without network access": the same input gives the same bytes.
#[test]
fn gate_e1_business_catalog_output_is_deterministic() {
    let bytes = serde_json::to_vec(&catalog()).expect("catalog JSON");
    let first = compile_business_catalog(&bytes, "test", "business_id").expect("first compile");
    let second = compile_business_catalog(&bytes, "test", "business_id").expect("second compile");
    assert_eq!(
        serde_json::to_vec(&first).expect("first output JSON"),
        serde_json::to_vec(&second).expect("second output JSON")
    );
}

// business-identity-consensus.md "One static catalog": "Unknown fields ... are errors".
#[test]
fn gate_e1_business_catalog_refuses_an_unknown_field() {
    compile(&catalog()).expect("the catalog without the field compiles");
    let mut top = catalog();
    top["comment"] = json!("unused");
    compile(&top).expect_err("an unknown catalog field is refused");
    let mut row = catalog();
    row["entries"][0]["comment"] = json!("unused");
    compile(&row).expect_err("an unknown row field is refused");
}

// business-identity-consensus.md "One static catalog": "Each entry has a unique stable `id`";
// "duplicate row IDs ... are errors".
#[test]
fn gate_e1_business_catalog_refuses_a_duplicate_row_id() {
    compile(&catalog()).expect("the catalog with unique IDs compiles");
    let mut duplicate = catalog();
    duplicate["entries"][1]["id"] = json!("first");
    compile(&duplicate).expect_err("a duplicate row ID is refused");
}

// business-identity-consensus.md "One static catalog": "unclosed pool references are errors",
// for a cost and for a reward.
#[test]
fn gate_e1_business_catalog_refuses_an_undeclared_pool() {
    compile(&catalog()).expect("the catalog whose pools are declared compiles");
    let mut cost = catalog();
    cost["entries"][0]["costs"][0]["pool_id"] = json!("undeclared");
    compile(&cost).expect_err("a cost naming an undeclared pool is refused");
    let mut reward = catalog();
    reward["entries"][0]["rewards"][0]["pool_id"] = json!("undeclared");
    compile(&reward).expect_err("a reward naming an undeclared pool is refused");
}
