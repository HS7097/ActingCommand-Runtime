// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 TOOL-1 (forced-compromises mapping row TOOL-1, compromise P-03; coordinator
// ruling 3 of wave 15): `actinglab package fmt` writes the canonical serialization (two-space
// indentation, CRLF line endings) of every JSON file of a content directory and, with --check,
// writes nothing and fails naming the files that are not canonical; `actinglab package edit`
// adds and replaces by JSON pointer (RFC 6901 pointers, RFC 6902 add and replace), then admits
// the edited package structurally before it writes anything. Both take the content directory by
// its path; no digest is computed or reported.

use super::*;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

const TASK_FILE: &str = "resources/operations/task/task.json";

// The canonical serialization as FCM P-03 states it: two-space indentation, one member or
// element per line, every line ending in CRLF, the last one too.
fn canonical_text(value: &Value) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap();
    format!("{}\r\n", pretty.replace('\n', "\r\n"))
}

fn run(args: &[&str]) -> crate::CliResult {
    crate::run_cli(args.iter().copied(), true)
}

fn path_text(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn assert_no_digest(result: &crate::CliResult) {
    let envelope = result.envelope_json();
    assert!(
        !envelope.contains("sha256") && !envelope.contains("digest"),
        "{envelope}"
    );
}

fn error_code(result: &crate::CliResult) -> &str {
    let Some(error) = result.envelope.error.as_ref() else {
        panic!("an error: {}", result.envelope_json());
    };
    error.code.as_str()
}

// An admissible content directory (the neutral fixture of the PKG-S4 dry-run gate), its JSON
// files written in the canonical serialization; returns its task document.
fn neutral_package(root: &Path) -> Value {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.edit.source",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10,
        "timeout_ms": 100,
        "max_steps": 2
    });
    let probe = |id: &str, expected: [u8; 3]| {
        json!({
            "id": id,
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "neutral content directory fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home_red", [255, 0, 0]), probe("terminal_blue", [0, 0, 255])],
        "page_rules": {
            "home": {"required": ["home_red"]},
            "terminal": {"required": ["terminal_blue"]}
        },
        "entry_page": "home",
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "purpose": "exercise the content-directory edit",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        (TASK_FILE, task.clone()),
    ] {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, canonical_text(&value)).unwrap();
    }
    task
}

fn package_files(root: &Path) -> Vec<(&'static str, Vec<u8>)> {
    [
        "control.json",
        "resources/operations/resources.json",
        TASK_FILE,
    ]
    .into_iter()
    .map(|path| (path, fs::read(root.join(path)).unwrap()))
    .collect()
}

#[test]
fn tool1_package_fmt_writes_every_json_file_in_the_canonical_serialization() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.fmt.source");
    fs::create_dir_all(package.join("data")).unwrap();
    fs::write(
        package.join("data/compact.json"),
        "{\"zeta\":1,\"alpha\":[true,null],\"name\":\"\u{e9}t\u{e9}\",\"nested\":{\"b\":2,\"a\":{}}}",
    )
    .unwrap();
    fs::write(
        package.join("data/four_spaces.json"),
        "{\n    \"list\": [],\n    \"text\": \"line\\nbreak\"\n}\n",
    )
    .unwrap();
    fs::write(package.join("data/notes.txt"), "kept\nas it is\n").unwrap();

    let result = run(&["--json", "package", "fmt", "--package", path_text(&package)]);

    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(data["json_files"], 2);
    assert_eq!(
        data["reformatted"],
        json!(["data/compact.json", "data/four_spaces.json"])
    );
    // Members keep their order, text stays UTF-8 and an escaped line break stays escaped.
    assert_eq!(
        fs::read_to_string(package.join("data/compact.json")).unwrap(),
        "{\r\n  \"zeta\": 1,\r\n  \"alpha\": [\r\n    true,\r\n    null\r\n  ],\r\n  \"name\": \
         \"\u{e9}t\u{e9}\",\r\n  \"nested\": {\r\n    \"b\": 2,\r\n    \"a\": {}\r\n  }\r\n}\r\n"
    );
    assert_eq!(
        fs::read_to_string(package.join("data/four_spaces.json")).unwrap(),
        "{\r\n  \"list\": [],\r\n  \"text\": \"line\\nbreak\"\r\n}\r\n"
    );
    assert_eq!(
        fs::read_to_string(package.join("data/notes.txt")).unwrap(),
        "kept\nas it is\n"
    );
    assert_no_digest(&result);

    let again = run(&["--json", "package", "fmt", "--package", path_text(&package)]);
    assert_eq!(again.exit_code(), 0, "{}", again.envelope_json());
    assert_eq!(
        again.envelope.data.as_ref().unwrap()["reformatted"],
        json!([])
    );
}

