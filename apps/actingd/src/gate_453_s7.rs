// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S7 (register §1.355; contracts/vision-model-folders.md, "The manifest"; coordinator
// work order rd-1114/brief-453-s7s8-vision-manifest.md): `actingd vision manifest` and `actingd
// vision check` on an injected pin table (a made-up registry and runtime pin) and temporary vision
// roots whose files are made-up bytes, never the real files:
// - `s7_manifest_directml_lists_the_library_with_the_code_pin_and_cpu_does_not`: the `directml`
//   manifest lists `ort/DirectML.dll` with the runtime pin's size, SHA-256, component and archive
//   path, and its component's license file; the `cpu` manifest lists neither; both list the
//   runtime's two libraries and its license file and the model's three files, with the model's
//   identity; no member names an address; the report line; and on the pin table this build
//   carries, the library is `DIRECTML_RUNTIME_PIN`'s exactly, for `directml` only;
// - `s7_check_a_complete_vision_root_answers_every_file_ok_and_equal_identities`: every file
//   `ok`, the model folder's identity the manifest's, the report line, nothing written and no
//   state root made;
// - `s7_check_names_a_missing_a_changed_and_an_unreadable_file` (Windows: the unreadable file is
//   one another handle holds open without sharing): `missing`, `sha256_mismatch` and
//   `unreadable` with the OS failure, every other file `ok`;
// - `s7_check_a_model_folder_of_another_identity_does_not_match`: a dictionary of other bytes
//   gives the folder another identity (`identity_matches` false), and the folders are listed as a
//   start lists them, a classifier folder the manifest does not name (fix 1: listed by name, not
//   asked about, no identity) and a folder that breaks the rule;
// - `s7_check_a_vision_root_that_cannot_be_resolved_relays_the_existing_cause`: a configured
//   root that does not exist is `vision_root_unavailable` at stage `config_vision`, the OS
//   failure its link, as a start refuses it.
// Fix 1 (review; coordinator work order rd-1114/brief-1693-fix1.md):
// - `s7_check_compares_the_size_first_and_reads_no_manifest_file_past_its_pin`: the reader hashes
//   no byte past its limit; a manifest file one byte longer than its pin is `sha256_mismatch` with
//   no cause, never opened (on Windows another handle holds it without sharing);
// - `s7_check_hashes_only_the_folders_it_is_asked_about_within_its_bounds`: with `--model`, the
//   named classifier folder's identity and another folder listed `asked` false with none; a file
//   over the file bound, and one over what the total bound leaves after the manifest's files,
//   each named in `over_bound` and not read, the manifest's folder still identified.
// Fix 2 (review; coordinator work order rd-1114/brief-1693-fix2.md):
// - `s7_check_reads_a_file_exactly_at_each_bound_and_none_one_byte_past_it`: a folder's file of
//   exactly the file bound is hashed, one byte more is `over_bound` `file`; one of exactly what the
//   total bound leaves is hashed, one byte more `over_bound` `total`; manifest files whose pins
//   hold exactly the total are read, one byte less of total refuses the build
//   (`serialization_failed`);
// - `s7_check_takes_64_model_names_and_refuses_a_65th_or_a_bad_name_before_reading`: 64 `--model`
//   names (one of the longest, 255 bytes) are answered, each that no folder has as `asked` and
//   `absent`; a 65th, a name of 256 bytes, and `..`, `.`, a blank name, a name with `/`, `\`, `:`
//   or a control character are `invalid_arguments` with no stage, on a configuration that does
//   not exist (so nothing was read).

use super::vision_report::{Bounds, Manifest, PinTable};
use super::*;
use actingcommand_vision_ffi::{
    DIRECTML_LIBRARY, DIRECTML_RUNTIME_PIN, DirectmlRuntimePin, ONNXRUNTIME_LIBRARY,
    ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY, OnnxExecutionProvider, PinnedRuntimeFile,
    ppocr_model_set_sha256, sha256_hex,
};
use serde_json::{Value, json};
use std::fs;

