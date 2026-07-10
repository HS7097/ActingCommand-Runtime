// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ports::DisabledLedger;
use crate::{CaptureBackendFactory, Clock, ConfigSource, InputBackendFactory, LabPorts};
use actingcommand_recognition::ScenePixelFormat;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

struct DisabledInputFactory;

impl InputBackendFactory for DisabledInputFactory {
    fn open(&self, _request: crate::InputBackendRequest) -> EnvResult<Box<dyn InputBackend>> {
        Err(LabError::device("input must not be opened in this test"))
    }
}

struct DisabledCaptureFactory;

impl CaptureBackendFactory for DisabledCaptureFactory {
    fn open(
        &self,
        _request: crate::CaptureBackendRequest,
    ) -> EnvResult<Box<dyn actingcommand_device::CaptureBackend>> {
        Err(LabError::device("capture must not be opened in this test"))
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now_unix_ms(&self) -> EnvResult<u64> {
        Ok(1_750_000_000_000)
    }

    fn sleep(&self, _duration: Duration) {}
}

struct DisabledConfig;

impl ConfigSource for DisabledConfig {
    fn load(&self) -> EnvResult<crate::UserConfig> {
        Err(LabError::device("config must not be loaded in this test"))
    }

    fn state_root(&self) -> EnvResult<PathBuf> {
        Err(LabError::device("config must not be loaded in this test"))
    }
}

struct TestPorts {
    input: DisabledInputFactory,
    capture: DisabledCaptureFactory,
    ledger: DisabledLedger,
    clock: FixedClock,
    config: DisabledConfig,
}

impl LabPorts for TestPorts {
    type InputFactory = DisabledInputFactory;
    type CaptureFactory = DisabledCaptureFactory;
    type Ledger = DisabledLedger;
    type Time = FixedClock;
    type Config = DisabledConfig;

    fn input_factory(&self) -> &Self::InputFactory {
        &self.input
    }

    fn capture_factory(&self) -> &Self::CaptureFactory {
        &self.capture
    }

    fn ledger(&mut self) -> &mut Self::Ledger {
        &mut self.ledger
    }

    fn clock(&self) -> &Self::Time {
        &self.clock
    }