#[test]
fn tool1_package_fmt_check_names_the_unformatted_files_and_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.fmt.check");
    fs::create_dir_all(package.join("data")).unwrap();
    let canonical = "{\r\n  \"kept\": true\r\n}\r\n";
    let compact = "{\"kept\":false}";
    fs::write(package.join("data/canonical.json"), canonical).unwrap();
    fs::write(package.join("data/compact.json"), compact).unwrap();

    let check = run(&[
        "--json",
        "package",
        "fmt",
        "--package",
        path_text(&package),
        "--check",
    ]);

    assert_eq!(check.exit_code(), 2, "{}", check.envelope_json());
    assert_eq!(
        error_code(&check),
        crate::codes::ActinglabCode::PackageFmtCheckFailed.as_str()
    );
    let details = check.envelope.error.as_ref().unwrap().details.as_ref();
    assert_eq!(
        details.unwrap()["unformatted"],
        json!(["data/compact.json"])
    );
    assert_eq!(
        fs::read_to_string(package.join("data/canonical.json")).unwrap(),
        canonical
    );
    assert_eq!(
        fs::read_to_string(package.join("data/compact.json")).unwrap(),
        compact
    );

    let format = run(&["--json", "package", "fmt", "--package", path_text(&package)]);
    assert_eq!(format.exit_code(), 0, "{}", format.envelope_json());
    let passed = run(&[
        "--json",
        "package",
        "fmt",
        "--package",
        path_text(&package),
        "--check",
    ]);
    assert_eq!(passed.exit_code(), 0, "{}", passed.envelope_json());
    assert_eq!(
        passed.envelope.data.as_ref().unwrap()["unformatted"],
        json!([])
    );
}

#[test]
fn tool1_package_edit_adds_and_replaces_by_json_pointer_then_admits_and_writes() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.edit.source");
    let task = neutral_package(&package);
    let before = package_files(&package);
    let patch = temp.path().join("edits.json");
    let edits = json!([
        {"file": TASK_FILE, "op": "replace", "path": "/defaults/template_threshold", "value": 0.95},
        {"file": TASK_FILE, "op": "replace", "path": "/operations/0/click",
         "value": {"kind": "point", "x": 0, "y": 0}},
        {"file": TASK_FILE, "op": "add", "path": "/provenance", "value": {"edits": ["threshold"]}},
        {"file": TASK_FILE, "op": "add", "path": "/provenance/edits/-", "value": "click"},
        {"file": TASK_FILE, "op": "add", "path": "/provenance/edits/0", "value": "first"}
    ]);
    fs::write(&patch, serde_json::to_vec(&edits).unwrap()).unwrap();

    let result = run(&[
        "--json",
        "package",
        "edit",
        "--package",
        path_text(&package),
        "--patch",
        path_text(&patch),
    ]);

    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().unwrap();
    assert_eq!(data["package_id"], "neutral.edit.source");
    assert_eq!(data["admission"], "passed");
    assert_eq!(data["edits"], 5);
    assert_eq!(data["files"], json!([TASK_FILE]));
    assert_eq!(data["written"], true);
    let mut expected = task;
    expected["defaults"]["template_threshold"] = json!(0.95);
    expected["operations"][0]["click"] = json!({"kind": "point", "x": 0, "y": 0});
    expected["provenance"] = json!({"edits": ["first", "threshold", "click"]});
    assert_eq!(
        fs::read_to_string(package.join(TASK_FILE)).unwrap(),
        canonical_text(&expected)
    );
    for (path, bytes) in &before[..2] {
        assert_eq!(&fs::read(package.join(path)).unwrap(), bytes, "{path}");
    }
    assert_no_digest(&result);
}

#[test]
fn tool1_package_edit_takes_one_edit_from_its_flags() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.edit.source");
    let task = neutral_package(&package);

    let result = run(&[
        "--json",
        "package",
        "edit",
        "--package",
        path_text(&package),
        "--file",
        TASK_FILE,
        "--replace",
        "/defaults/template_threshold",
        "--value",
        "0.8",
    ]);

    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let mut expected = task;
    expected["defaults"]["template_threshold"] = json!(0.8);
    assert_eq!(
        fs::read_to_string(package.join(TASK_FILE)).unwrap(),
        canonical_text(&expected)
    );
}