const RUNTIME: &str = "neutral-runtime";
const GPU: &str = "neutral-gpu";
const MODEL_COMPONENT: &str = "neutral-model";
const MODEL: &str = "neutral.ocr";
const RUNTIME_LICENSE: &str = "ort/licenses/onnxruntime/LICENSE";
const GPU_LICENSE: &str = "ort/licenses/directml/LICENSE.txt";
const DETECTOR: &str = "models/neutral.ocr/det.onnx";
const RECOGNIZER: &str = "models/neutral.ocr/rec.onnx";
const DICTIONARY: &str = "models/neutral.ocr/keys.txt";

fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(std::ffi::OsString::from).collect()
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// The made-up bytes of the file at `place` below a vision root.
fn content(place: &str) -> Vec<u8> {
    format!("neutral bytes of {place}").into_bytes()
}

fn length_of(place: &str) -> u64 {
    u64::try_from(content(place).len()).expect("a small file")
}

fn sha_of(place: &str) -> String {
    sha256_hex(&content(place))
}

/// One runtime library of the made-up pin, pinned by its made-up bytes.
fn pinned(
    file_name: &'static str,
    tool_source: &'static str,
    archive_path: &'static str,
) -> PinnedRuntimeFile {
    let place = format!("ort/{file_name}");
    PinnedRuntimeFile {
        file_name,
        size: length_of(&place),
        sha256: leak(sha_of(&place)),
        tool_source,
        archive_path,
    }
}

/// A pinned license file of a registry component.
fn license(path: &str, place: &str) -> Value {
    json!({"path": path, "vision_root_path": place, "size": length_of(place),
           "sha256": sha_of(place), "hash_provenance": "made up"})
}

/// An artifact of the model component at `place`, with the address a registry names.
fn artifact(place: &str) -> Value {
    json!({"id": place, "url": format!("https://example.invalid/{place}"),
           "relative_path": format!("neutral/{place}"), "vision_root_path": place,
           "size": length_of(place), "sha256": sha_of(place)})
}

/// The made-up pin table: the runtime's two libraries and the GPU library of the pin; the
/// registry's license file of each of their components, and the OCR model `neutral.ocr` of
/// three files; one more component whose artifact has no place in a vision root.
fn table() -> PinTable<'static> {
    let registry = json!({
        "schema_version": "actingcommand.windows_tool_sources.v1",
        "components": {
            RUNTIME: {
                "kind": "download_zip",
                "archive": {"url": "https://example.invalid/runtime.zip"},
                "pinned_license_files": [license("LICENSE", RUNTIME_LICENSE)]
            },
            GPU: {
                "kind": "download_zip",
                "archive": {"url": "https://example.invalid/gpu.zip"},
                "pinned_license_files": [license("LICENSE.txt", GPU_LICENSE)]
            },
            MODEL_COMPONENT: {
                "kind": "download_files",
                "model_ref": MODEL,
                "family": "ppocr-ctc",
                "artifacts": [artifact(DETECTOR), artifact(RECOGNIZER), artifact(DICTIONARY)]
            },
            "neutral-elsewhere": {
                "kind": "download_files",
                "artifacts": [{"id": "elsewhere", "url": "https://example.invalid/elsewhere",
                               "size": 1, "sha256": "0".repeat(64)}]
            }
        }
    });
    let pin = DirectmlRuntimePin {
        onnxruntime_version: "1.24.4",
        directml_version: "1.15.4",
        onnxruntime: pinned(ONNXRUNTIME_LIBRARY, RUNTIME, "native/onnxruntime.dll"),
        providers_shared: pinned(
            ONNXRUNTIME_SHARED_PROVIDERS_LIBRARY,
            RUNTIME,
            "native/onnxruntime_providers_shared.dll",
        ),
        directml: pinned(DIRECTML_LIBRARY, GPU, "native/DirectML.dll"),
    };
    PinTable {
        registry: leak(registry.to_string()),
        pin: Box::leak(Box::new(pin)),
    }
}

/// The made-up model's identity from its three files' bytes.
fn model_identity(dictionary: &[u8]) -> String {
    ppocr_model_set_sha256(
        Some(sha_of(DETECTOR).as_str()),
        &sha_of(RECOGNIZER),
        &sha256_hex(dictionary),
        None,
    )
    .expect("an identity")
}

