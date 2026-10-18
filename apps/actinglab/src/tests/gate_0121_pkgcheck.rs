// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A5 (M-PKG §3.2, S5-T4 "`package check` 只做结构准入"; register §1.97 PC1;
// apps key 71): `actinglab package check --package <path>` gives the Runtime's own structural
// admission verdict, the verdict of `PreparedContainedTask::describe_path` that check-config and
// the daemon read. For the same package it admits exactly what `describe_path` admits, answering
// `{status: "admitted", package_id, entry_task_id}` with exit 0, and refuses with top-level
// `package_invalid` (exit 2) whose `details.code` is the admission's own code, with the
// declaration a refused declaration names. A declared OCR model that is not installed locally
// does not refuse. A directory, a `.zip` and a `.json` container of the same content have the
// same verdict.

use crate::package_tool::gate_390_tool1::neutral_package;
use actingcommand_lab::PreparedContainedTask;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const TASK_FILE: &str = "resources/operations/task/task.json";
/// The files `neutral_package` writes.
const PACKAGE_FILES: [&str; 3] = [
    "control.json",
    "resources/operations/resources.json",
    TASK_FILE,
];
/// What the fixture's control declares.
const PACKAGE_ID: &str = "neutral.edit.source";
const ENTRY_TASK_ID: &str = "task";

/// One admission's verdict: admitted with what the package's control declares, or refused with
/// the admission's code and the declaration it names (`null` when it names none).
#[derive(Debug, PartialEq)]
enum Verdict {
    Admitted {
        package_id: String,
        entry_task_id: String,
    },
    Refused {
        code: String,
        declaration: Value,
    },
}

/// The verdict the Runtime's pack admission reads (`pack_admission.rs`: `describe_path` with a
/// 60 s budget).
fn runtime_verdict(package: &Path) -> Verdict {
    match PreparedContainedTask::describe_path(
        "pack-admission",
        package,
        Instant::now() + Duration::from_secs(60),
    ) {
        Ok(descriptor) => Verdict::Admitted {
            package_id: descriptor.package_id().to_owned(),
            entry_task_id: descriptor.entry_task_id().to_owned(),
        },
        Err(error) => Verdict::Refused {
            code: error.code().as_str().to_owned(),
            declaration: serde_json::to_value(error.declaration_issue()).unwrap(),
        },
    }
}

/// The verdict `actinglab --json package check --package <package>` answers, after checking the
/// answer's form: exit 0 with exactly `{status: "admitted", package_id, entry_task_id}`, or
/// exit 2 with `package_invalid` and `details.code`.
fn check_verdict(label: &str, package: &Path) -> Verdict {
    let result = crate::run_cli(
        [
            "--json",
            "package",
            "check",
            "--package",
            package.to_str().unwrap(),
        ],
        true,
    );
    let envelope = result.envelope_json();
    match (&result.envelope.data, &result.envelope.error) {
        (Some(data), None) => {
            assert_eq!(result.exit_code(), 0, "{label}: {envelope}");
            let members = data
                .as_object()
                .unwrap_or_else(|| panic!("{label}: an answer that is no object: {envelope}"))
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                members,
                BTreeSet::from(["entry_task_id", "package_id", "status"]),
                "{label}: {envelope}"
            );
            assert_eq!(data["status"], "admitted", "{label}: {envelope}");
            Verdict::Admitted {
                package_id: data["package_id"].as_str().unwrap().to_owned(),
                entry_task_id: data["entry_task_id"].as_str().unwrap().to_owned(),
            }
        }
        (None, Some(error)) => {
            assert_eq!(result.exit_code(), 2, "{label}: {envelope}");
            assert_eq!(error.code, "package_invalid", "{label}: {envelope}");
            let details = error
                .details
                .as_ref()
                .unwrap_or_else(|| panic!("{label}: a refusal without details: {envelope}"));
            Verdict::Refused {
                code: details["code"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{label}: no details.code: {envelope}"))
                    .to_owned(),
                declaration: details["declaration"].clone(),
            }
        }
        _ => panic!("{label}: neither an answer nor an error: {envelope}"),
    }
}

