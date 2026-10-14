// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-1 G9 (exec key 25): the model folder listing of
// `contracts/vision-model-folders.md` (sections "Folder layouts", "Description `model.json`"
// and "Invalid folders"), in a temporary directory. A folder that breaks the rule is listed with
// its reason and does not stop the listing (Workflow #360 Q3: an invalid folder does not block
// startup; the contract: startup "keeps running; every request naming it fails"); the valid
// folders beside it are listed as usual. Requirement basis: `rd-1009/tests/req-basis-el.md`
// section E-1, row "G9 model folders". Model files are placeholders: the listing reads only
// directory entries, file metadata and descriptions.

use crate::{NnModelSpec, OcrModelLayout, OcrModelSpec, VisionModelListing, list_vision_models};
use std::fs;
use std::path::PathBuf;

const ONNX: &[u8] = b"placeholder model bytes";
const KEYS: &[u8] = b"a\nb\nc\n";

// A `models` directory of its own below the system temporary directory, removed when dropped.
struct Models(PathBuf);

impl Models {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "actingcommand-gate-e1-models-{}-{label}",
            std::process::id()
        ));
        // A directory left behind by an aborted earlier run of the same process id would add
        // its folders to this listing.
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("models")).expect("models directory");
        Self(root)
    }

    fn path(&self) -> PathBuf {
        self.0.join("models")
    }

    // Writes `relative` (segments separated by `/`) inside the model folder `folder`.
    fn file(&self, folder: &str, relative: &str, bytes: &[u8]) {
        let path = relative
            .split('/')
            .fold(self.path().join(folder), |parent, part| parent.join(part));
        fs::create_dir_all(path.parent().expect("file parent")).expect("model folder");
        fs::write(path, bytes).expect("model file");
    }

    // A complete flat OCR folder without a detector.
    fn flat_text(&self, folder: &str) {
        self.file(folder, "rec.onnx", ONNX);
        self.file(folder, "keys.txt", KEYS);
    }

    // A complete nested OCR folder without a detector.
    fn nested_text(&self, folder: &str) {
        self.file(folder, "rec/inference.onnx", ONNX);
        self.file(folder, "rec/keys.txt", KEYS);
    }

    fn list(&self) -> VisionModelListing {
        list_vision_models(&self.path()).expect("an invalid folder never fails the listing")
    }
}

impl Drop for Models {
    fn drop(&mut self) {
        // A failed removal leaves a temporary directory behind; it never changes a result.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ocr<'a>(listing: &'a VisionModelListing, name: &str) -> &'a OcrModelSpec {
    listing
        .ocr
        .iter()
        .find(|spec| spec.model_ref == name)
        .unwrap_or_else(|| panic!("{name} is listed as an OCR model: {listing:?}"))
}

fn nn<'a>(listing: &'a VisionModelListing, name: &str) -> &'a NnModelSpec {
    listing
        .nn
        .iter()
        .find(|spec| spec.model_ref == name)
        .unwrap_or_else(|| panic!("{name} is listed as an NN model: {listing:?}"))
}

// vision-model-folders.md "Invalid folders": the folder is recorded with its path and its
// reason ("one `invalid_model:<name>` binding per invalid folder with its reason") and is not a
// model.
fn assert_invalid(listing: &VisionModelListing, models: &Models, name: &str) {
    let path = models.path().join(name);
    let entry = listing
        .invalid
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{name} is listed as invalid: {listing:?}"));
    assert!(!entry.reason.trim().is_empty(), "{name} has a reason");
    assert!(
        listing.ocr.iter().all(|spec| spec.model_ref != name)
            && listing.nn.iter().all(|spec| spec.model_ref != name),
        "{name} is not listed as a model: {listing:?}"
    );
}