/// `place` below `root`, its `/` segments joined.
fn at(root: &Path, place: &str) -> PathBuf {
    place
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Writes `bytes` at `place` below `root`.
fn write(root: &Path, place: &str, bytes: &[u8]) {
    let path = at(root, place);
    fs::create_dir_all(path.parent().expect("a parent folder")).expect("the folder");
    fs::write(path, bytes).expect("the file");
}

/// The places of a complete vision root of `cpu` for the made-up table.
const CPU_PLACES: [&str; 6] = [
    "ort/onnxruntime.dll",
    "ort/onnxruntime_providers_shared.dll",
    RUNTIME_LICENSE,
    DETECTOR,
    RECOGNIZER,
    DICTIONARY,
];

/// Every file below `dir` with its bytes, by its path relative to `dir`.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("list a folder") {
            let entry = entry.expect("a folder entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("an entry type").is_dir() {
                pending.push((entry.path(), relative));
            } else {
                files.insert(relative, fs::read(entry.path()).expect("read a file"));
            }
        }
    }
    files
}

/// A configuration in `base` whose `vision` section is `vision`; its state root is never made.
fn config(base: &Path, vision: &Value) -> PathBuf {
    let config = base.join("actingd.config.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": base.join("state"),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "vision": vision,
            "instances": []
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");
    config
}

/// `actingd vision check --config <config>` on the made-up table, as its report line.
fn check_line(config: &Path) -> Value {
    check_line_with(config, &[], vision_report::BOUNDS)
}

/// `actingd vision check --config <config> <more>` on the made-up table within `bounds`, as its
/// report line.
fn check_line_with(config: &Path, more: &[&str], bounds: Bounds) -> Value {
    let mut words = vec![
        "vision",
        "check",
        "--config",
        config.to_str().expect("a UTF-8 path"),
    ];
    words.extend_from_slice(more);
    let reported = vision_report::run_with(&arguments(&words), table(), bounds)
        .unwrap_or_else(|error| panic!("vision check answers: {}", error.outcome()));
    let (outcome, developer, line) = report_line(
        vision_report::SCHEMA_VERSION,
        SharedCode::SerializationFailed,
        Ok(reported),
        Detail::Full,
    );
    assert!(outcome.is(ActingdCode::VisionCheckCompleted), "{outcome}");
    assert!(developer.is_none(), "{developer:?}");
    let line: Value = serde_json::from_str(&line).expect("one JSON line");
    assert_eq!(line["schema_version"], "actingcommand.actingd.vision.v1");
    assert_eq!(line["outcome"]["code"], "vision_check_completed", "{line}");
    line
}

/// `--model <name>` for each of `names`, in their order.
fn model_words(names: &[String]) -> Vec<&str> {
    names
        .iter()
        .flat_map(|name| ["--model", name.as_str()])
        .collect()
}

/// The places of a manifest's files, in its order.
fn places(manifest: &Manifest) -> Vec<&str> {
    manifest
        .files
        .iter()
        .map(|file| file.vision_root_path.as_str())
        .collect()
}

/// Each file of a check's report by its place: its state, and its cause when it has one.
fn states(report: &Value) -> BTreeMap<String, (String, Value)> {
    report["files"]
        .as_array()
        .expect("the files")
        .iter()
        .map(|file| {
            (
                file["vision_root_path"]
                    .as_str()
                    .expect("a place")
                    .to_owned(),
                (
                    file["state"].as_str().expect("a state").to_owned(),
                    file.get("cause").cloned().unwrap_or(Value::Null),
                ),
            )
        })
        .collect()
}

#[test]
fn s7_manifest_directml_lists_the_library_with_the_code_pin_and_cpu_does_not() {
    let table = table();
    let directml = vision_report::manifest(table, OnnxExecutionProvider::Directml)
        .unwrap_or_else(|error| panic!("the directml manifest: {}", error.outcome()));
    let cpu = vision_report::manifest(table, OnnxExecutionProvider::Cpu)
        .unwrap_or_else(|error| panic!("the cpu manifest: {}", error.outcome()));
    let mut expected_cpu = CPU_PLACES.to_vec();
    expected_cpu.sort_unstable();
    assert_eq!(
        places(&cpu),
        expected_cpu,
        "cpu: the runtime, its license, the model"
    );
    let mut expected_directml = expected_cpu.clone();
    expected_directml.extend(["ort/DirectML.dll", GPU_LICENSE]);
    expected_directml.sort_unstable();
    assert_eq!(
        places(&directml),
        expected_directml,
        "directml: and the GPU library and its license"
    );
    let library = directml
        .files
        .iter()
        .find(|file| file.vision_root_path == "ort/DirectML.dll")
        .expect("the GPU library");
    assert_eq!(
        (
            library.size,
            library.sha256.as_str(),
            library.component.as_str(),
            library.archive_path.as_deref(),
            library.license,
            library.pinned,
        ),
        (
            table.pin.directml.size,
            table.pin.directml.sha256,
            table.pin.directml.tool_source,
            Some(table.pin.directml.archive_path),
            false,
            true,
        ),
        "the library with the runtime pin's values"
    );
    let gpu_license = directml
        .files
        .iter()
        .find(|file| file.vision_root_path == GPU_LICENSE)
        .expect("the GPU license");
    assert!(gpu_license.license && gpu_license.pinned, "{gpu_license:?}");
    assert_eq!(gpu_license.component, GPU);
    for manifest in [&cpu, &directml] {
        assert_eq!(manifest.models.len(), 1, "{:?}", manifest.models);
        let model = &manifest.models[0];
        assert_eq!(
            (model.model_ref.as_str(), model.family.as_str()),
            (MODEL, "ppocr-ctc")
        );
        assert_eq!(model.files, [DETECTOR, RECOGNIZER, DICTIONARY]);
        assert_eq!(
            model.model_sha256,
            model_identity(&content(DICTIONARY)),
            "the identity from the three pins"
        );
    }

    // The report line, with no address in it.
    let reported = vision_report::run_with(
        &arguments(&["vision", "manifest", "--provider", "directml"]),
        table,
        vision_report::BOUNDS,
    )
    .unwrap_or_else(|error| panic!("vision manifest answers: {}", error.outcome()));
    let (outcome, developer, line) = report_line(
        vision_report::SCHEMA_VERSION,
        SharedCode::SerializationFailed,
        Ok(reported),
        Detail::Full,
    );
    assert!(outcome.is(ActingdCode::VisionManifestReported), "{outcome}");
    assert!(developer.is_none(), "{developer:?}");
    assert!(
        !line.contains("://"),
        "the manifest names no address: {line}"
    );
    let line: Value = serde_json::from_str(&line).expect("one JSON line");
    assert_eq!(line["schema_version"], "actingcommand.actingd.vision.v1");
    assert_eq!(
        line["outcome"]["code"], "vision_manifest_reported",
        "{line}"
    );
    assert_eq!(
        line["report"],
        serde_json::to_value(&directml).expect("the manifest"),
        "{line}"
    );

    // The pin table this build carries: the library is the code pin's, for directml only.
    let carried = PinTable {
        registry: config::ac_adb::TOOL_SOURCES_PIN,
        pin: &DIRECTML_RUNTIME_PIN,
    };
    let carried_directml = vision_report::manifest(carried, OnnxExecutionProvider::Directml)
        .unwrap_or_else(|error| panic!("this build's directml manifest: {}", error.outcome()));
    let carried_cpu = vision_report::manifest(carried, OnnxExecutionProvider::Cpu)
        .unwrap_or_else(|error| panic!("this build's cpu manifest: {}", error.outcome()));
    let library = carried_directml
        .files
        .iter()
        .find(|file| file.vision_root_path == "ort/DirectML.dll")
        .expect("this build's GPU library");
    assert_eq!(
        (library.size, library.sha256.as_str()),
        (
            DIRECTML_RUNTIME_PIN.directml.size,
            DIRECTML_RUNTIME_PIN.directml.sha256
        ),
        "the code pin"
    );
    assert!(
        carried_cpu
            .files
            .iter()
            .all(|file| file.vision_root_path != "ort/DirectML.dll"),
        "{:?}",
        places(&carried_cpu)
    );
    assert!(
        carried_directml.files.iter().all(|file| file.pinned),
        "every file of this build's manifest is pinned: {:?}",
        places(&carried_directml)
    );
}

#[test]
fn s7_check_a_complete_vision_root_answers_every_file_ok_and_equal_identities() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("vision");
    for place in CPU_PLACES {
        write(&root, place, &content(place));
    }
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );
    let before = tree(&root);

    let line = check_line(&config);

    let report = &line["report"];
    assert_eq!(report["provider"], "cpu", "{report}");
    let canonical = fs::canonicalize(&root).expect("the canonical root");
    assert_eq!(
        report["vision_root"],
        Value::from(canonical.to_string_lossy().into_owned())
    );
    let states = states(report);
    assert_eq!(
        states.keys().map(String::as_str).collect::<Vec<_>>(),
        {
            let mut expected = CPU_PLACES.to_vec();
            expected.sort_unstable();
            expected
        },
        "{report}"
    );
    for (place, (state, cause)) in &states {
        assert_eq!(state, "ok", "{place}: {report}");
        assert!(cause.is_null(), "{place}: {report}");
    }
    let identity = model_identity(&content(DICTIONARY));
    assert_eq!(
        report["folders"],
        json!([{"model_ref": MODEL, "family": "ppocr-ctc", "layout": "flat", "asked": true,
                "identity": identity}]),
        "{report}"
    );
    assert_eq!(
        report["models"],
        json!([{"model_ref": MODEL, "family": "ppocr-ctc", "model_sha256": identity,
                "present": true, "identity": identity, "identity_matches": true}]),
        "{report}"
    );
    assert_eq!(tree(&root), before, "nothing written in the vision root");
    assert!(!temp.path().join("state").exists(), "no state root made");
}