    fn config(&self) -> &Self::Config {
        &self.config
    }
}

fn test_lab(root: &Path) -> Lab<TestPorts> {
    Lab::new(
        TestPorts {
            input: DisabledInputFactory,
            capture: DisabledCaptureFactory,
            ledger: DisabledLedger,
            clock: FixedClock,
            config: DisabledConfig,
        },
        crate::LabState::open(root).unwrap(),
    )
    .unwrap()
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[test]
fn instance_id_is_stable_safe_and_desensitized() {
    let first = env_instance_id("127.0.0.1:16416", "salt").unwrap();
    let second = env_instance_id(" 127.0.0.1:16416 ", "salt").unwrap();
    assert_eq!(first, second);
    assert!(first.starts_with(ENV_INSTANCE_ID_PREFIX));
    assert!(!first.contains(':'));
    assert!(!first.contains('/'));
    assert!(!first.contains('\\'));
    assert!(!first.contains("127.0.0.1"));
}

#[test]
fn unsafe_or_unlisted_values_are_rejected() {
    let key = EnvDetectionKey {
        key: "ui_theme".to_string(),
        min_confidence: 0.7,
        stale_below_confidence: None,
        ttl_ms: None,
        allowed_values: vec!["Default".to_string()],
        candidates: Vec::new(),
    };
    for value in ["", "../bad", "bad/name", "bad\\name", "C:bad", "bad..name"] {
        let candidate = candidate(value);
        assert!(validate_env_value(&candidate, &key).is_err());
    }
    let other_candidate = candidate("Other");
    assert!(validate_env_value(&other_candidate, &key).is_err());
    let default_candidate = candidate("Default");
    assert!(validate_env_value(&default_candidate, &key).is_ok());
}

#[test]
fn result_path_uses_instance_id_not_raw_endpoint() {
    let path = env_result_path(Path::new("ours/env-detection"), "envinst_abc");
    let text = path.display().to_string();
    assert!(text.contains("envinst_abc"));
    assert!(!text.contains("127.0.0.1"));
    assert_eq!(
        path.file_name().and_then(|value| value.to_str()),
        Some("result.json")
    );
}

#[test]
fn flat_resource_authored_catalog_is_normalized() {
    let temp = TempDir::new().unwrap();
    let env_dir = temp.path().join(ENV_DETECTION_DIR);
    fs::create_dir_all(&env_dir).unwrap();
    fs::write(
        env_dir.join(ENV_DETECTION_CATALOG),
        r#"{
                "schema_version": "env-detections.v1",
                "game": "arknights",
                "detections": [{
                    "detector_id": "detect_ui_theme",
                    "detector_version": "1",
                    "key": "ui_theme",
                    "method": "any_of",
                    "threshold": 0.7,
                    "invalidate_below_confidence": 0.6,
                    "ttl_ms": null,
                    "allowed_values": ["Default"],
                    "candidates": [{
                        "value": "Default",
                        "template": "hometheme/Default/Terminal.png",
                        "roi": [844, 58, 268, 272]
                    }]
                }]
            }"#,
    )
    .unwrap();

    let catalog = load_env_catalog(&env_dir).unwrap();
    let detector = catalog.detector("detect_ui_theme").unwrap();
    assert_eq!(detector.game_id.as_deref(), Some("arknights"));
    assert_eq!(detector.version(), "1");
    assert_eq!(detector.keys.len(), 1);
    let key = &detector.keys[0];
    assert_eq!(key.key, "ui_theme");
    assert_eq!(key.min_confidence, 0.7);
    assert_eq!(key.stale_below_confidence, Some(0.6));
    let candidate = &key.candidates[0];
    assert_eq!(
        candidate.template_path.as_deref(),
        Some("hometheme/Default/Terminal.png")
    );
    assert_eq!(
        candidate.region,
        Some(EnvRect {
            x: 844,
            y: 58,
            width: 268,
            height: 272
        })
    );
}

#[test]
fn interactive_steps_are_data_defined_and_validated() {
    let mut detector = detector();
    detector.steps = vec![
        EnvDetectionStep {
            kind: "tap".to_string(),
            x: Some(100),
            y: Some(200),
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: None,
        },
        EnvDetectionStep {
            kind: "long-tap".to_string(),
            x: Some(110),
            y: Some(210),
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: Some(500),
        },
        EnvDetectionStep {
            kind: "swipe".to_string(),
            x: None,
            y: None,
            x1: Some(10),
            y1: Some(20),
            x2: Some(30),
            y2: Some(40),
            duration_ms: Some(300),
        },
        EnvDetectionStep {
            kind: "wait".to_string(),
            x: None,
            y: None,
            x1: None,
            y1: None,
            x2: None,
            y2: None,
            duration_ms: Some(1),
        },
    ];
    for (index, step) in detector.steps.iter().enumerate() {
        validate_detection_step(&detector, index, step).unwrap();
    }
    let plan = serde_json::to_value(detector.steps[1].to_plan().unwrap()).unwrap();
    assert_eq!(plan["type"], "long_tap");
}

#[test]
fn invalid_interactive_steps_fail_loud() {
    let detector = detector();
    let missing_coordinate = EnvDetectionStep {
        kind: "tap".to_string(),
        x: Some(100),
        y: None,
        x1: None,
        y1: None,
        x2: None,
        y2: None,
        duration_ms: None,
    };
    let err = validate_detection_step(&detector, 0, &missing_coordinate)
        .expect_err("missing coordinate rejected");
    assert!(err.message.contains("missing coordinate y"));

    let invalid_duration = EnvDetectionStep {
        kind: "wait".to_string(),
        x: None,
        y: None,
        x1: None,
        y1: None,
        x2: None,
        y2: None,
        duration_ms: Some(0),
    };
    let err = validate_detection_step(&detector, 1, &invalid_duration)
        .expect_err("zero duration rejected");
    assert!(err.message.contains("duration_ms"));
}

#[test]
fn dry_run_detection_steps_plan_without_device_work() {
    let mut detector = detector();
    detector.steps = vec![EnvDetectionStep {
        kind: "tap".to_string(),
        x: Some(100),
        y: Some(200),
        x1: None,
        y1: None,
        x2: None,
        y2: None,
        duration_ms: None,
    }];
    let temp = TempDir::new().unwrap();
    let mut lab = test_lab(temp.path());
    let request = crate::EnvDetectRequest {
        scope: crate::EnvScopeRequest {
            resource_root: temp.path().to_path_buf(),
            state_root: temp.path().to_path_buf(),
            instance: "fixture".to_string(),
            game: "arknights".to_string(),
            server: Some("cn".to_string()),
        },
        task: detector.id.clone(),
        scene_path: None,
        capture_config: None,
        touch_config: None,
        require_fresh: false,
        fresh_delay: Duration::from_millis(160),
        dry_run: true,
    };
    let run = run_detection_steps(&mut lab, &request, &detector).unwrap();
    assert!(run.planned_only);
    assert!(!run.executed);
    let step = serde_json::to_value(&run.steps[0]).unwrap();
    assert_eq!(step["step"]["type"], "tap");
}

#[test]
fn stale_resource_hash_blocks_resolution() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result = result(&context, &detector, "old-hash", "Default", 0.95, None);
    let err = ensure_result_fresh(&result, &detector, &context, "new-hash", current_unix_ms())
        .expect_err("stale hash rejected");
    assert!(err.message.contains("resource hash changed"));
}