// vision-model-folders.md "Folder layouts", column Flat: an optional `det.onnx`, `rec.onnx` and
// `keys.txt` for OCR, `model.onnx` for an NN classifier; "Description `model.json`": a folder
// without `model.json` has the family's defaults (`ppocr-ctc` for OCR, `onnx-classify` for NN).
#[test]
fn gate_e1_vision_models_flat_layout_is_listed() {
    let models = Models::new("flat");
    models.file("text-full", "det.onnx", ONNX);
    models.flat_text("text-full");
    models.flat_text("text-regions");
    models.file("classifier", "model.onnx", ONNX);

    let listing = models.list();
    assert!(listing.invalid.is_empty(), "{:?}", listing.invalid);
    assert_eq!((listing.ocr.len(), listing.nn.len()), (2, 1), "{listing:?}");

    let folder = models.path().join("text-full");
    let full = ocr(&listing, "text-full");
    assert_eq!(full.layout, OcrModelLayout::Flat);
    assert_eq!(full.detector_path, Some(folder.join("det.onnx")));
    assert_eq!(full.recognizer_path, folder.join("rec.onnx"));
    assert_eq!(full.dictionary_path, folder.join("keys.txt"));
    assert_eq!(full.description.family, "ppocr-ctc");

    let folder = models.path().join("text-regions");
    let regions = ocr(&listing, "text-regions");
    assert_eq!(regions.layout, OcrModelLayout::Flat);
    assert_eq!(regions.detector_path, None);
    assert_eq!(regions.recognizer_path, folder.join("rec.onnx"));
    assert_eq!(regions.dictionary_path, folder.join("keys.txt"));

    let classifier = nn(&listing, "classifier");
    assert_eq!(
        classifier.model_path,
        models.path().join("classifier").join("model.onnx")
    );
    assert_eq!(classifier.description.family, "onnx-classify");
}

// vision-model-folders.md "Folder layouts", column Nested: an optional `det\inference.onnx`,
// `rec\inference.onnx` and `rec\keys.txt`.
#[test]
fn gate_e1_vision_models_nested_layout_is_listed() {
    let models = Models::new("nested");
    models.file("text-full", "det/inference.onnx", ONNX);
    models.nested_text("text-full");
    models.nested_text("text-regions");

    let listing = models.list();
    assert!(listing.invalid.is_empty(), "{:?}", listing.invalid);
    assert_eq!((listing.ocr.len(), listing.nn.len()), (2, 0), "{listing:?}");

    let folder = models.path().join("text-full");
    let full = ocr(&listing, "text-full");
    assert_eq!(full.layout, OcrModelLayout::Nested);
    assert_eq!(
        full.detector_path,
        Some(folder.join("det").join("inference.onnx"))
    );
    assert_eq!(
        full.recognizer_path,
        folder.join("rec").join("inference.onnx")
    );
    assert_eq!(full.dictionary_path, folder.join("rec").join("keys.txt"));
    assert_eq!(full.description.family, "ppocr-ctc");

    let folder = models.path().join("text-regions");
    let regions = ocr(&listing, "text-regions");
    assert_eq!(regions.layout, OcrModelLayout::Nested);
    assert_eq!(regions.detector_path, None);
    assert_eq!(
        regions.recognizer_path,
        folder.join("rec").join("inference.onnx")
    );
    assert_eq!(regions.dictionary_path, folder.join("rec").join("keys.txt"));
}

// vision-model-folders.md "Folder layouts": the OCR recognizer and dictionary are required in
// either layout; "Invalid folders": "a layout is incomplete".
#[test]
fn gate_e1_vision_models_missing_rec_or_keys_is_invalid_and_does_not_block_the_listing() {
    let models = Models::new("incomplete");
    models.file("flat-without-keys", "det.onnx", ONNX);
    models.file("flat-without-keys", "rec.onnx", ONNX);
    models.file("flat-without-rec", "det.onnx", ONNX);
    models.file("flat-without-rec", "keys.txt", KEYS);
    models.file("nested-without-keys", "rec/inference.onnx", ONNX);
    models.file("nested-without-rec", "rec/keys.txt", KEYS);
    models.flat_text("text");

    let listing = models.list();
    for name in [
        "flat-without-keys",
        "flat-without-rec",
        "nested-without-keys",
        "nested-without-rec",
    ] {
        assert_invalid(&listing, &models, name);
    }
    assert_eq!(listing.invalid.len(), 4, "{listing:?}");
    assert_eq!(ocr(&listing, "text").layout, OcrModelLayout::Flat);
}