#[cfg(windows)]
#[test]
fn s7_check_names_a_missing_a_changed_and_an_unreadable_file() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("vision");
    for place in CPU_PLACES {
        match place {
            // Missing.
            "ort/onnxruntime_providers_shared.dll" => {}
            // Of another hash: one byte changed, the size kept.
            "ort/onnxruntime.dll" => {
                let mut bytes = content(place);
                bytes[0] ^= 0x01;
                write(&root, place, &bytes);
            }
            _ => write(&root, place, &content(place)),
        }
    }
    // Unreadable: another handle holds it open and shares nothing.
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(at(&root, RUNTIME_LICENSE))
        .expect("the license held open");
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );

    let line = check_line(&config);

    let report = &line["report"];
    let states = states(report);
    let state = |place: &str| states.get(place).expect(place).0.as_str();
    assert_eq!(state("ort/onnxruntime_providers_shared.dll"), "missing");
    assert_eq!(state("ort/onnxruntime.dll"), "sha256_mismatch");
    assert_eq!(state(RUNTIME_LICENSE), "unreadable", "{report}");
    let cause = &states[RUNTIME_LICENSE].1;
    assert_eq!(
        cause["code"], "foreign_os_error",
        "the OS failure: {report}"
    );
    for place in [DETECTOR, RECOGNIZER, DICTIONARY] {
        assert_eq!(state(place), "ok", "{place}: {report}");
    }
    for place in [
        "ort/onnxruntime.dll",
        "ort/onnxruntime_providers_shared.dll",
    ] {
        assert!(states[place].1.is_null(), "{place}: no cause: {report}");
    }
    assert_eq!(report["models"][0]["identity_matches"], true, "{report}");
}

