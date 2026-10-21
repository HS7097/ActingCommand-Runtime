// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A32 (slice E3, test row T3): `recognition_pack_undeclared` judges by "only the
// one declared for that server (variant) counts". Two packs of one server both qualify as
// recognition maintenance packages: with one of them declared `recognition` for that server,
// neither raises the warning; with no declaration for the server, the warning is still raised.
// Each pack is an assembled neutral package that taps from `home` into `terminal` and reads one
// mark on each page, described by the Runtime's own preparation as `package bundle` describes
// the packs it admits. Every expected value comes from the E3 brief (A32).

use crate::package_directory::{admission_deadline, undeclared_recognition_packs};
use actingcommand_contract::{BundleMaintenance, MaintenanceUse};
use actingcommand_execution_kernel::{PreparedContainedTask, TaskPackageDescriptor};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

const SERVER: &str = "test";
const FIRST: &str = "neutral.reading.first";
const SECOND: &str = "neutral.reading.second";
const UNDECLARED: &str = "recognition_pack_undeclared";

fn ocr_target(id: &str, region: Value) -> Value {
    json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
        "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

fn mark(id: &str, page: &str, target: &str) -> Value {
    json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page, "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    })
}

/// Writes the assembled package `package_id` of server `SERVER` at `path`: a guarded tap from
/// `home` to `terminal`, a scheduling outcome without a designated operation, and two marks.
fn write_reader(path: &Path, package_id: &str) {
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": [SERVER],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "read", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]},
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
        }],
        "resource_readings": [
            mark("k1", "home", "count/head"),
            mark("k2", "terminal", "count/page")
        ]
    });
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": SERVER,
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": SERVER,
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            ocr_target("count/head", json!({"x": 1, "y": 0, "width": 1, "height": 1})),
            ocr_target("count/page", json!({"x": 0, "y": 0, "width": 2, "height": 1}))
        ]
    });
    let pages = json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(File::create(path).expect("reader package file"));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in files {
        zip.start_file(name, options).expect("reader package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("reader package JSON"))
            .expect("reader package content");
    }
    zip.finish().expect("finish reader package");
}

/// The two qualifying packs of `SERVER`, described as `package bundle` admits them.
fn readers(temp: &TempDir) -> Vec<TaskPackageDescriptor> {
    [FIRST, SECOND]
        .into_iter()
        .map(|package_id| {
            let path = temp.path().join(format!("{package_id}.zip"));
            write_reader(&path, package_id);
            let descriptor =
                PreparedContainedTask::describe_path("package-bundle", &path, admission_deadline())
                    .unwrap_or_else(|error| panic!("{package_id} is admitted: {error}"));
            assert_eq!(descriptor.server(), SERVER);
            assert_eq!(
                descriptor.recognition_problem(),
                None,
                "{package_id} qualifies as a recognition maintenance package"
            );
            descriptor
        })
        .collect()
}

// T3: one of the two qualifying packs declared for the server is the one that counts, so no
// warning; without a declaration for the server, the warning is raised.
#[test]
fn e3_t3_only_the_recognition_pack_declared_for_the_server_counts() {
    let temp = TempDir::new().expect("temporary directory");
    let descriptors = readers(&temp);

    let declared = [BundleMaintenance {
        package_id: FIRST.to_owned(),
        server: SERVER.to_owned(),
        uses: vec![MaintenanceUse::Recognition],
    }];
    let warnings = undeclared_recognition_packs(&declared, &descriptors);
    assert!(
        warnings.is_empty(),
        "one pack declared for the server: no warning, got {warnings:?}"
    );

    let warnings = undeclared_recognition_packs(&[], &descriptors);
    assert!(!warnings.is_empty(), "an undeclared server still warns");
    for warning in &warnings {
        assert_eq!(warning.code().as_str(), UNDECLARED);
        let package_id = warning
            .value("package_id")
            .and_then(Value::as_str)
            .expect("the warning names its pack");
        assert!(
            [FIRST, SECOND].contains(&package_id),
            "the warning names a qualifying pack of the server: {package_id}"
        );
    }
}
