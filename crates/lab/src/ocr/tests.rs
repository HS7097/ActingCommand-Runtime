// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 LAB-3 (FCM T-02, L-02): the offline tools load the Runtime's OCR provider over
// the vision root's model folders on the CPU, the Lab self-test evaluates OCR marks with it,
// and without an OCR model OCR is unavailable under a registered code, never skipped. Neutral
// synthetic folders, frames and a scripted provider; no model is loaded and no ONNX Runtime is
// initialised.

use super::*;
use crate::recording::frames::LoadedFrame;
use crate::recording::marks::{EvalFrame, evaluate_mark};
use crate::recording::model::{RecordedMark, SelfTestStatus};
use crate::recording::steps::default_recording_defaults;
use actingcommand_contract::LabErrorClass;
use actingcommand_contract::outcome::Category;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_page_detector::load_page_set_from_json_str;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 64;
const FRAME_HEIGHT: u32 = 32;

// Reads "Start" on a frame whose first pixel is red and "Stop" on any other, at confidence 0.95.
#[derive(Debug)]
struct ScriptedOcr;

impl VisionProvider for ScriptedOcr {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let text = if request.frame.rgb8_pixels.first() == Some(&255) {
            "Start"
        } else {
            "Stop"
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: text.to_owned(),
                rect: request.region,
                confidence: Some(0.95),
            }],
            confidence: Some(0.95),
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// A vision root with ONNX Runtime and the given model folders: `ocr` holds rec.onnx and
// keys.txt, `nn` holds model.onnx. The files are empty: listing reads no model file.
fn vision_root(runtime: bool, ocr: &[&str], nn: &[&str]) -> TempDir {
    let root = TempDir::new().expect("vision root");
    fs::create_dir_all(root.path().join("models")).expect("models folder");
    if runtime {
        fs::create_dir_all(root.path().join("ort")).expect("ort folder");
        fs::write(root.path().join("ort").join("onnxruntime.dll"), b"").expect("runtime library");
    }
    for name in ocr {
        let folder = root.path().join("models").join(name);
        fs::create_dir_all(&folder).expect("OCR model folder");
        fs::write(folder.join("rec.onnx"), b"").expect("recognizer");
        fs::write(folder.join("keys.txt"), b"").expect("dictionary");
    }
    for name in nn {
        let folder = root.path().join("models").join(name);
        fs::create_dir_all(&folder).expect("NN model folder");
        fs::write(folder.join("model.onnx"), b"").expect("classifier");
    }
    root
}

// The cause of an unavailable OCR, after checking the top: `lab_ocr_unavailable`, an error.
fn cause(ocr: &LabOcr) -> Outcome {
    assert!(ocr.provider().is_none(), "OCR must be unavailable");
    let outcome = ocr.unavailable_outcome().expect("the unavailable outcome");
    assert!(outcome.is(LabCode::LabOcrUnavailable), "{outcome}");
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.causes().len(), 1, "{outcome}");
    outcome.causes()[0].outcome().clone()
}

fn path_text(path: &Path) -> Value {
    json!(path.to_string_lossy())
}

#[test]
fn lab3_a_vision_root_with_an_ocr_model_folder_gives_the_runtime_provider() {
    let root = vision_root(true, &["neutral-ocr"], &["neutral-nn"]);

    let ocr = LabOcr::from_vision_root(root.path());

    let provider = ocr
        .provider()
        .expect("an OCR model folder makes OCR available");
    assert!(ocr.unavailable_outcome().is_none());
    assert_eq!(
        ocr.vision_root(),
        Some(fs::canonicalize(root.path()).unwrap().as_path())
    );
    // The provider serves the listed folders by name, as the Runtime's does: the listed OCR
    // model is admitted without reading it, an unlisted one is refused.
    provider
        .require_ocr_model("neutral-ocr", MODEL_SHA256)
        .expect("the listed OCR model");
    assert_eq!(
        provider
            .require_ocr_model("other-ocr", MODEL_SHA256)
            .expect_err("an OCR model the root does not hold")
            .code(),
        VisionProviderErrorCode::ModelMismatch
    );
}