#[test]
fn s7_check_a_model_folder_of_another_identity_does_not_match() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("vision");
    let mut other_dictionary = content(DICTIONARY);
    other_dictionary[0] ^= 0x01;
    for place in CPU_PLACES {
        if place == DICTIONARY {
            write(&root, place, &other_dictionary);
        } else {
            write(&root, place, &content(place));
        }
    }
    // A classifier folder the manifest does not name, and a folder that holds no model.
    write(
        &root,
        "models/neutral.classifier/model.onnx",
        b"neutral classifier",
    );
    fs::create_dir_all(at(&root, "models/neutral.empty")).expect("an empty folder");
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );

    let line = check_line(&config);

    let report = &line["report"];
    let states = states(report);
    assert_eq!(states[DICTIONARY].0, "sha256_mismatch", "{report}");
    let manifest_identity = model_identity(&content(DICTIONARY));
    let folder_identity = model_identity(&other_dictionary);
    assert_ne!(manifest_identity, folder_identity, "setup");
    let folders = report["folders"].as_array().expect("the folders");
    assert_eq!(folders.len(), 3, "{report}");
    // Fix 1: a folder neither the manifest nor a `--model` names is listed, not read.
    assert_eq!(
        folders[0],
        json!({"model_ref": "neutral.classifier", "family": "onnx-classify", "asked": false}),
        "{report}"
    );
    assert_eq!(folders[1]["model_ref"], "neutral.empty", "{report}");
    assert!(folders[1]["invalid"].is_string(), "{report}");
    assert_eq!(
        folders[2],
        json!({"model_ref": MODEL, "family": "ppocr-ctc", "layout": "flat", "asked": true,
               "identity": folder_identity}),
        "{report}"
    );
    assert_eq!(
        report["models"],
        json!([{"model_ref": MODEL, "family": "ppocr-ctc", "model_sha256": manifest_identity,
                "present": true, "identity": folder_identity, "identity_matches": false}]),
        "{report}"
    );
}

