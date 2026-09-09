// SPDX-License-Identifier: AGPL-3.0-only

fn neutral_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "click":{"kind":"point","x":1,"y":0},
                 "unguarded_trusted_coordinate":true,
                 "retryable":false
             }]
        }"#,
    )
}

fn neutral_non_home_start_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "click":{"kind":"point","x":1,"y":0},
                 "unguarded_trusted_coordinate":true,
                 "retryable":false
             }]
        }"#,
    )
}

fn neutral_stability_contained_task_package(
    consecutive_unchanged_threshold: u32,
    max_steps: u32,
) -> Vec<u8> {
    let declaration = serde_json::json!({
        "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
        "consecutive_unchanged_threshold": consecutive_unchanged_threshold,
        "max_steps": max_steps
    });
    let control = serde_json::to_vec(&serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.semantic.stability-task",
        "execution_mode": "in_page_guard",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5_000,
        "max_steps": max_steps,
        "stability_termination": declaration.clone()
    }))
    .expect("stability control JSON");
    let task = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "stability_termination": declaration,
        "operations": [{
            "id": "repeat",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    }))
    .expect("stability task JSON");
    let recognition = br#"{
        "schema_version":"0.3",
        "game":"neutral",
        "server":"test",
        "coordinate_space":{"width":2,"height":1},
        "defaults":{"color_max_distance":0.0},
        "targets":[
            {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
            {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
            {"type":"color","id":"page/error","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,255,0]}
        ]
    }"#;
    let pages = br#"{
        "schema_version":"0.3",
        "pages":[
            {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
            {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]},
            {"id":"neutral/error","required":["page/error"],"optional":[],"forbidden":[]}
        ]
    }"#;

    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: [(&str, &[u8]); 5] = [
        ("control.json", &control),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        ("resources/operations/task/task.json", &task),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    for (path, contents) in files {
        zip.start_file(path, options).expect("stability zip entry");
        zip.write_all(contents).expect("stability zip content");
    }
    zip.finish().expect("finish stability zip").into_inner()
}

fn neutral_post_admission_ocr_contained_task_package() -> Vec<u8> {
    let stability = serde_json::json!({
        "region": {"x": 1, "y": 0, "width": 1, "height": 1},
        "comparison": {"mode": "exact_pixels_v1", "parameters": {}},
        "consecutive_unchanged_threshold": 2,
        "max_steps": 4
    });
    let truth = serde_json::to_vec(&serde_json::json!({
        "schema_version": "actingcommand.ocr-truth-set.v1",
        "items": ["synthetic truth"]
    }))
    .expect("OCR truth JSON");
    let truth_sha256 = format!("{:x}", Sha256::digest(&truth));
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.3",
        "entry_task_id": "task",
        "files": [{
            "path": "operations/task/truth.json",
            "sha256": truth_sha256.clone()
        }]
    }))
    .expect("OCR manifest JSON");
    let control = serde_json::to_vec(&serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.semantic.post-admission-ocr-task",
        "execution_mode": "in_page_guard",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5_000,
        "max_steps": 4,
        "stability_termination": stability.clone()
    }))
    .expect("OCR control JSON");
    let task = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.7",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "scheduling_outcome": {
            "mappings": [{
                "outcome_key": "comparison_recorded",
                "effect": "no_designated_effect",
                "terminal_pages": ["home"]
            }]
        },
        "post_admission_ocr": {
            "page_id": "home",
            "target_id": "fixture/ocr",
            "truth_set": {"path": "truth.json", "sha256": truth_sha256},
            "normalization": "trim_lowercase_v1",
            "comparison": "exact_set_v1",
            "limits": {
                "max_frames": 2,
                "max_items": 16,
                "max_string_bytes": 64,
                "max_total_bytes": 4096,
                "max_truth_entries": 16
            },
            "outcome_key": "comparison_recorded"
        },
        "stability_termination": stability,
        "operations": [{
            "id": "repeat",
            "from": "home",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    }))
    .expect("OCR task JSON");
    let recognition = br#"{
        "schema_version":"0.6",
        "game":"neutral",
        "server":"test",
        "coordinate_space":{"width":2,"height":1},
        "defaults":{"color_max_distance":0.0},
        "targets":[
            {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
            {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
            {"type":"color","id":"page/error","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,255,0]},
            {
                "type":"ocr",
                "id":"fixture/ocr",
                "region":{"x":0,"y":0,"width":1,"height":1},
                "languages":["en"],
                "timeout_ms":1000,
                "match_mode":"exact",
                "expected":["unused"],
                "case_sensitive":true,
                "minimum_confidence":0.0,
                "model_ref":"PP-OCRv6_medium",
                "model_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        ]
    }"#;
    let pages = br#"{
        "schema_version":"0.3",
        "pages":[
            {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
            {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]},
            {"id":"neutral/error","required":["page/error"],"optional":[],"forbidden":[]}
        ]
    }"#;

    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: [(&str, &[u8]); 6] = [
        ("control.json", &control),
        ("resources/manifest.json", &manifest),
        ("resources/operations/task/task.json", &task),
        ("resources/operations/task/truth.json", &truth),
        ("resources/recognition/neutral.test.pack.json", recognition),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    for (path, contents) in files {
        zip.start_file(path, options).expect("OCR zip entry");
        zip.write_all(contents).expect("OCR zip content");
    }
    zip.finish().expect("finish OCR zip").into_inner()
}

fn expected_contained_task_sampling_seed<T: serde::Serialize>(value: &T) -> u64 {
    let bytes = serde_json::to_vec(value).expect("sampling seed oracle input");
    let digest = Sha256::digest(bytes);
    let mut seed = [0_u8; 8];
    seed.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(seed)
}

fn neutral_region_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "click":{"kind":"rect","x":0,"y":0,"width":2,"height":1},
                 "unguarded_trusted_coordinate":true,
                 "retryable":false
             }]
        }"#,
    )
}