#[test]
fn lab3_without_a_usable_ocr_model_ocr_is_unavailable_with_its_cause() {
    let missing = TempDir::new().unwrap();
    let absent_root = missing.path().join("absent");
    let ocr = LabOcr::from_vision_root(&absent_root);
    assert!(cause(&ocr).is(VisionListingCode::VisionRootUnavailable));
    assert_eq!(
        ocr.unavailable_outcome().unwrap().value("path"),
        Some(&path_text(&absent_root))
    );

    let no_models = TempDir::new().unwrap();
    assert!(
        cause(&LabOcr::from_vision_root(no_models.path()))
            .is(VisionListingCode::VisionRootUnavailable)
    );

    let empty = vision_root(true, &[], &[]);
    assert!(
        cause(&LabOcr::from_vision_root(empty.path())).is(VisionListingCode::VisionModelsEmpty)
    );

    let nn_only = vision_root(true, &[], &["neutral-nn"]);
    let model_missing = cause(&LabOcr::from_vision_root(nn_only.path()));
    assert!(model_missing.is(LabCode::LabOcrModelMissing));
    let models = fs::canonicalize(nn_only.path()).unwrap().join("models");
    assert_eq!(model_missing.value("path"), Some(&path_text(&models)));

    let no_runtime = vision_root(false, &["neutral-ocr"], &[]);
    let runtime_missing = cause(&LabOcr::from_vision_root(no_runtime.path()));
    assert!(runtime_missing.is(LabCode::LabOcrRuntimeMissing));
    let library = fs::canonicalize(no_runtime.path())
        .unwrap()
        .join("ort")
        .join("onnxruntime.dll");
    assert_eq!(runtime_missing.value("path"), Some(&path_text(&library)));
}

#[test]
fn lab3_the_runtime_configuration_vision_section_names_the_vision_root() {
    let install = TempDir::new().unwrap();
    let config_dir = install.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();
    let canonical = |path: &Path| fs::canonicalize(path).unwrap();

    // A relative root resolves against the configuration's folder; the configured CUDA
    // execution provider does not matter, the offline tools run on the CPU.
    let relative = config_dir.join("alternate");
    fs::create_dir_all(&relative).unwrap();
    let models = vision_root(true, &["neutral-ocr"], &[]);
    copy_tree(models.path(), &relative);
    let configured = LabOcr::from_runtime_configuration(
        br#"{"vision": {"execution_provider": "cuda", "root": "alternate"}}"#,
        &config_dir,
        Some(install.path()),
    );
    assert!(configured.provider().is_some());
    assert_eq!(
        configured.vision_root(),
        Some(canonical(&relative).as_path())
    );

    // Without a root the vision root is `vision` beside the install root.
    let default_root = install.path().join("vision");
    copy_tree(models.path(), &default_root);
    let default = LabOcr::from_runtime_configuration(
        br#"{"vision": {"execution_provider": "cpu"}}"#,
        &config_dir,
        Some(install.path()),
    );
    assert!(default.provider().is_some());
    assert_eq!(
        default.vision_root(),
        Some(canonical(&default_root).as_path())
    );

    // No vision section, no installation, or a configuration that does not decode: OCR is
    // unavailable with the reason.
    let unconfigured = LabOcr::from_runtime_configuration(
        br#"{"instances": []}"#,
        &config_dir,
        Some(install.path()),
    );
    assert!(cause(&unconfigured).is(LabCode::LabOcrVisionUnconfigured));
    assert!(unconfigured.vision_root().is_none());
    assert!(cause(&LabOcr::from_installation(None)).is(LabCode::LabOcrVisionUnconfigured));
    let malformed = LabOcr::from_runtime_configuration(b"{", &config_dir, Some(install.path()));
    assert!(cause(&malformed).is(SharedCode::ConfigDecodeFailed));
}

// Copies the files of `from` into `to`, keeping the folder layout.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn pack() -> RecognitionPack {
    load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
            "targets": [
                {"type": "color", "id": "label/marker", "region": {"x": 0, "y": 0, "width": 4, "height": 4},
                 "expected": [255, 0, 0]},
                {"type": "ocr", "id": "label/title", "region": {"x": 8, "y": 8, "width": 32, "height": 16},
                 "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["Start"],
                 "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "neutral-ocr",
                 "model_sha256": MODEL_SHA256},
                {"type": "composite", "id": "label/both", "mode": "all_of",
                 "members": ["label/marker", "label/title"]}
            ]
        })
        .to_string(),
    )
    .expect("the neutral pack")
}

fn detector(required: &str) -> PageDetector {
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["label/marker"], "optional": [], "forbidden": []},
            {"id": "neutral/title", "required": [required], "optional": [], "forbidden": []}
        ]
    });
    PageDetector::new(load_page_set_from_json_str(&pages.to_string()).expect("pages"))
        .expect("detector")
}

