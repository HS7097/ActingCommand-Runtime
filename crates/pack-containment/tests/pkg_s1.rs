// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 PKG-S1 (option 丙), model M-PKG v1.1 §4.2: a package is read by its path and
//! admitted structurally. Neither a digest of the package nor a pin inside it is compared, and
//! no name is read as an identity. Tests S1-T1, S1-T2, S1-T3 and S1-T5.

use actingcommand_contract::{ContentDirectory, ContentDirectoryVersion, PackageRef};
use actingcommand_pack_containment::{
    CONTENT_JSON_V1, Containment, ContainmentError, InstanceId, Sha256Hash,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A directory of its own below the system temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "actingcommand-pkg-s1-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A failed removal leaves a temporary directory behind; it never changes a result.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A text-only source package: its control, its resource table and one task whose only page
/// is recognized by a color probe.
fn source_entries() -> BTreeMap<String, Vec<u8>> {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.source",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "source_task"
    });
    let task = json!({
        "schema_version": "0.6",
        "task_id": "source_task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "neutral source fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [{
            "id": "home_color",
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": [255, 0, 0]
        }],
        "page_rules": {"home": {"required": ["home_color"]}},
        "entry_page": "home",
        "target_page": "home",
        "operations": []
    });
    BTreeMap::from([
        (
            "control.json".to_owned(),
            serde_json::to_vec_pretty(&control).expect("control JSON"),
        ),
        (
            "resources/operations/resources.json".to_owned(),
            br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#.to_vec(),
        ),
        (
            "resources/operations/source_task/task.json".to_owned(),
            serde_json::to_vec_pretty(&task).expect("task JSON"),
        ),
    ])
}

/// An assembled package (the layout a `.zip` with `resources/manifest.json` holds) whose
/// manifest declares `files`.
fn assembled_entries(files: Value) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "control.json".to_owned(),
            br#"{"game":"neutral","server":"test","entry_task_id":"assembled_task"}"#.to_vec(),
        ),
        (
            "resources/manifest.json".to_owned(),
            serde_json::to_vec(&json!({
                "schema_version": "0.3",
                "entry_task_id": "assembled_task",
                "files": files
            }))
            .expect("manifest JSON"),
        ),
        (
            "resources/operations/assembled_task/task.json".to_owned(),
            br#"{"schema_version":"0.5","task_id":"assembled_task","game":"neutral","server_scope":["test"],"coordinate_space":{"width":1,"height":1},"operations":[]}"#.to_vec(),
        ),
        (
            "resources/recognition/neutral.test.pack.json".to_owned(),
            br#"{"schema_version":"0.5","game":"neutral","server":"test","coordinate_space":{"width":1,"height":1},"targets":[{"type":"color","id":"home_color","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#.to_vec(),
        ),
        (
            "resources/recognition/neutral.test.pages.json".to_owned(),
            br#"{"schema_version":"0.5","pages":[{"id":"neutral/home","required":["home_color"]}]}"#
                .to_vec(),
        ),
    ])
}

fn write_directory(root: &Path, entries: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in entries {
        let file = path
            .split('/')
            .fold(root.to_path_buf(), |parent, part| parent.join(part));
        std::fs::create_dir_all(file.parent().expect("entry parent")).expect("entry directory");
        std::fs::write(file, bytes).expect("entry file");
    }
}

fn zip_bytes(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in entries {
        zip.start_file(path.as_str(), zip::write::FileOptions::default())
            .expect("ZIP entry");
        zip.write_all(bytes).expect("ZIP entry bytes");
    }
    zip.finish().expect("ZIP").into_inner()
}

fn content_json(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let files = entries
        .iter()
        .map(|(path, bytes)| {
            let text = String::from_utf8(bytes.clone()).expect("text entry");
            (path.clone(), Value::String(text))
        })
        .collect::<Map<_, _>>();
    serde_json::to_vec(&json!({"schema_version": CONTENT_JSON_V1, "files": files}))
        .expect("content JSON")
}

fn content_reference(sha256: &str) -> PackageRef {
    PackageRef::ContentDirectory(ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: sha256.to_owned(),
    })
}

fn legacy_reference(bytes: &[u8]) -> PackageRef {
    PackageRef::LegacyZipSha256(Sha256Hash::digest(bytes).to_string())
}

/// Admits `locator` under `reference`; the admitted entry task and the recorded reference.
fn admit(locator: &Path, reference: &PackageRef) -> Result<(String, PackageRef), ContainmentError> {
    let mut containment = Containment::new();
    let bundle = containment.load_path(
        &InstanceId::new("neutral-pkg-s1").expect("instance"),
        locator,
        reference,
        false,
        Instant::now() + Duration::from_secs(60),
    )?;
    Ok((
        bundle.task_id().as_str().to_owned(),
        bundle.package_ref().clone(),
    ))
}

