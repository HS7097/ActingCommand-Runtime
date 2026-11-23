// SPDX-License-Identifier: AGPL-3.0-only

// #463 SEL-F, Addendum 1 (register §1.523; contracts/lab-recording.md, "Converting a package from
// its own files"; coordinator work order rd-1123/brief-463-self-fix1.md, its tests 10 and 11):
// `actinglab package fix-select` on the command line and through `serve --stdio --front mcp`. The
// conversion itself, on packages `record stop` generated, is the Lab's `gate_463_self` and
// `gate_463_self_scan`; here its row, its faces and its writing.
// - `self_t11_fix_select_is_a_row_beside_package_edit_and_needs_no_runtime` (11): the operation
//   table's row `package fix-select` has the module, the entries and the Runtime need of `package
//   edit` (authoring, the Lab's entry, offline), writes and is not irreversible; in an environment
//   that can reach no Runtime its command line runs and answers its own refusal of a package that
//   declares no variable (`record_select_invalid` with `variable_missing`, exit 1), and the MCP
//   face (`serve --stdio --front mcp`) answers that line.
// - `self_t10_a_refused_conversion_writes_nothing` (10): content directories that declare no such
//   variable, declare it with nothing that reads it, and have it read by a select's policy document
//   that is not as `record stop` writes it: `variable_missing`, `variable_read_elsewhere` and
//   `members_edited`, with and without `--dry-run`, each leaving every file's bytes as they were.
// No Runtime, no device: temporary files and the embedded operation table.

use super::env_lock;
use super::gate_434_lab5b4::{Isolated, printed};
use crate::cli_module::{self, serve};
use crate::package_tool::gate_390_tool1::neutral_package;
use actingcommand_lab_ops::table::{OperationModule, RuntimeNeed};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const VARIABLE: &str = "list_item";
const TASK_DIR: &str = "resources/operations/task";

// Every file of the content directory `root` by its `/`-separated path, with its bytes.
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("a directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                walk(root, &path, found);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("below the root")
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                found.insert(relative, fs::read(&path).expect("a file"));
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(root, root, &mut found);
    found
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    let mut bytes = serde_json::to_vec_pretty(value).expect("JSON");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("a file");
}

// The neutral content directory at `<temporary>/<name>`, its control declaring the variable
// `list_item` when `declared`, and, when `read`, a select step whose policy document names the
// variable's key but not as `record stop` writes it.
fn content(isolated: &Isolated, name: &str, declared: bool, read: bool) -> String {
    let root = isolated.temp.path().join(name);
    let mut task = neutral_package(&root);
    if declared {
        let control_path = root.join("control.json");
        let mut control: Value =
            serde_json::from_slice(&fs::read(&control_path).expect("control.json"))
                .expect("control JSON");
        control["variables"] = json!([{
            "name": VARIABLE,
            "type": "ordered",
            "title": {"en-US": VARIABLE},
            "options": [{"value": "item_a", "title": {"en-US": "ALPHA"}},
                        {"value": "item_b", "title": {"en-US": "BRAVO"}}],
            "min_count": 1,
            "max_count": 2,
            "default": ["item_b"]
        }]);
        write_json(&control_path, &control);
    }
    if read {
        write_json(
            &root.join(TASK_DIR).join("policies/step_01_select.json"),
            &json!({
                "facts": [{"fact_key": format!("session.dispatch.parameter.{VARIABLE}")}],
                "gates": [],
                "scoring": []
            }),
        );
        task["operations"]
            .as_array_mut()
            .expect("the operations")
            .push(json!({
                "id": "step_01_select",
                "select": {"layout_id": "layout/step_01",
                           "policy": {"path": "policies/step_01_select.json"}}
            }));
        write_json(&root.join(TASK_DIR).join("task.json"), &task);
    }
    root.display().to_string()
}

// The command line `package fix-select` of the variable on `package`, with `--dry-run` when `dry`.
fn line(package: &str, dry: bool) -> Vec<&str> {
    let mut words = vec!["--json"];
    if dry {
        words.push("--dry-run");
    }
    words.extend([
        "package",
        "fix-select",
        "--package",
        package,
        "--variable",
        VARIABLE,
    ]);
    words
}

#[test]
fn self_t11_fix_select_is_a_row_beside_package_edit_and_needs_no_runtime() {
    let table = cli_module::table().expect("the operation table");
    let row = table
        .operation("package fix-select")
        .expect("the table has the row package fix-select");
    let edit = table
        .operation("package edit")
        .expect("the table has the row package edit");
    assert_eq!(row.module, OperationModule::Authoring);
    assert_eq!(row.module, edit.module);
    assert_eq!(row.entries, edit.entries);
    assert_eq!(row.runtime, RuntimeNeed::Offline);
    assert_eq!(row.runtime, edit.runtime);
    assert!(row.writes && !row.irreversible && !row.disruptive);
    assert_eq!(
        row.params
            .iter()
            .map(|param| (param.name.as_str(), param.required))
            .collect::<Vec<_>>(),
        [("package", true), ("variable", true), ("fixed", false)]
    );

    let _guard = env_lock();
    let isolated = Isolated::new();
    let package = content(&isolated, "plain", false, false);
    let exit = printed(&line(&package, false));
    let answered = exit
        .line
        .clone()
        .unwrap_or_else(|text| panic!("the command line printed no line: {text}"));
    assert_eq!(exit.code, 1, "{answered}");
    assert_eq!(
        answered["outcome"]["code"], "record_select_invalid",
        "{answered}"
    );
    assert!(
        answered.to_string().contains("variable_missing"),
        "{answered}"
    );
    assert!(
        !isolated.runtime.exists(),
        "nothing reached for a Runtime's state"
    );

    let request = json!({
        "op": "package fix-select",
        "params": {"package": package, "variable": VARIABLE}
    });
    let mut output = Vec::new();
    let mut developer = Vec::new();
    serve::serve_as(
        table,
        Some(serve::Front::Mcp),
        format!("{request}\n").as_bytes(),
        &mut output,
        &mut developer,
    )
    .expect("the loop ends");
    let served = String::from_utf8(output).expect("UTF-8 lines");
    let lines = served.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one line: {served}");
    let envelope = serde_json::from_str::<Value>(lines[0])
        .unwrap_or_else(|error| panic!("one JSON line ({error}): {served}"));
    assert_eq!(envelope["exit_code"], 1, "{envelope}");
    assert_eq!(
        envelope["line"]["outcome"], answered["outcome"],
        "{envelope}"
    );
    assert_eq!(
        envelope["line"]["message"], answered["message"],
        "{envelope}"
    );
}

#[test]
fn self_t10_a_refused_conversion_writes_nothing() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let cases = [
        ("plain", false, false, "variable_missing"),
        ("unread", true, false, "variable_read_elsewhere"),
        ("edited", true, true, "members_edited"),
    ];
    for (name, declared, read, word) in cases {
        let package = content(&isolated, name, declared, read);
        let before = files(Path::new(&package));
        for dry in [true, false] {
            let exit = printed(&line(&package, dry));
            let answered = exit
                .line
                .clone()
                .unwrap_or_else(|text| panic!("{name}: no line: {text}"));
            assert_eq!(exit.code, 1, "{name}: {answered}");
            assert_eq!(
                answered["outcome"]["code"], "record_select_invalid",
                "{name}: {answered}"
            );
            assert!(answered.to_string().contains(word), "{name}: {answered}");
            assert!(
                files(Path::new(&package)) == before,
                "{name} (dry run {dry}): every file as it was"
            );
        }
    }
}