#[test]
fn tool1_package_edit_refused_by_admission_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.edit.source");
    neutral_package(&package);
    let before = package_files(&package);
    let patch = temp.path().join("edits.json");
    // The second edit misspells a member the task declares, which admission refuses.
    let edits = json!([
        {"file": TASK_FILE, "op": "replace", "path": "/defaults/template_threshold", "value": 0.95},
        {"file": TASK_FILE, "op": "add", "path": "/provenence", "value": {"edits": []}}
    ]);
    fs::write(&patch, serde_json::to_vec(&edits).unwrap()).unwrap();

    let result = run(&[
        "--json",
        "package",
        "edit",
        "--package",
        path_text(&package),
        "--patch",
        path_text(&patch),
    ]);

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(
        error_code(&result),
        crate::codes::ActinglabCode::PackageEditAdmissionRefused.as_str()
    );
    assert_eq!(package_files(&package), before);
}

#[test]
fn tool1_package_edit_pointer_that_does_not_resolve_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.edit.source");
    neutral_package(&package);
    let before = package_files(&package);

    let result = run(&[
        "--json",
        "package",
        "edit",
        "--package",
        path_text(&package),
        "--file",
        TASK_FILE,
        "--replace",
        "/defaults/template_treshold",
        "--value",
        "0.95",
    ]);

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(
        error_code(&result),
        crate::codes::ActinglabCode::PackageEditPointerUnresolved.as_str()
    );
    assert_eq!(package_files(&package), before);
}

// RFC 6902: add puts a member in an object (an existing one is replaced in its place), inserts
// into an array at an index up to its length, `-` appending; replace needs the member or element
// to exist; the empty pointer is the whole document. RFC 6901: `~1` is `/`, `~0` is `~`, and an
// array index has no leading zero.
#[test]
fn tool1_add_and_replace_follow_rfc_6902() {
    let base = || json!({"object": {"member": 1, "other": 2}, "list": [10, 20]});
    let applied = [
        (
            EditOp::Add,
            "/object/new",
            json!(3),
            r#"{"object":{"member":1,"other":2,"new":3},"list":[10,20]}"#,
        ),
        (
            EditOp::Add,
            "/object/member",
            json!(5),
            r#"{"object":{"member":5,"other":2},"list":[10,20]}"#,
        ),
        (
            EditOp::Add,
            "/list/0",
            json!(5),
            r#"{"object":{"member":1,"other":2},"list":[5,10,20]}"#,
        ),
        (
            EditOp::Add,
            "/list/2",
            json!(30),
            r#"{"object":{"member":1,"other":2},"list":[10,20,30]}"#,
        ),
        (
            EditOp::Add,
            "/list/-",
            json!(30),
            r#"{"object":{"member":1,"other":2},"list":[10,20,30]}"#,
        ),
        (
            EditOp::Add,
            "/a~1b~0c",
            json!(1),
            r#"{"object":{"member":1,"other":2},"list":[10,20],"a/b~c":1}"#,
        ),
        (
            EditOp::Replace,
            "/object/member",
            json!({"deep": true}),
            r#"{"object":{"member":{"deep":true},"other":2},"list":[10,20]}"#,
        ),
        (
            EditOp::Replace,
            "/list/1",
            json!(25),
            r#"{"object":{"member":1,"other":2},"list":[10,25]}"#,
        ),
        (EditOp::Replace, "", json!([]), "[]"),
    ];
    for (op, pointer, value, expected) in applied {
        let mut document = base();
        if let Err(failure) = apply(&mut document, op, pointer, value) {
            panic!("{op:?} {pointer}: {failure:?}");
        }
        assert_eq!(
            serde_json::to_string(&document).unwrap(),
            expected,
            "{op:?} {pointer}"
        );
    }
    let refused = [
        (
            EditOp::Replace,
            "/object/absent",
            EditFailure::PointerUnresolved,
        ),
        (
            EditOp::Add,
            "/absent/member",
            EditFailure::PointerUnresolved,
        ),
        (EditOp::Add, "/list/3", EditFailure::PointerUnresolved),
        (EditOp::Replace, "/list/2", EditFailure::PointerUnresolved),
        (EditOp::Replace, "/list/-", EditFailure::PointerUnresolved),
        (EditOp::Replace, "/list/01", EditFailure::PointerUnresolved),
        (
            EditOp::Add,
            "/object/member/deeper",
            EditFailure::PointerUnresolved,
        ),
        (EditOp::Add, "object", EditFailure::PointerInvalid),
        (EditOp::Replace, "/object/~2", EditFailure::PointerInvalid),
    ];
    for (op, pointer, failure) in refused {
        let mut document = base();
        assert_eq!(
            apply(&mut document, op, pointer, json!(0)),
            Err(failure),
            "{op:?} {pointer}"
        );
        assert_eq!(document, base(), "{op:?} {pointer} changed the document");
    }
}