fn neutral_contained_task_package_with_execution_timeout(timeout_ms: u64) -> Vec<u8> {
    neutral_contained_task_package_with_task_and_timeout(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "click":{"kind":"point","x":1,"y":0},
                 "unguarded_trusted_coordinate":true,
                 "retryable":false
             }]
        }"#,
        timeout_ms,
    )
}

fn neutral_vision_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task_and_recognition(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "click":{"kind":"point","x":1,"y":0},
                 "unguarded_trusted_coordinate":true,
                 "retryable":false
             }]
        }"#,
        br#"{
            "schema_version":"0.6",
            "game":"neutral",
            "server":"test",
            "coordinate_space":{"width":2,"height":1},
            "targets":[
                {
                    "type":"ocr",
                    "id":"page/home",
                    "region":{"x":0,"y":0,"width":1,"height":1},
                    "languages":["en"],
                    "timeout_ms":1000,
                    "match_mode":"exact",
                    "expected":["home"],
                    "case_sensitive":true,
                    "minimum_confidence":0.9,
                    "model_ref":"PP-OCRv6_medium",
                    "model_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                {
                    "type":"ocr",
                    "id":"page/terminal",
                    "region":{"x":0,"y":0,"width":1,"height":1},
                    "languages":["en"],
                    "timeout_ms":1000,
                    "match_mode":"exact",
                    "expected":["terminal"],
                    "case_sensitive":true,
                    "minimum_confidence":0.9,
                    "model_ref":"PP-OCRv6_medium",
                    "model_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                {
                    "type":"ocr",
                    "id":"page/error",
                    "region":{"x":0,"y":0,"width":1,"height":1},
                    "languages":["en"],
                    "timeout_ms":1000,
                    "match_mode":"exact",
                    "expected":["error"],
                    "case_sensitive":true,
                    "minimum_confidence":0.9,
                    "model_ref":"PP-OCRv6_medium",
                    "model_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                {
                    "type":"color",
                    "id":"guard/ready",
                    "region":{"x":1,"y":0,"width":1,"height":1},
                    "expected":[0,255,0]
                }
            ]
        }"#,
    )
}