#[test]
fn lab3_an_ocr_target_without_a_model_is_refused_with_its_code_never_skipped() {
    let pack = pack();
    let unavailable = LabOcr::from_installation(None);

    unavailable
        .require(&pack, ["label/marker"])
        .expect("a target without OCR needs no model");
    for (requested, reached) in [
        ("label/title", "label/title"),
        ("label/both", "label/title"),
    ] {
        let error = unavailable
            .require(&pack, [requested])
            .expect_err("an OCR target needs the OCR model");
        assert_eq!(error.code, "lab_ocr_unavailable", "{requested}");
        assert_eq!(error.class, LabErrorClass::SafetyBlocked);
        assert_eq!(error.blocked_by, vec!["ocr_model".to_string()]);
        let outcome = &error.details.as_ref().expect("the outcome")["outcome"];
        assert_eq!(outcome["code"], "lab_ocr_unavailable");
        assert_eq!(outcome["category"], "error");
        assert_eq!(outcome["values"]["target_id"], reached, "{requested}");
        assert_eq!(outcome["causes"][0]["code"], "lab_ocr_vision_unconfigured");
    }
    let error = unavailable
        .require_pages(&pack, &detector("label/both"))
        .expect_err("page detection evaluates every page target");
    assert_eq!(error.code, "lab_ocr_unavailable");
    unavailable
        .require_pages(&pack, &detector("label/marker"))
        .expect("a page set without OCR needs no model");

    let available = LabOcr::with_provider(Arc::new(ScriptedOcr));
    available
        .require(&pack, ["label/title", "label/both"])
        .expect("OCR is available");
    available
        .require_pages(&pack, &detector("label/both"))
        .expect("OCR is available");
}

fn frame(red: bool) -> LoadedFrame {
    let pixel = if red { [255, 0, 0] } else { [0, 0, 255] };
    let pixels = pixel.repeat((FRAME_WIDTH * FRAME_HEIGHT) as usize);
    LoadedFrame {
        png: Vec::new(),
        sha256: String::new(),
        frame: Frame::from_pixels(
            FRAME_WIDTH,
            FRAME_HEIGHT,
            pixels.clone(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .expect("frame"),
        scene: Scene::from_rgb8(FRAME_WIDTH, FRAME_HEIGHT, &pixels).expect("scene"),
    }
}

fn ocr_mark() -> RecordedMark {
    serde_json::from_value(json!({
        "id": "label/title",
        "family": "ocr",
        "step": 1,
        "region": {"x": 8, "y": 8, "width": 32, "height": 16},
        "ocr": {
            "languages": ["en"],
            "timeout_ms": 1000,
            "match_mode": "exact",
            "expected": ["Start"],
            "case_sensitive": false,
            "minimum_confidence": 0.8,
            "model_ref": "neutral-ocr",
            "model_sha256": MODEL_SHA256
        },
        "self_test": {"status": "not_evaluated", "frames": 0, "single_sample": false},
        "created_at_unix_ms": 1
    }))
    .expect("an OCR mark")
}

#[test]
fn lab3_the_mark_self_test_evaluates_an_ocr_mark_on_every_frame() {
    let mark = ocr_mark();
    let defaults = default_recording_defaults();
    let (start, other) = (frame(true), frame(false));
    let available = LabOcr::with_provider(Arc::new(ScriptedOcr));

    let both = [
        EvalFrame {
            id: "f0001",
            frame: &start,
        },
        EvalFrame {
            id: "f0002",
            frame: &start,
        },
    ];
    let (test, failure) =
        evaluate_mark(&mark, None, &both, &defaults, &available).expect("self-test");
    assert_eq!(test.status, SelfTestStatus::Passed);
    assert_eq!(test.frames, 2);
    assert!(failure.is_none());

    let one_fails = [
        EvalFrame {
            id: "f0001",
            frame: &start,
        },
        EvalFrame {
            id: "f0002",
            frame: &other,
        },
    ];
    let (test, failure) =
        evaluate_mark(&mark, None, &one_fails, &defaults, &available).expect("self-test");
    assert_eq!(test.status, SelfTestStatus::Failed);
    assert_eq!(test.reason.as_deref(), Some("self_test_failed"));
    let failure = failure.expect("the failing frame");
    assert_eq!(failure["frame_id"], "f0002");
    assert_eq!(failure["text"], "Stop");

    // Without an OCR model the mark is not evaluated under the registered reason.
    let unavailable = LabOcr::from_installation(None);
    let (test, failure) =
        evaluate_mark(&mark, None, &both, &defaults, &unavailable).expect("self-test");
    assert_eq!(test.status, SelfTestStatus::NotEvaluated);
    assert_eq!(test.reason.as_deref(), Some("lab_ocr_unavailable"));
    assert!(failure.is_none());
}