#[test]
fn s7_check_a_vision_root_that_cannot_be_resolved_relays_the_existing_cause() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let absent = temp.path().join("absent-vision");
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": absent}),
    );

    let refused = vision_report::run_with(
        &arguments(&[
            "vision",
            "check",
            "--config",
            config.to_str().expect("a UTF-8 path"),
        ]),
        table(),
        vision_report::BOUNDS,
    )
    .expect_err("a root that does not exist is refused");

    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "vision_root_unavailable",
        "{outcome}"
    );
    assert_eq!(
        outcome.value("stage").and_then(Value::as_str),
        Some("config_vision"),
        "{outcome}"
    );
    let envelope = serde_json::to_value(outcome.envelope(Detail::Full)).expect("the envelope");
    assert_eq!(envelope["causes"][0]["relation"], "caused_by", "{envelope}");
    assert_eq!(
        envelope["causes"][0]["code"], "foreign_os_error",
        "{envelope}"
    );
    assert!(!absent.exists(), "nothing made");
    assert!(!temp.path().join("state").exists(), "no state root made");
}

#[test]
fn s7_check_compares_the_size_first_and_reads_no_manifest_file_past_its_pin() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    // The reader hashes no byte past its limit: of ten bytes, a limit of four hashes four.
    let longer = temp.path().join("ten-bytes.bin");
    fs::write(&longer, b"0123456789").expect("a file of ten bytes");
    let mut file = fs::File::open(&longer).expect("open it");
    let hashed = vision_report::hash_at_most(&mut file, 4).expect("hash it");
    assert_eq!(hashed, (sha256_hex(b"0123"), 4), "four bytes, no more");

    // A manifest file one byte longer than its pin is a mismatch and is never opened: on Windows
    // another handle holds it open without sharing, so opening it would answer `unreadable`.
    let root = temp.path().join("vision");
    for place in CPU_PLACES {
        write(&root, place, &content(place));
    }
    let library = "ort/onnxruntime.dll";
    let mut grown = content(library);
    grown.push(b'!');
    write(&root, library, &grown);
    #[cfg(windows)]
    let _held = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(at(&root, library))
            .expect("the library held open")
    };
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );

    let line = check_line(&config);

    let report = &line["report"];
    let states = states(report);
    assert_eq!(states[library].0, "sha256_mismatch", "{report}");
    assert!(
        states[library].1.is_null(),
        "not opened, no cause: {report}"
    );
    assert_eq!(report["models"][0]["identity_matches"], true, "{report}");
}

