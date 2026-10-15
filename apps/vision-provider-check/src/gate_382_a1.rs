// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7, G15; inventory-apps key 39): vision-provider-check
// gives each vision root the code actingd's listing and `check-config` refuse it with
// (`model_folders_report` doc: "It fails where actingd's listing and `check-config` fail").
// The roots and their codes are tests/support/vision_folders.rs, which actingd's
// `gate_382_a1` runs `check-config` on.

use super::{CheckOptions, model_folders_report};
use actingcommand_vision_ffi::{VISION_MODELS_DIRECTORY, VisionListingCode};
use std::fs;
use std::path::PathBuf;

#[path = "../../../tests/support/vision_folders.rs"]
mod vision_folders;

/// An empty folder for this test's vision roots in the system temporary folder, named by this
/// process (this crate takes no temporary-folder dependency).
fn scratch() -> PathBuf {
    let base =
        std::env::temp_dir().join(format!("vision-provider-check-g15-{}", std::process::id()));
    if base.exists() {
        fs::remove_dir_all(&base).expect("clear the scratch folder");
    }
    fs::create_dir_all(&base).expect("create the scratch folder");
    base
}

#[test]
fn gate_the_model_folder_report_gives_each_vision_root_its_code() {
    let base = scratch();
    for (case, expected) in vision_folders::CASES {
        let root = vision_folders::vision_root(&base, case);
        let report = model_folders_report(&CheckOptions {
            models_root: root.join(VISION_MODELS_DIRECTORY),
            hash: false,
        })
        .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(report.code, expected.map(VisionListingCode::code), "{case}");
        assert_eq!(report.ok, expected.is_none(), "{case}");
    }
    fs::remove_dir_all(&base).expect("remove the scratch folder");
}
