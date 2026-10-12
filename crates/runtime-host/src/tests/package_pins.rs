// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 PKG-S1 (option 丙), model M-PKG v1.1 §4.2 S1-T4: the `sha256` pins of a
//! post-admission OCR truth set and of an OCR field dictionary are well formed but name other
//! bytes; the package is admitted all the same, because no pin inside a package is compared.
//! The keys stay required and format-checked until PKG-S2.

use super::*;
use serde_json::{Value, json};

/// The entries of a ZIP package, by name.
fn package_entries(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("package ZIP");
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("package entry");
        let mut contents = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut contents).expect("package entry bytes");
        entries.insert(entry.name().to_owned(), contents);
    }
    entries
}

fn package_bytes(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, contents) in entries {
        zip.start_file(
            path.as_str(),
            FileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )
        .expect("package entry");
        zip.write_all(contents).expect("package entry bytes");
    }
    zip.finish().expect("package ZIP").into_inner()
}

/// The schema 0.7 post-admission OCR package whose truth set pin names other bytes.
fn truth_set_with_unrelated_pin() -> BTreeMap<String, Vec<u8>> {
    let mut entries = package_entries(&neutral_post_admission_ocr_contained_task_package(false));
    let task_path = "resources/operations/task/task.json";
    let mut task: Value = serde_json::from_slice(&entries[task_path]).expect("task JSON");
    task["post_admission_ocr"]["truth_set"]["sha256"] = json!("c".repeat(64));
    entries.insert(
        task_path.to_owned(),
        serde_json::to_vec(&task).expect("task JSON"),
    );
    entries
}

/// The schema 0.8 OCR fields package (as the fields callback tests build it) whose dictionary
/// pin names other bytes.
fn dictionary_with_unrelated_pin() -> BTreeMap<String, Vec<u8>> {
    let mut entries = package_entries(&neutral_post_admission_ocr_contained_task_package(true));
    let truth = serde_json::to_vec(
        &json!({"schema_version":"actingcommand.ocr-truth-set.v2","items":["home"],"aliases":[]}),
    )
    .expect("dictionary JSON");
    let truth_sha = format!("{:x}", Sha256::digest(&truth));
    let mut control: Value =
        serde_json::from_slice(&entries["control.json"]).expect("control JSON");
    control["execution_mode"] = json!("navigable_route");
    control["capture_interval_ms"] = json!(3);
    control["step_timeout_ms"] = json!(27);
    control
        .as_object_mut()
        .expect("control object")
        .remove("stability_termination");
    let mut task: Value =
        serde_json::from_slice(&entries["resources/operations/task/task.json"]).expect("task");
    task["schema_version"] = json!("0.8");
    task["target_page"] = json!("terminal");
    task["operations"][0]["to"] = json!("terminal");
    task["operations"][0]["expect_after"] =
        json!({"page_id":"terminal","timeout_ms":480_000,"interval_ms":7});
    task["operations"][0]["post_delay_ms"] = json!(1);
    task.as_object_mut()
        .expect("task object")
        .remove("stability_termination");
    task["post_admission_ocr"] = json!({"mode":"fields_v1","page_ids":["home"],"fields":[{
        "id":"location","group":"page","target_id":"fixture/ocr","required":true,"privacy":"public","trim":"whitespace_v1",
        "value":{"type":"dictionary_entry","dictionary":{"path":"truth.json","sha256":"c".repeat(64)}}}],
        "limits":{"max_frames":2,"max_items":16,"max_string_bytes":64,"max_total_bytes":4096,"max_truth_entries":16},"outcome_key":"fields_recorded"});
    task["scheduling_outcome"] = json!({"mappings":[{"outcome_key":"fields_recorded","effect":"no_designated_effect","terminal_pages":["terminal"]}]});
    let mut manifest: Value =
        serde_json::from_slice(&entries["resources/manifest.json"]).expect("manifest JSON");
    manifest["files"][0]["sha256"] = json!(truth_sha);
    entries.insert(
        "control.json".to_owned(),
        serde_json::to_vec(&control).expect("control JSON"),
    );
    entries.insert(
        "resources/operations/task/task.json".to_owned(),
        serde_json::to_vec(&task).expect("task JSON"),
    );
    entries.insert("resources/operations/task/truth.json".to_owned(), truth);
    entries.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(&manifest).expect("manifest JSON"),
    );
    entries
}

#[test]
fn s1_t4_ocr_truth_set_and_dictionary_pins_are_not_compared_at_admission() {
    for (name, entries) in [
        ("truth-set", truth_set_with_unrelated_pin()),
        ("dictionary", dictionary_with_unrelated_pin()),
    ] {
        let root = TempDir::new().expect("tempdir");
        let bytes = package_bytes(&entries);
        let path = root.path().join(format!("{name}.zip"));
        fs::write(&path, &bytes).expect("package file");
        let reference =
            actingcommand_contract::PackageRef::from(format!("{:x}", Sha256::digest(&bytes)));
        let admitted = PreparedContainedTask::load_path(
            "node.a",
            &path,
            &reference,
            Some(Arc::new(FakeVisionProvider::default())),
            Instant::now() + Duration::from_secs(60),
        );
        assert!(
            admitted.is_ok(),
            "{name}: the package is refused: {:?}",
            admitted.err()
        );
    }
}