#[test]
fn low_confidence_blocks_resolution() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result = result(&context, &detector, "hash", "Default", 0.60, None);
    let err = ensure_result_fresh(&result, &detector, &context, "hash", current_unix_ms())
        .expect_err("low confidence rejected");
    assert!(err.message.contains("confidence"));
}

#[test]
fn stored_unlisted_env_value_blocks_resolution() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result = result(&context, &detector, "hash", "Other", 0.95, None);
    let err = ensure_result_fresh(&result, &detector, &context, "hash", current_unix_ms())
        .expect_err("unlisted result value rejected");

    assert!(err.message.contains("not in allowed_values"));
    assert_eq!(env_stale_reason(&err), "unallowed_value");
}

#[test]
fn stored_unsafe_env_value_blocks_resolution() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result = result(&context, &detector, "hash", "../Default", 0.95, None);
    let err = ensure_result_fresh(&result, &detector, &context, "hash", current_unix_ms())
        .expect_err("unsafe result value rejected");

    assert!(err.message.contains("unsafe value"));
    assert_eq!(env_stale_reason(&err), "unsafe_value");
}

#[test]
fn stale_result_payload_reports_machine_readable_needs_detection() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result = result(&context, &detector, "old-hash", "Default", 0.95, None);
    let err = ensure_result_fresh(&result, &detector, &context, "new-hash", current_unix_ms())
        .expect_err("stale hash rejected");
    let result_path = env_result_path(&context.env_dir, &context.instance_id);

    let payload = serde_json::to_value(needs_detection_payload(
        &detector,
        &context,
        &result_path,
        Some(&result),
        env_stale_reason(&err),
        Some(&err.message),
    ))
    .unwrap();

    assert_eq!(payload["status"], "needs_detection");
    assert_eq!(payload["reason"], "resource_hash_changed");
    assert_eq!(payload["detector_id"], "detect_ui_theme");
    assert_eq!(payload["instance_id"], "envinst_a");
    assert_eq!(payload["recommended_action"], "run_detect");
    assert_eq!(payload["detections"][0]["key"], "ui_theme");
    assert_eq!(payload["detections"][0]["value"], "Default");
    assert_eq!(payload["error"], err.message);
}

#[test]
fn missing_result_payload_reports_run_detect_without_fake_detections() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result_path = env_result_path(&context.env_dir, &context.instance_id);

    let payload = serde_json::to_value(needs_detection_payload(
        &detector,
        &context,
        &result_path,
        None,
        "missing_result",
        None,
    ))
    .unwrap();

    assert_eq!(payload["status"], "needs_detection");
    assert_eq!(payload["reason"], "missing_result");
    assert_eq!(payload["recommended_action"], "run_detect");
    assert!(payload["detections"].as_array().unwrap().is_empty());
    assert!(payload.get("source_result").is_none());
    assert!(payload.get("error").is_none());
}

#[test]
fn stale_reason_classifies_common_result_failures() {
    let cases = [
        (
            "env detection result schema 'old' is stale; expected 'new'",
            "schema_mismatch",
        ),
        (
            "env detection result belongs to a different instance_id",
            "instance_mismatch",
        ),
        (
            "env detection result scope is stale: result arknights.cn command arknights.jp",
            "scope_mismatch",
        ),
        (
            "env detection result detector is stale: result a@1 command a@2",
            "detector_mismatch",
        ),
        (
            "env detection result is stale because detector resource hash changed",
            "resource_hash_changed",
        ),
        (
            "env detection result is missing key 'ui_theme'",
            "missing_key",
        ),
        (
            "env key 'ui_theme' is stale: confidence 0.1 below threshold 0.7",
            "low_confidence",
        ),
        (
            "env key 'ui_theme' expired at 1; run detect first",
            "expired",
        ),
        (
            "env key 'ui_theme' has unsafe value '../bad'",
            "unsafe_value",
        ),
        (
            "env key 'ui_theme' value 'Other' is not in allowed_values",
            "unallowed_value",
        ),
    ];

    for (message, expected) in cases {
        let error = LabError::usage(message);
        assert_eq!(env_stale_reason(&error), expected);
    }
}