fn neutral_retrying_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "to":"terminal",
                 "click":{"kind":"point","x":1,"y":0},
                 "guard":{
                     "page_id":"home",
                     "target_id":"guard/ready",
                     "expected_rect":{"x":1,"y":0,"width":1,"height":1},
                     "color_probe":"guard/ready"
                 },
                 "retryable":true,
                 "max_attempts":6,
                 "retry_interval_ms":1
             }]
        }"#,
    )
}

fn neutral_region_retrying_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "to":"terminal",
                 "click":{"kind":"rect","x":0,"y":0,"width":2,"height":1},
                 "guard":{
                     "page_id":"home",
                     "target_id":"guard/ready",
                     "expected_rect":{"x":1,"y":0,"width":1,"height":1},
                     "color_probe":"guard/ready"
                 },
                 "retryable":true,
                 "max_attempts":6,
                 "retry_interval_ms":1
             }]
        }"#,
    )
}

fn neutral_error_page_retrying_contained_task_package() -> Vec<u8> {
    neutral_contained_task_package_with_task(
        br#"{
            "schema_version":"0.6",
            "task_id":"task",
            "game":"neutral",
            "server_scope":["test"],
            "coordinate_space":{"width":2,"height":1},
            "entry_page":"home",
            "target_page":"terminal",
            "error_pages":["error"],
             "operations":[{
                 "id":"open_terminal",
                 "from":"home",
                 "to":"terminal",
                 "click":{"kind":"point","x":1,"y":0},
                 "guard":{
                     "page_id":"home",
                     "target_id":"guard/ready",
                     "expected_rect":{"x":1,"y":0,"width":1,"height":1},
                     "color_probe":"guard/ready"
                 },
                 "retryable":true,
                 "max_attempts":6,
                 "retry_interval_ms":1
             }]
        }"#,
    )
}

fn neutral_mapped_contained_task_package(outcome_key: &str, effect: &str) -> Vec<u8> {
    neutral_mapped_contained_task_package_with_pages(outcome_key, effect, &["terminal"], &[])
}

fn neutral_mapped_retrying_contained_task_package(outcome_key: &str) -> Vec<u8> {
    let no_effect_outcome_key = complementary_outcome_key(outcome_key);
    let task = serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": outcome_key,
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": no_effect_outcome_key,
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        },
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": true,
            "max_attempts": 6,
            "retry_interval_ms": 1
        }]
    });
    neutral_contained_task_package_with_task(
        &serde_json::to_vec(&task).expect("mapped retrying contained task JSON"),
    )
}

fn neutral_two_key_mapped_contained_task_package(
    effect_outcome_key: &str,
    no_effect_outcome_key: &str,
) -> Vec<u8> {
    let task = serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": effect_outcome_key,
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": no_effect_outcome_key,
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        },
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }]
    });
    neutral_contained_task_package_with_task(
        &serde_json::to_vec(&task).expect("two-key mapped contained task JSON"),
    )
}

fn neutral_mapped_contained_task_package_with_pages(
    outcome_key: &str,
    effect: &str,
    terminal_pages: &[&str],
    error_pages: &[&str],
) -> Vec<u8> {
    let complementary_key = complementary_outcome_key(outcome_key);
    let complementary_effect = if effect == "designated_effect_completed" {
        "no_designated_effect"
    } else {
        "designated_effect_completed"
    };
    let mut task = serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": outcome_key,
                    "effect": effect,
                    "terminal_pages": terminal_pages
                },
                {
                    "outcome_key": complementary_key,
                    "effect": complementary_effect,
                    "terminal_pages": ["terminal"]
                }
            ]
        },
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }]
    });
    if !error_pages.is_empty() {
        task["error_pages"] = serde_json::json!(error_pages);
    }
    neutral_contained_task_package_with_task(
        &serde_json::to_vec(&task).expect("mapped contained task JSON"),
    )
}

