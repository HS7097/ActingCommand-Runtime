// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S5, model M-PKG v1.1 §3.5, §4.6 S5-T3 and D9: `record stop` writes its
// package as `<package_id>.<ext>`; a file of that name already there is replaced in place (the
// new file beside it, the old one set aside and deleted), so a reader sees the old package or
// the new one, never part of one; the container round trip compares the expanded table with
// the generated one entry by entry, and a different table fails it. The recording is test plan
// E-5's synthetic EXEC-I5 recording (`gate_e5_record_stop`): two colour-marked steps, so the
// package is the single JSON container.

use crate::RecordStopOptions;
use crate::gate_e5_record_stop::Recorded;
use crate::recording::container::encode;
use crate::recording::crosscheck::round_trip;
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

// The fixture's package id: its game, server and task id (no `--package-id` given).
const PACKAGE_ID: &str = "neutral.test.neutral_task";
const FILE_NAME: &str = "neutral.test.neutral_task.json";
const OLD: &[u8] = b"an earlier package of the same name";

fn file_name(path: &Value) -> String {
    PathBuf::from(path.as_str().expect("a path"))
        .file_name()
        .and_then(|name| name.to_str())
        .expect("a file name")
        .to_owned()
}

// S5-T3: the package is `<package_id>.json` in the recording and in `--lab-dir`; an earlier file
// of that name is replaced (the dry run says `to_replace` and writes nothing); a reader that
// opened the old file before keeps reading it whole; afterwards the name holds exactly the new
// package, nothing else is left beside it, and no answer member, binding example or recorded
// artifact names a digest.
#[test]
fn s5_t3_record_stop_writes_its_package_as_its_package_id_and_replaces_one_of_that_name() {
    let recorded = Recorded::new();
    let lab_dir = recorded.temp.path().join("packages");
    fs::create_dir(&lab_dir).expect("--lab-dir");
    let target = lab_dir.join(FILE_NAME);
    fs::write(&target, OLD).expect("an earlier package");
    let options = RecordStopOptions {
        lab_dir: Some(lab_dir.display().to_string()),
        ..RecordStopOptions::default()
    };

    let validated = recorded
        .stop(&RecordStopOptions {
            dry_run: true,
            ..options.clone()
        })
        .unwrap_or_else(|(code, details)| panic!("dry run: {code}: {details}"));
    assert_eq!(validated["lab_status"], "validated", "{validated}");
    assert_eq!(validated["lab_dir_status"], "to_replace", "{validated}");
    assert_eq!(fs::read(&target).expect("the earlier package"), OLD);

    let mut reader = fs::File::open(&target).expect("a reader of the earlier package");
    let generated = recorded
        .stop(&options)
        .unwrap_or_else(|(code, details)| panic!("record stop: {code}: {details}"));
    let mut held = Vec::new();
    reader
        .read_to_end(&mut held)
        .expect("the reader keeps its file");
    drop(reader);
    assert_eq!(held, OLD);

    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(generated["lab_dir_status"], "replaced", "{generated}");
    assert_eq!(generated["package_id"], PACKAGE_ID);
    assert_eq!(file_name(&generated["path"]), FILE_NAME);
    assert_eq!(file_name(&generated["lab_dir_path"]), FILE_NAME);
    let written = fs::read(&target).expect("the new package");
    assert_ne!(written, OLD);
    assert_eq!(
        written,
        fs::read(generated["path"].as_str().expect("copy path")).expect("the recording's copy")
    );
    assert_eq!(json!(written.len()), generated["byte_count"]);
    let entries = expand_content_container(
        &written,
        ContentContainer::Json,
        ContainmentLimits::default(),
    )
    .expect("the new package expands");
    assert_eq!(
        generated["entries"],
        json!(entries.keys().collect::<Vec<_>>())
    );
    let mut beside = fs::read_dir(&lab_dir)
        .expect("--lab-dir")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    beside.sort();
    assert_eq!(beside, [FILE_NAME], "no .part or set-aside file is left");

    for member in ["digest", "sha256", "package_ref"] {
        assert!(generated.get(member).is_none(), "{member}: {generated}");
    }
    for example in ["binding_example", "prerequisite_entry_example"] {
        assert!(
            generated[example].get("package_digest").is_none(),
            "{example}: {}",
            generated[example]
        );
    }
    let recording: Value =
        serde_json::from_slice(&fs::read(recorded.recording_file()).expect("recording file"))
            .expect("recording");
    assert_eq!(recording["status"], "stopped");
    assert_eq!(recording["artifact"]["package_id"], PACKAGE_ID);
    for member in ["digest", "sha256"] {
        assert!(
            recording["artifact"].get(member).is_none(),
            "{member}: {}",
            recording["artifact"]
        );
    }
}

// S5-T3: the container round trip compares the expanded table with the generated one entry by
// entry: the encoding of the same table passes; the encoding of a table that differs in one
// file fails with `record_artifact_admission_failed` at stage `container_round_trip`, naming the
// differing path.
#[test]
fn s5_t3_a_container_that_expands_to_another_table_fails_its_round_trip() {
    let table = |task: &[u8]| {
        BTreeMap::from([
            ("control.json".to_owned(), b"{}\n".to_vec()),
            (
                "resources/operations/task/task.json".to_owned(),
                task.to_vec(),
            ),
        ])
    };
    let generated = table(br#"{"task_id":"task"}"#);
    let bytes = encode(&generated, ContentContainer::Json).expect("the generated container");
    round_trip(&bytes, ContentContainer::Json, &generated)
        .unwrap_or_else(|error| panic!("the same table: {}: {}", error.code, error.message));

    let other = encode(&table(br#"{"task_id":"other"}"#), ContentContainer::Json)
        .expect("another container");
    let error = round_trip(&other, ContentContainer::Json, &generated)
        .expect_err("another table fails the round trip");
    assert_eq!(error.code, "record_artifact_admission_failed");
    let details = error.details.expect("details");
    assert_eq!(details["stage"], "container_round_trip", "{details}");
    assert_eq!(
        details["paths"],
        json!(["resources/operations/task/task.json"]),
        "{details}"
    );
}