// S1-T1: a directory whose name has the digest form but is not the digest of what it holds is
// admitted, under that name as its reference or under another; the reference is only recorded.
#[test]
fn s1_t1_a_digest_named_directory_is_admitted_whatever_its_content() {
    let scratch = Scratch::new("t1");
    let name = "d".repeat(64);
    let directory = scratch.path().join(&name);
    write_directory(&directory, &source_entries());
    for reference in [content_reference(&name), content_reference(&"e".repeat(64))] {
        let (task, recorded) = admit(&directory, &reference)
            .unwrap_or_else(|error| panic!("a digest-form name is no identity: {error}"));
        assert_eq!(task, "source_task");
        assert_eq!(recorded, reference);
    }
}

// S1-T2: a package changed in place is admitted under the reference it had before the change,
// and what is admitted is the changed content.
#[test]
fn s1_t2_a_package_changed_in_place_is_admitted_under_its_former_reference() {
    let scratch = Scratch::new("t2");
    let directory = scratch.path().join("package");
    let mut entries = source_entries();
    write_directory(&directory, &entries);
    // The reference the package had before the change, as the authoring tools measure it.
    let former = PackageRef::ContentDirectory(
        Containment::new()
            .snapshot_content_directory(&directory, Instant::now() + Duration::from_secs(60))
            .expect("measure the package")
            .reference,
    );
    admit(&directory, &former).expect("the package is admitted before the change");

    let task_path = "resources/operations/source_task/task.json";
    let mut task: Value = serde_json::from_slice(&entries[task_path]).expect("task JSON");
    task["goal"] = json!("neutral source fixture, revised in place");
    let revised = serde_json::to_vec_pretty(&task).expect("task JSON");
    entries.insert(task_path.to_owned(), revised.clone());
    write_directory(&directory, &entries);

    let mut containment = Containment::new();
    let bundle = containment
        .load_path(
            &InstanceId::new("neutral-pkg-s1").expect("instance"),
            &directory,
            &former,
            false,
            Instant::now() + Duration::from_secs(60),
        )
        .unwrap_or_else(|error| panic!("the former reference gates nothing: {error}"));
    assert_eq!(bundle.task_id().as_str(), "source_task");
    assert_eq!(bundle.entry(task_path), Some(revised.as_slice()));
    assert_eq!(bundle.package_ref(), &former);
}

// S1-T3: the same content as a directory, a `.json` container and a `.zip` container is
// admitted under a reference that is not its digest; an assembled package ZIP is admitted by
// its path under the same reference, and under a ZIP reference that is not its SHA-256.
#[test]
fn s1_t3_the_path_chooses_the_reading_and_no_reference_is_compared() {
    let scratch = Scratch::new("t3");
    let entries = source_entries();
    let unrelated = content_reference(&"e".repeat(64));
    let directory = scratch.path().join("source");
    write_directory(&directory, &entries);
    let json_container = scratch.path().join("source.json");
    std::fs::write(&json_container, content_json(&entries)).expect("JSON container");
    let zip_container = scratch.path().join("source.zip");
    std::fs::write(&zip_container, zip_bytes(&entries)).expect("ZIP container");
    for locator in [&directory, &json_container, &zip_container] {
        let (task, recorded) = admit(locator, &unrelated)
            .unwrap_or_else(|error| panic!("{} is refused: {error}", locator.display()));
        assert_eq!(task, "source_task", "{}", locator.display());
        assert_eq!(recorded, unrelated);
    }

    let assembled = scratch.path().join("assembled.zip");
    std::fs::write(&assembled, zip_bytes(&assembled_entries(json!([])))).expect("assembled");
    for reference in [
        unrelated.clone(),
        PackageRef::LegacyZipSha256("0".repeat(64)),
    ] {
        let (task, recorded) = admit(&assembled, &reference)
            .unwrap_or_else(|error| panic!("the assembled ZIP is refused: {error}"));
        assert_eq!(task, "assembled_task");
        assert_eq!(recorded, reference);
    }
}

// S1-T5: an assembled package whose manifest declares a wrong SHA-256 for one of its files is
// admitted; the per-file pins are compared with nothing.
#[test]
fn s1_t5_an_assembled_manifest_file_hash_is_not_compared() {
    let scratch = Scratch::new("t5");
    let entries = assembled_entries(json!([{
        "path": "operations/assembled_task/task.json",
        "sha256": format!("sha256:{}", "0".repeat(64))
    }]));
    let bytes = zip_bytes(&entries);
    let package = scratch.path().join("assembled.zip");
    std::fs::write(&package, &bytes).expect("assembled package");
    let reference = legacy_reference(&bytes);
    let (task, recorded) = admit(&package, &reference)
        .unwrap_or_else(|error| panic!("a wrong manifest file hash refuses nothing: {error}"));
    assert_eq!(task, "assembled_task");
    assert_eq!(recorded, reference);
}