#[test]
fn env_pointer_resolution_uses_detected_value() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let result = result(&context, &detector, "hash", "Default", 0.95, None);
    let (resolved, keys) = resolve_env_markers(
        "hometheme/{env:ui_theme}/DepotEnter.png",
        &detector,
        &result,
        current_unix_ms(),
    )
    .unwrap();
    assert_eq!(resolved, "hometheme/Default/DepotEnter.png");
    assert_eq!(keys[0].key, "ui_theme");
    assert_eq!(keys[0].value, "Default");
}

#[test]
fn scene_size_candidate_detects_resolution_without_template_file() {
    let temp = TempDir::new().unwrap();
    let resource_root = temp.path();
    let context = context(resource_root, "envinst_a");
    let detector = resolution_detector();
    let scene =
        Scene::from_pixels(1280, 720, &vec![0; 1280 * 720 * 3], ScenePixelFormat::Rgb8).unwrap();
    let hash = detector_resource_hash(&detector, resource_root).unwrap();
    let result = evaluate_detector(&detector, &context, &scene, &hash, current_unix_ms()).unwrap();

    assert_eq!(result.detections["resolution"].value, "1280x720");
    assert_eq!(result.detections["resolution"].confidence, 1.0);
}

#[test]
fn env_candidate_must_declare_one_matcher() {
    let mut no_matcher = candidate("Default");
    no_matcher.template_path = None;
    assert!(no_matcher.matcher("ui_theme").is_err());

    let mut mixed = candidate("Default");
    mixed.width = Some(1280);
    mixed.height = Some(720);
    assert!(mixed.matcher("ui_theme").is_err());

    let mut partial_size = candidate("Default");
    partial_size.template_path = None;
    partial_size.width = Some(1280);
    assert!(partial_size.matcher("ui_theme").is_err());
}

#[test]
fn lock_conflict_is_visible() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("envinst_a/result.json");
    let first = EnvResultLock::acquire(&path).unwrap();
    let err = EnvResultLock::acquire(&path).expect_err("second lock rejected");
    assert!(err.message.contains("lock conflict"));
    first.release().unwrap();
    EnvResultLock::acquire(&path).unwrap().release().unwrap();
}

#[test]
fn detection_writes_result_and_resolution_reads_it() {
    let temp = TempDir::new().unwrap();
    let resource_root = temp.path();
    fs::create_dir_all(resource_root.join("templates")).unwrap();
    fs::write(
        resource_root.join("templates/default.png"),
        encode_png(1, 1, [255, 0, 0]),
    )
    .unwrap();
    let context = context(resource_root, "envinst_a");
    let detector = detector();
    let scene = Scene::from_pixels(1, 1, &[255, 0, 0], ScenePixelFormat::Rgb8).unwrap();
    let hash = detector_resource_hash(&detector, resource_root).unwrap();
    let result = evaluate_detector(&detector, &context, &scene, &hash, current_unix_ms()).unwrap();
    let path = env_result_path(&context.env_dir, &context.instance_id);
    write_env_result(&path, &result).unwrap();
    let loaded = load_env_result(&path).unwrap();
    ensure_result_fresh(&loaded, &detector, &context, &hash, current_unix_ms()).unwrap();
    assert_eq!(loaded.detections["ui_theme"].value, "Default");
}

#[test]
fn concurrent_result_writes_leave_readable_json() {
    let temp = TempDir::new().unwrap();
    let context = context(temp.path(), "envinst_a");
    let detector = detector();
    let path = env_result_path(&context.env_dir, &context.instance_id);
    let first = result(&context, &detector, "hash", "Default", 0.95, None);
    let mut second = result(&context, &detector, "hash", "Default", 0.96, None);
    second.detections.get_mut("ui_theme").unwrap().source = "concurrent-second".to_string();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let path_a = path.clone();
    let path_b = path.clone();
    let barrier_a = std::sync::Arc::clone(&barrier);
    let barrier_b = std::sync::Arc::clone(&barrier);
    let first_writer = std::thread::spawn(move || {
        barrier_a.wait();
        write_env_result(&path_a, &first)
    });
    let second_writer = std::thread::spawn(move || {
        barrier_b.wait();
        write_env_result(&path_b, &second)
    });

    barrier.wait();
    let outcomes = [first_writer.join().unwrap(), second_writer.join().unwrap()];
    assert!(outcomes.iter().any(Result::is_ok));
    for err in outcomes.iter().filter_map(|outcome| outcome.as_ref().err()) {
        assert!(err.message.contains("lock conflict"));
    }

    let loaded = load_env_result(&path).unwrap();
    ensure_result_fresh(&loaded, &detector, &context, "hash", current_unix_ms()).unwrap();
    assert!(loaded.detections.contains_key("ui_theme"));
    assert!(
        !path.with_extension("json.lock").exists(),
        "env detection lock file should not remain after concurrent writes"
    );
}