// vision-model-folders.md "Folder layouts": a model folder holds exactly one layout;
// "Invalid folders": "it mixes layouts".
#[test]
fn gate_e1_vision_models_mixed_layout_is_invalid_and_does_not_block_the_listing() {
    let models = Models::new("mixed");
    models.flat_text("flat-and-nested");
    models.nested_text("flat-and-nested");
    models.flat_text("text-and-classifier");
    models.file("text-and-classifier", "model.onnx", ONNX);
    models.nested_text("nested-and-classifier");
    models.file("nested-and-classifier", "model.onnx", ONNX);
    models.file("classifier", "model.onnx", ONNX);

    let listing = models.list();
    for name in [
        "flat-and-nested",
        "text-and-classifier",
        "nested-and-classifier",
    ] {
        assert_invalid(&listing, &models, name);
    }
    assert_eq!(listing.invalid.len(), 3, "{listing:?}");
    assert_eq!(
        nn(&listing, "classifier").model_path,
        models.path().join("classifier").join("model.onnx")
    );
}

// vision-model-folders.md "Folder layouts": an angle classifier (`cls.onnx` or `cls\`) is not
// supported; "Invalid folders": "it holds an angle classifier".
#[test]
fn gate_e1_vision_models_with_cls_is_invalid_and_does_not_block_the_listing() {
    let models = Models::new("cls");
    models.flat_text("flat-with-cls");
    models.file("flat-with-cls", "cls.onnx", ONNX);
    models.nested_text("nested-with-cls");
    models.file("nested-with-cls", "cls/inference.onnx", ONNX);
    models.flat_text("text");

    let listing = models.list();
    assert_invalid(&listing, &models, "flat-with-cls");
    assert_invalid(&listing, &models, "nested-with-cls");
    assert_eq!(listing.invalid.len(), 2, "{listing:?}");
    assert_eq!(ocr(&listing, "text").layout, OcrModelLayout::Flat);
}

// vision-model-folders.md "Description `model.json`": UTF-8 JSON; `family` is `ppocr-ctc` for an
// OCR folder and `onnx-classify` for an NN folder and must match the layout; "Invalid folders":
// "its `model.json` is unreadable, invalid or names another family".
#[test]
fn gate_e1_vision_models_malformed_or_mismatched_model_json_is_invalid() {
    let models = Models::new("description");
    models.flat_text("text-described");
    models.file(
        "text-described",
        "model.json",
        br#"{"schema_version":"actingcommand.vision_model.v1","family":"ppocr-ctc","languages":["en"]}"#,
    );
    // A description whose save was cut short.
    models.flat_text("text-malformed");
    models.file(
        "text-malformed",
        "model.json",
        br#"{"schema_version":"actingcommand.vision_model.v1","family":"ppocr-ctc""#,
    );
    models.flat_text("text-declared-classifier");
    models.file(
        "text-declared-classifier",
        "model.json",
        br#"{"schema_version":"actingcommand.vision_model.v1","family":"onnx-classify"}"#,
    );
    models.file("classifier-declared-text", "model.onnx", ONNX);
    models.file(
        "classifier-declared-text",
        "model.json",
        br#"{"schema_version":"actingcommand.vision_model.v1","family":"ppocr-ctc"}"#,
    );

    let listing = models.list();
    for name in [
        "text-malformed",
        "text-declared-classifier",
        "classifier-declared-text",
    ] {
        assert_invalid(&listing, &models, name);
    }
    assert_eq!(listing.invalid.len(), 3, "{listing:?}");
    let described = ocr(&listing, "text-described");
    assert_eq!(described.description.family, "ppocr-ctc");
    assert_eq!(described.description.languages, ["en"]);
}

// vision-model-folders.md "Invalid folders": a folder whose name is not a valid `model_ref`
// (here: over 255 bytes, 86 characters of three UTF-8 bytes each, 258 bytes) breaks the rule;
// "Vision root": `models\<name>\`, where `<name>` is its `model_ref`.
#[test]
fn gate_e1_vision_models_illegal_name_is_invalid_and_does_not_block_the_listing() {
    let models = Models::new("name");
    let long_name = "\u{6a21}\u{578b}".repeat(43);
    assert_eq!(long_name.len(), 258);
    models.flat_text(&long_name);
    models.flat_text("text");

    let listing = models.list();
    assert_invalid(&listing, &models, &long_name);
    assert_eq!(listing.invalid.len(), 1, "{listing:?}");
    assert_eq!(ocr(&listing, "text").layout, OcrModelLayout::Flat);
}
