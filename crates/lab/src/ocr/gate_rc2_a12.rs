// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A12 part 1 (exec key 87; related exec key 47, LAB-3): the offline tools started
// from a single-location root (`runtime\BUILD-MANIFEST.json` two levels above the executable, no
// install selection; WD-1, register §1.96) load the OCR provider over the vision root that the
// root's `actingd.config.json` names, and a root without its configuration is
// `config_read_failed`, never "unconfigured" (coordinator decision 6 of 2026-10-19). Requirement
// basis: rd-1019/plan-0121rc2/brief-A12.md, "Requirement" 1 and 2. Synthetic folders with empty
// model files: no model is loaded and no ONNX Runtime is initialised.

use super::*;
use actingcommand_contract::outcome::Category;
use std::fs;
use tempfile::TempDir;

// A single-location root as the report reproduces it: `runtime\BUILD-MANIFEST.json`, a `tools`
// folder, the vision root `vision` with an OCR model folder (rec.onnx, keys.txt) and
// `ort\onnxruntime.dll`, and no `install\active.json`; `actingd.config.json` with a `vision`
// section when `configured`.
fn single_location_root_folder(configured: bool) -> TempDir {
    let root = TempDir::new().expect("root");
    let path = root.path();
    fs::create_dir_all(path.join("runtime")).expect("runtime folder");
    fs::write(path.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("build manifest");
    fs::create_dir_all(path.join("tools")).expect("tools folder");
    let model = path.join("vision").join("models").join("neutral-ocr");
    fs::create_dir_all(&model).expect("OCR model folder");
    fs::write(model.join("rec.onnx"), b"").expect("recognizer");
    fs::write(model.join("keys.txt"), b"").expect("dictionary");
    let ort = path.join("vision").join("ort");
    fs::create_dir_all(&ort).expect("ort folder");
    fs::write(ort.join("onnxruntime.dll"), b"").expect("runtime library");
    if configured {
        fs::write(
            path.join("actingd.config.json"),
            br#"{"vision": {"execution_provider": "cpu"}}"#,
        )
        .expect("root configuration");
    }
    root
}

#[test]
fn a12_a_single_location_root_loads_the_vision_root_its_configuration_names() {
    let root = single_location_root_folder(true);

    let ocr = LabOcr::from_single_location_root(root.path());

    assert!(
        ocr.unavailable_outcome().is_none(),
        "{:?}",
        ocr.unavailable_outcome().map(ToString::to_string)
    );
    assert!(ocr.provider().is_some(), "OCR must be available");
    let vision = fs::canonicalize(root.path().join("vision")).expect("vision root");
    assert_eq!(ocr.vision_root(), Some(vision.as_path()));
}

#[test]
fn a12_a_single_location_root_without_its_configuration_is_config_read_failed() {
    let root = single_location_root_folder(false);

    let ocr = LabOcr::from_single_location_root(root.path());

    assert!(ocr.provider().is_none(), "OCR must be unavailable");
    let outcome = ocr.unavailable_outcome().expect("the unavailable outcome");
    assert!(outcome.is(LabCode::LabOcrUnavailable), "{outcome}");
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.causes().len(), 1, "{outcome}");
    assert!(
        outcome.causes()[0]
            .outcome()
            .is(SharedCode::ConfigReadFailed),
        "{outcome}"
    );
}

#[test]
fn a12_only_a_root_with_a_build_manifest_that_is_no_slot_folder_is_a_single_location_root() {
    let folder = single_location_root_folder(true);
    let root = fs::canonicalize(folder.path()).expect("root");

    // `tools\` and `runtime\` of a root with a build manifest.
    for directory in ["tools", "runtime"] {
        assert_eq!(
            single_location_root(&root.join(directory).join("actinglab.exe")),
            Some(root.clone()),
            "{directory}"
        );
    }

    // A slot folder of an A/B installation, even with its own build manifest.
    let slot = root.join("A");
    fs::create_dir_all(slot.join("runtime")).expect("slot runtime folder");
    fs::write(slot.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("slot manifest");
    assert_eq!(
        single_location_root(&slot.join("tools").join("actinglab.exe")),
        None
    );

    // A folder without `runtime\BUILD-MANIFEST.json`.
    let bare_folder = TempDir::new().expect("bare folder");
    let bare = fs::canonicalize(bare_folder.path()).expect("bare root");
    fs::create_dir_all(bare.join("tools")).expect("bare tools folder");
    assert_eq!(
        single_location_root(&bare.join("tools").join("actinglab.exe")),
        None
    );
}
