// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #382 A-1 G15 (test plan v2.2 §3.7; inventory-apps key 39): the vision roots that
//! both vision checks read, and the one code each root must get from both. vision-provider-check
//! lists `<root>\models` as actingd's startup listing does, and actingd `check-config` refuses a
//! vision root with the same code (`vision_root_unavailable`, `vision_models_empty`,
//! `vision_model_folder_invalid`; contracts/outcome-codes, the vision-ffi fragment). The two
//! programs have no library target, so each runs its own check on these roots in its own test
//! binary against this one table.

use actingcommand_vision_ffi::{
    ONNXRUNTIME_LIBRARY, VISION_MODELS_DIRECTORY, VISION_RUNTIME_DIRECTORY, VisionListingCode,
};
use std::fs;
use std::path::{Path, PathBuf};

/// Each case and the code both checks give its vision root; `None` when the root passes.
pub const CASES: [(&str, Option<VisionListingCode>); 4] = [
    ("missing", Some(VisionListingCode::VisionRootUnavailable)),
    ("empty", Some(VisionListingCode::VisionModelsEmpty)),
    ("invalid", Some(VisionListingCode::VisionModelFolderInvalid)),
    ("valid", None),
];

/// Builds the vision root of `case` under `base` and returns it. Besides `missing`, each root has
/// ONNX Runtime's library as an empty placeholder (nothing is loaded) and a `models` folder:
/// empty, holding one folder that breaks the folder rule (the flat OCR layout without its
/// `keys.txt`), or holding one flat OCR model (`rec.onnx` and `keys.txt`).
pub fn vision_root(base: &Path, case: &str) -> PathBuf {
    let root = base.join(case);
    if case == "missing" {
        return root;
    }
    let models = root.join(VISION_MODELS_DIRECTORY);
    fs::create_dir_all(&models).expect("models folder");
    let runtime = root.join(VISION_RUNTIME_DIRECTORY);
    fs::create_dir_all(&runtime).expect("runtime folder");
    fs::write(runtime.join(ONNXRUNTIME_LIBRARY), b"").expect("runtime placeholder");
    let model = |name: &str, files: &[&str]| {
        let folder = models.join(name);
        fs::create_dir_all(&folder).expect("model folder");
        for file in files {
            fs::write(folder.join(file), b"a").expect("model file");
        }
    };
    match case {
        "empty" => {}
        "invalid" => model("neutral.broken", &["rec.onnx"]),
        "valid" => model("neutral.model", &["rec.onnx", "keys.txt"]),
        other => panic!("no vision root case {other}"),
    }
    root
}