#[test]
fn s7_check_hashes_only_the_folders_it_is_asked_about_within_its_bounds() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("vision");
    for place in CPU_PLACES {
        write(&root, place, &content(place));
    }
    // Two classifier folders the manifest does not name; the rescue row asks about the first.
    let classifier = vec![b'c'; 100];
    write(&root, "models/neutral.classifier/model.onnx", &classifier);
    write(
        &root,
        "models/neutral.unasked/model.onnx",
        b"neutral unasked classifier",
    );
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );
    let canonical = fs::canonicalize(&root).expect("the canonical root");
    let classifier_path = at(&canonical, "models/neutral.classifier/model.onnx")
        .to_string_lossy()
        .into_owned();
    let pinned = CPU_PLACES.into_iter().map(length_of).sum::<u64>();
    let identity = model_identity(&content(DICTIONARY));
    let folder = |line: &Value, model_ref: &str| {
        line["report"]["folders"]
            .as_array()
            .expect("the folders")
            .iter()
            .find(|folder| folder["model_ref"] == model_ref)
            .cloned()
            .unwrap_or_else(|| panic!("the folder {model_ref}: {line}"))
    };
    let asked = ["--model", "neutral.classifier"];

    // Within the bounds: the asked folder's identity; the other listed by name, not read.
    let line = check_line_with(&config, &asked, vision_report::BOUNDS);
    assert_eq!(
        folder(&line, "neutral.classifier"),
        json!({"model_ref": "neutral.classifier", "family": "onnx-classify", "asked": true,
               "identity": sha256_hex(&classifier)}),
        "{line}"
    );
    assert_eq!(
        folder(&line, "neutral.unasked"),
        json!({"model_ref": "neutral.unasked", "family": "onnx-classify", "asked": false}),
        "{line}"
    );
    assert_eq!(folder(&line, MODEL)["identity"], identity, "{line}");

    // A file over the file bound is named and not read.
    let line = check_line_with(
        &config,
        &asked,
        Bounds {
            file_bytes: 99,
            total_bytes: vision_report::BOUNDS.total_bytes,
        },
    );
    assert_eq!(
        folder(&line, "neutral.classifier"),
        json!({"model_ref": "neutral.classifier", "family": "onnx-classify", "asked": true,
               "identity": null, "over_bound": {"path": classifier_path, "size": 100,
               "bound": "file", "bound_bytes": 99}}),
        "{line}"
    );
    assert_eq!(folder(&line, MODEL)["identity"], identity, "{line}");

    // A file over what the total bound leaves after the manifest's files is named and not read.
    let total = pinned + 99;
    let line = check_line_with(
        &config,
        &asked,
        Bounds {
            file_bytes: vision_report::BOUNDS.file_bytes,
            total_bytes: total,
        },
    );
    assert_eq!(
        folder(&line, "neutral.classifier"),
        json!({"model_ref": "neutral.classifier", "family": "onnx-classify", "asked": true,
               "identity": null, "over_bound": {"path": classifier_path, "size": 100,
               "bound": "total", "bound_bytes": total}}),
        "{line}"
    );
    assert_eq!(
        folder(&line, MODEL)["identity"],
        identity,
        "the manifest's files, read once within the total, give the model's identity: {line}"
    );
}

#[test]
fn s7_check_reads_a_file_exactly_at_each_bound_and_none_one_byte_past_it() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("vision");
    for place in CPU_PLACES {
        write(&root, place, &content(place));
    }
    // A classifier folder of one file of 100 bytes, which the check is asked about.
    let classifier = vec![b'c'; 100];
    write(&root, "models/neutral.classifier/model.onnx", &classifier);
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );
    let canonical = fs::canonicalize(&root).expect("the canonical root");
    let classifier_path = at(&canonical, "models/neutral.classifier/model.onnx")
        .to_string_lossy()
        .into_owned();
    let pinned = CPU_PLACES.into_iter().map(length_of).sum::<u64>();
    let identity = model_identity(&content(DICTIONARY));
    let asked = ["--model", "neutral.classifier"];
    let within = |file_bytes: u64, total_bytes: u64| {
        let line = check_line_with(
            &config,
            &asked,
            Bounds {
                file_bytes,
                total_bytes,
            },
        );
        assert_eq!(
            line["report"]["models"][0]["identity"], identity,
            "the manifest's model identified within ({file_bytes}, {total_bytes}): {line}"
        );
        line["report"]["folders"]
            .as_array()
            .expect("the folders")
            .iter()
            .find(|folder| folder["model_ref"] == "neutral.classifier")
            .cloned()
            .unwrap_or_else(|| panic!("the classifier folder: {line}"))
    };
    let hashed = json!({"model_ref": "neutral.classifier", "family": "onnx-classify",
                        "asked": true, "identity": sha256_hex(&classifier)});
    let over = |bound: &str, bound_bytes: u64| {
        json!({"model_ref": "neutral.classifier", "family": "onnx-classify", "asked": true,
               "identity": null, "over_bound": {"path": classifier_path, "size": 100,
               "bound": bound, "bound_bytes": bound_bytes}})
    };
    let total = vision_report::BOUNDS.total_bytes;
    let file = vision_report::BOUNDS.file_bytes;

    // The file bound: a file of exactly it is hashed; one byte more is over it and not read.
    assert_eq!(within(100, total), hashed, "exactly the file bound");
    assert_eq!(within(99, total), over("file", 99), "one byte past it");

    // The total bound: a file of exactly what it leaves after the manifest's files is hashed; one
    // byte more is over it and not read.
    assert_eq!(within(file, pinned + 100), hashed, "exactly what is left");
    assert_eq!(
        within(file, pinned + 99),
        over("total", pinned + 99),
        "one byte past it"
    );

    // The manifest's files against the total bound: pins of exactly it are read (and leave
    // nothing for the folder's file); one byte less of total is a defect of the build.
    assert_eq!(
        within(file, pinned),
        over("total", pinned),
        "the manifest's files fill the total exactly"
    );
    let refused = vision_report::run_with(
        &arguments(&[
            "vision",
            "check",
            "--config",
            config.to_str().expect("a UTF-8 path"),
            "--model",
            "neutral.classifier",
        ]),
        table(),
        Bounds {
            file_bytes: file,
            total_bytes: pinned - 1,
        },
    )
    .expect_err("pins one byte past the total are refused");
    assert_eq!(
        refused.outcome().code().as_str(),
        "serialization_failed",
        "{}",
        refused.outcome()
    );
}