fn candidate(value: &str) -> EnvDetectionCandidate {
    EnvDetectionCandidate {
        value: value.to_string(),
        template_path: Some("templates/default.png".to_string()),
        width: None,
        height: None,
        region: None,
        threshold: None,
        source: None,
    }
}

fn size_candidate(value: &str, width: u32, height: u32) -> EnvDetectionCandidate {
    EnvDetectionCandidate {
        value: value.to_string(),
        template_path: None,
        width: Some(width),
        height: Some(height),
        region: None,
        threshold: None,
        source: None,
    }
}

fn detector() -> EnvDetector {
    EnvDetector {
        id: "detect_ui_theme".to_string(),
        version: Some("1".to_string()),
        game_id: Some("arknights".to_string()),
        server_id: Some("cn".to_string()),
        resource_pack_id: Some("test-pack".to_string()),
        match_metric: Some("ccorr_normed".to_string()),
        steps: Vec::new(),
        keys: vec![EnvDetectionKey {
            key: "ui_theme".to_string(),
            min_confidence: 0.7,
            stale_below_confidence: Some(0.7),
            ttl_ms: None,
            allowed_values: vec!["Default".to_string()],
            candidates: vec![candidate("Default")],
        }],
    }
}

fn resolution_detector() -> EnvDetector {
    EnvDetector {
        id: "detect_resolution".to_string(),
        version: Some("1".to_string()),
        game_id: Some("arknights".to_string()),
        server_id: None,
        resource_pack_id: Some("test-pack".to_string()),
        match_metric: None,
        steps: Vec::new(),
        keys: vec![EnvDetectionKey {
            key: "resolution".to_string(),
            min_confidence: 1.0,
            stale_below_confidence: Some(1.0),
            ttl_ms: None,
            allowed_values: vec!["1280x720".to_string(), "1920x1080".to_string()],
            candidates: vec![
                size_candidate("1280x720", 1280, 720),
                size_candidate("1920x1080", 1920, 1080),
            ],
        }],
    }
}

fn context(root: &Path, instance_id: &str) -> EnvCommandContext {
    EnvCommandContext {
        resource_root: root.to_path_buf(),
        env_dir: root.join(ENV_DETECTION_DIR),
        instance_id: instance_id.to_string(),
        game_id: "arknights".to_string(),
        server_id: "cn".to_string(),
    }
}

fn result(
    context: &EnvCommandContext,
    detector: &EnvDetector,
    resource_hash: &str,
    value: &str,
    confidence: f32,
    expires_at_unix_ms: Option<u64>,
) -> EnvDetectionResult {
    let now = current_unix_ms();
    EnvDetectionResult {
        schema_version: ENV_RESULT_SCHEMA_VERSION.to_string(),
        instance_id: context.instance_id.clone(),
        game_id: context.game_id.clone(),
        server_id: context.server_id.clone(),
        detector_id: detector.id.clone(),
        detector_version: detector.version().to_string(),
        resource_pack_id: detector.resource_pack_id(context),
        resource_pack_hash: resource_hash.to_string(),
        generated_at_unix_ms: now,
        detections: BTreeMap::from([(
            "ui_theme".to_string(),
            EnvDetectedValue {
                value: value.to_string(),
                confidence,
                source: "test".to_string(),
                detected_at_unix_ms: now,
                detector_id: detector.id.clone(),
                expires_at_unix_ms,
            },
        )]),
    }
}

fn encode_png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let pixels = vec![color; usize::try_from(width * height).unwrap()];
    encode_rgb_png(width, height, &pixels)
}

fn encode_rgb_png(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<u8> {
    let mut raw = Vec::new();
    for row in 0..height {
        raw.push(0);
        let start = usize::try_from(row * width).unwrap();
        let end = start + usize::try_from(width).unwrap();
        for pixel in &pixels[start..end] {
            raw.extend_from_slice(pixel);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    write_uncompressed_deflate(&mut zlib, &raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(kind.len() + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn write_uncompressed_deflate(out: &mut Vec<u8>, data: &[u8]) {
    for (index, chunk) in data.chunks(65_535).enumerate() {
        let is_last = index == data.len().div_ceil(65_535) - 1;
        out.push(u8::from(is_last));
        let len = u16::try_from(chunk.len()).unwrap();
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