fn neutral_non_retryable_destination_package(with_error_page: bool) -> Vec<u8> {
    let mut task = serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }]
    });
    if with_error_page {
        task["error_pages"] = serde_json::json!(["error"]);
    }
    neutral_contained_task_package_with_task(
        &serde_json::to_vec(&task).expect("non-retryable destination task JSON"),
    )
}

fn neutral_contained_task_package_with_task(task: &[u8]) -> Vec<u8> {
    neutral_contained_task_package_with_task_and_timeout(task, 5_000)
}

fn explicit_home_contained_task_package(
    package_id: &str,
    home_color: [u8; 3],
    other_color: [u8; 3],
) -> Vec<u8> {
    let task = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "fixture01",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "home",
        "operations": [{
            "id": "return_home",
            "from": "other",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    }))
    .expect("explicit Home task JSON");
    let recognition = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.3",
        "game": "fixture01",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": home_color},
            {"type": "color", "id": "page/other", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": other_color},
            {"type": "color", "id": "home/green_anchor", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            {"type": "color", "id": "home/alternate_anchor", "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [255, 255, 255]}
        ]
    }))
    .expect("explicit Home recognition JSON");
    let pages = serde_json::to_vec(&serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {
                "id": "fixture01/home",
                "required": ["page/home"],
                "any_of": [["home/green_anchor", "home/alternate_anchor"]],
                "optional": [],
                "forbidden": []
            },
            {
                "id": "fixture01/other",
                "required": ["page/other"],
                "any_of": [],
                "optional": [],
                "forbidden": []
            }
        ]
    }))
    .expect("explicit Home pages JSON");
    let control = serde_json::to_vec(&serde_json::json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "fixture01",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 500,
        "timeout_ms": 5000,
        "max_steps": 2
    }))
    .expect("explicit Home control JSON");
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in [
        ("control.json", control.as_slice()),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#.as_slice(),
        ),
        ("resources/operations/task/task.json", task.as_slice()),
        (
            "resources/recognition/fixture01.test.pack.json",
            recognition.as_slice(),
        ),
        (
            "resources/recognition/fixture01.test.pages.json",
            pages.as_slice(),
        ),
    ] {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn neutral_contained_task_package_with_task_and_timeout(task: &[u8], timeout_ms: u64) -> Vec<u8> {
    neutral_contained_task_package_with_timeout(
        task,
        br#"{
            "schema_version":"0.3",
            "game":"neutral",
            "server":"test",
            "coordinate_space":{"width":2,"height":1},
            "defaults":{"color_max_distance":0.0},
            "targets":[
                {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]},
                {"type":"color","id":"page/error","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,255,0]},
                {"type":"color","id":"guard/ready","region":{"x":1,"y":0,"width":1,"height":1},"expected":[0,255,0]}
            ]
        }"#,
        timeout_ms,
    )
}

fn neutral_contained_task_package_with_task_and_recognition(
    task: &[u8],
    recognition_pack: &[u8],
) -> Vec<u8> {
    neutral_contained_task_package_with_timeout(task, recognition_pack, 5_000)
}

fn neutral_contained_task_package_with_timeout(
    task: &[u8],
    recognition_pack: &[u8],
    timeout_ms: u64,
) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let control = format!(
        r#"{{
            "schema_version":"Lab-1y.control.v1",
            "package_id":"neutral.semantic.task",
            "execution_mode":"navigable_route",
            "game":"neutral",
            "server":"test",
            "resolution":{{"width":2,"height":1}},
            "entry_task_id":"task",
            "capture_interval_ms":1,
            "step_timeout_ms":50,
            "timeout_ms":{timeout_ms},
            "max_steps":2
        }}"#
    );
    let files: &[(&str, &[u8])] = &[
        ("control.json", control.as_bytes()),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        (
            "resources/operations/task/task.json",
            task,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            recognition_pack,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                    {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]},
                    {"id":"neutral/error","required":["page/error"],"optional":[],"forbidden":[]}
                ]
            }"#,
        ),
    ];
    for (path, contents) in files {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(contents).expect("zip content");
    }
    zip.finish().expect("finish zip").into_inner()
}