#[test]
fn s7_check_takes_64_model_names_and_refuses_a_65th_or_a_bad_name_before_reading() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("vision");
    for place in CPU_PLACES {
        write(&root, place, &content(place));
    }
    let config = config(
        temp.path(),
        &json!({"execution_provider": "cpu", "root": root}),
    );
    // 64 names, the last of the longest a model folder may have (255 bytes); no folder has any.
    let mut names = (0..63)
        .map(|index| format!("neutral.absent.{index:02}"))
        .collect::<Vec<_>>();
    names.push(format!("neutral.{}", "n".repeat(247)));
    assert_eq!(names[63].len(), 255, "setup");

    let line = check_line_with(&config, &model_words(&names), vision_report::BOUNDS);

    // In name order: the 63 `neutral.absent.…`, the long `neutral.nn…`, then `neutral.ocr`.
    let mut expected = names
        .iter()
        .map(|name| json!({"model_ref": name, "asked": true, "absent": true}))
        .collect::<Vec<_>>();
    let identity = model_identity(&content(DICTIONARY));
    let model_folder = json!({"model_ref": MODEL, "family": "ppocr-ctc", "layout": "flat",
                              "asked": true, "identity": identity});
    expected.push(model_folder);
    assert_eq!(
        line["report"]["folders"],
        Value::Array(expected),
        "each name asked about that no folder has is listed asked and absent, in name order: \
         {line}"
    );
    assert_eq!(
        line["report"]["models"][0]["present"], true,
        "the manifest's model, whose folder is there, present: {line}"
    );

    // Refused before anything is read: the configuration named does not exist, so a check that
    // read it would refuse it at stage config_load.
    let missing = temp.path().join("missing.config.json");
    let refused = |names: &[String]| {
        let mut words = vec![
            "vision",
            "check",
            "--config",
            missing.to_str().expect("a UTF-8 path"),
        ];
        words.extend(model_words(names));
        let error = vision_report::run_with(&arguments(&words), table(), vision_report::BOUNDS)
            .expect_err("refused");
        error.outcome()
    };
    let mut surplus = names.clone();
    surplus.push("neutral.absent.64".to_owned());
    let too_long = format!("neutral.{}", "n".repeat(248));
    assert_eq!(too_long.len(), 256, "setup");
    for (why, asked) in [
        ("a 65th name", surplus),
        ("a name of 256 bytes", vec![too_long]),
        ("..", vec!["..".to_owned()]),
        (".", vec![".".to_owned()]),
        ("a blank name", vec![" ".to_owned()]),
        ("a slash", vec!["neutral/ocr".to_owned()]),
        ("a backslash", vec!["neutral\\ocr".to_owned()]),
        ("a colon", vec!["C:".to_owned()]),
        ("a control character", vec!["neutral\u{7}ocr".to_owned()]),
    ] {
        let outcome = refused(asked.as_slice());
        assert_eq!(
            outcome.code().as_str(),
            "invalid_arguments",
            "{why}: {outcome}"
        );
        assert!(
            outcome.value("stage").is_none(),
            "{why}: refused before the configuration is read: {outcome}"
        );
    }
    assert!(!missing.exists(), "nothing made");
    assert!(!temp.path().join("state").exists(), "no state root made");
}