/// The content directory `neutral_package` writes, at `root`, with its task document changed
/// by `change`.
fn package_with(root: &Path, change: impl FnOnce(&mut Value)) -> PathBuf {
    let mut task = neutral_package(root);
    change(&mut task);
    fs::write(
        root.join(TASK_FILE),
        format!("{}\n", serde_json::to_string_pretty(&task).unwrap()),
    )
    .unwrap();
    root.to_path_buf()
}

/// The ZIP container of the content directory at `root`.
fn zip_container(root: &Path, path: PathBuf) -> PathBuf {
    let mut zip = ZipWriter::new(fs::File::create(&path).unwrap());
    for file in PACKAGE_FILES {
        zip.start_file(file, FileOptions::default()).unwrap();
        zip.write_all(&fs::read(root.join(file)).unwrap()).unwrap();
    }
    zip.finish().unwrap();
    path
}

/// The JSON container (`actingcommand.package.content-json.v1`) of the content directory at
/// `root`.
fn json_container(root: &Path, path: PathBuf) -> PathBuf {
    let files = PACKAGE_FILES
        .iter()
        .map(|file| {
            (
                (*file).to_owned(),
                Value::String(fs::read_to_string(root.join(file)).unwrap()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let container = json!({
        "schema_version": "actingcommand.package.content-json.v1",
        "files": files,
    });
    fs::write(&path, serde_json::to_vec(&container).unwrap()).unwrap();
    path
}

/// What the requirement says a row's verdict is.
enum Expected {
    Admitted,
    Refused {
        code: &'static str,
        declaration: bool,
    },
}

#[test]
fn package_check_gives_the_runtime_admission_verdict_for_every_package_form() {
    let temp = TempDir::new().unwrap();
    let directory = package_with(&temp.path().join("neutral.check.source"), |_| {});
    let unsupported = temp.path().join("neutral.check.txt");
    fs::write(&unsupported, "neutral\n").unwrap();
    let rows = [
        ("a content directory", directory.clone(), Expected::Admitted),
        (
            "the same content as a .zip container",
            zip_container(&directory, temp.path().join("neutral.check.zip")),
            Expected::Admitted,
        ),
        (
            "the same content as a .json container",
            json_container(&directory, temp.path().join("neutral.check.json")),
            Expected::Admitted,
        ),
        (
            "a missing path",
            temp.path().join("neutral.check.absent"),
            Expected::Refused {
                code: "content_directory_missing",
                declaration: false,
            },
        ),
        (
            "an unsupported file type",
            unsupported,
            Expected::Refused {
                code: "content_container_unsupported",
                declaration: false,
            },
        ),
        (
            "an unknown member in the task declaration",
            package_with(&temp.path().join("neutral.check.unknown"), |task| {
                task["unknown_member"] = json!(true);
            }),
            Expected::Refused {
                code: "resource_declaration_invalid",
                declaration: true,
            },
        ),
        (
            "a declared OCR model that is not installed locally",
            package_with(&temp.path().join("neutral.check.ocr"), |task| {
                task["ocr_targets"] = json!([{
                    "id": "ocr/label",
                    "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                    "languages": ["en"],
                    "timeout_ms": 1000,
                    "match_mode": "exact",
                    "expected": ["unused"],
                    "case_sensitive": false,
                    "minimum_confidence": 0.0,
                    "model_ref": "neutral-ocr-model-not-installed",
                    "model_sha256": "a".repeat(64)
                }]);
            }),
            Expected::Admitted,
        ),
    ];
    for (label, package, expected) in rows {
        let checked = check_verdict(label, &package);
        // Verdict = the Runtime's: the same admission, or the same code and declaration.
        assert_eq!(checked, runtime_verdict(&package), "{label}: parity");
        match expected {
            Expected::Admitted => assert_eq!(
                checked,
                Verdict::Admitted {
                    package_id: PACKAGE_ID.to_owned(),
                    entry_task_id: ENTRY_TASK_ID.to_owned(),
                },
                "{label}"
            ),
            Expected::Refused { code, declaration } => match &checked {
                Verdict::Refused {
                    code: checked_code,
                    declaration: checked_declaration,
                } => {
                    assert_eq!(checked_code, code, "{label}");
                    assert_eq!(
                        checked_declaration.is_object(),
                        declaration,
                        "{label}: {checked_declaration}"
                    );
                }
                admitted => panic!("{label}: admitted, not refused: {admitted:?}"),
            },
        }
    }
}
