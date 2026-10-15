// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-4 gate tests of actingd (test plan v2.2 §3.7 row A-4; inventory-apps.md; every
// expected value comes from the cited requirement, not from what the code does today).
//
// G7 (apps key 3, "Resource-package admission at start and in check-config"): every configured
// `resource_package` is admitted by `validate_resource_packages`, which check-config and the
// daemon's start share (contracts/actingd-check-config.md, "Instance resource package", Workflow
// #336 PKG-S1). The path chooses the reading (contracts/package-reference.md, "Content-directory
// admission"): a directory, a `.zip` container or a `.json` container is admitted structurally
// and echoed as `{path, kind}`; a missing path is `resource_package_missing`; a package the loader
// refuses is `resource_package_invalid` with the loader's code as its link; no digest is computed
// or compared and no name is read as an identity, so a directory named like a digest is admitted
// like any other. check-config resolves a relative path against the configuration file's
// directory, echoes the admitted package on its instance, and adds no package entry to
// `not_checked`. The plan's digest-named rows (test-ci-plan.md:202, req-basis-ha.md A-4 G7)
// predate PKG-S1, which retired them (option 丙); these rows follow the contract as it stands.
// The packages are the shared EXEC-I6 fixture (`tests/support/exec_i6.rs`).

use crate::config::validate_resource_packages;
use actingcommand_contract::{
    IdentifierIssuer, InstanceResourcePackage, InstanceResourcePackageKind,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

#[allow(dead_code)]
#[path = "../../../tests/support/exec_i6.rs"]
mod exec_i6;

// The instance that names the package.
const ALIAS: &str = "neutral.packaged";
// The package: one EXEC-I6 `linear_steps` package without a prerequisite.
const PACKAGE_ID: &str = "neutral.packaged.task";
// Every file of the package as `exec_i6::write_pack` writes it.
const PACKAGE_FILES: [&str; 3] = [
    "control.json",
    "resources/operations/resources.json",
    "resources/operations/task/task.json",
];

// G7: a source package directory, the same package as a ZIP container and as the single JSON
// container, and a directory whose name has the form of a SHA-256 digest that its content does
// not have, are each admitted by their path with their kind.
#[test]
fn gate_382_a4_g7_a_package_directory_zip_or_json_container_is_admitted_by_its_path() {
    let temp = TempDir::new().expect("tempdir");
    let directory = package_directory(temp.path(), "neutral-packaged");
    let zip = temp.path().join("neutral-packaged.zip");
    zip_container(&directory, &zip);
    let json = temp.path().join("neutral-packaged.json");
    json_container(&directory, &json);
    let digest_named = package_directory(
        temp.path(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );

    for (path, kind) in [
        (&directory, InstanceResourcePackageKind::Directory),
        (&zip, InstanceResourcePackageKind::File),
        (&json, InstanceResourcePackageKind::File),
        (&digest_named, InstanceResourcePackageKind::Directory),
    ] {
        let admitted = match validate_resource_packages(&configured(path)) {
            Ok(admitted) => admitted,
            Err(rejection) => panic!("{} is admitted, not refused: {rejection}", path.display()),
        };
        assert_eq!(
            admitted,
            BTreeMap::from([(
                ALIAS.to_owned(),
                InstanceResourcePackage {
                    path: path.to_string_lossy().into_owned(),
                    kind,
                },
            )]),
            "{}",
            path.display()
        );
    }
}

// G7: a path that does not exist is `resource_package_missing`, naming the instance and the path.
#[test]
fn gate_382_a4_g7_a_missing_package_is_resource_package_missing() {
    let temp = TempDir::new().expect("tempdir");
    let absent = temp.path().join("neutral-absent");
    let rejection = match validate_resource_packages(&configured(&absent)) {
        Ok(admitted) => panic!("a missing package is refused, not admitted: {admitted:?}"),
        Err(rejection) => rejection,
    };
    assert_eq!(rejection.code.as_str(), "resource_package_missing");
    let outcome = rejection.outcome();
    assert_eq!(outcome.code().as_str(), "resource_package_missing");
    assert_eq!(outcome.value("alias"), Some(&json!(ALIAS)), "{outcome}");
    assert!(
        outcome
            .value("path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.ends_with("neutral-absent")),
        "{outcome}"
    );
}

// G7: a package directory that holds derived output (`resources/manifest.json`) is refused by the
// loader (`source_contains_derived_output`, contracts/package-reference.md), so the package is
// `resource_package_invalid` with the loader's code as its link.
#[test]
fn gate_382_a4_g7_a_package_the_loader_refuses_is_resource_package_invalid_with_its_code() {
    let temp = TempDir::new().expect("tempdir");
    let directory = package_directory(temp.path(), "neutral-derived");
    fs::write(
        directory.join("resources").join("manifest.json"),
        serde_json::to_vec(&json!({"entry_task_id": "task"})).expect("the manifest JSON"),
    )
    .expect("write the derived manifest");
    let rejection = match validate_resource_packages(&configured(&directory)) {
        Ok(admitted) => panic!("a refused package is not admitted: {admitted:?}"),
        Err(rejection) => rejection,
    };
    assert_eq!(rejection.code.as_str(), "resource_package_invalid");
    let outcome = rejection.outcome();
    assert_eq!(outcome.value("alias"), Some(&json!(ALIAS)), "{outcome}");
    assert_eq!(
        outcome
            .causes()
            .iter()
            .map(|link| link.outcome().code().as_str())
            .collect::<Vec<_>>(),
        ["source_contains_derived_output"],
        "{outcome}"
    );
}

// G7: check-config resolves a relative `resource_package` against the configuration file's
// directory, admits it and echoes `{path, kind}` on its instance, and `not_checked` names no
// package.
#[test]
fn gate_382_a4_g7_check_config_echoes_the_admitted_package_and_adds_no_package_to_not_checked() {
    let temp = TempDir::new().expect("tempdir");
    let packages = temp.path().join("packages");
    fs::create_dir(&packages).expect("create the package folder");
    let directory = package_directory(&packages, "neutral-packaged");
    let instance_id = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    let config = temp.path().join("actingd.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": temp.path().join("state"),
            "bind_host": "127.0.0.1",
            "bind_port": 0,
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": [{
                "alias": ALIAS,
                "instance_id": instance_id.transport(),
                "fixture_backend": {
                    "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                    "max_inputs": 1
                },
                "resource_package": Path::new("packages").join("neutral-packaged")
            }]
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");

    let checked = crate::check_config::run(vec![
        OsString::from("check-config"),
        OsString::from("--config"),
        config.into_os_string(),
    ])
    .unwrap_or_else(|error| panic!("check-config passes: {error}"));

    let report = &checked.data["report"];
    let instance = report["instances"]
        .as_array()
        .and_then(|instances| {
            instances
                .iter()
                .find(|instance| instance["alias"] == json!(ALIAS))
        })
        .unwrap_or_else(|| panic!("the instance is reported: {report}"));
    assert_eq!(
        instance["resource_package"],
        json!({"path": directory.to_string_lossy(), "kind": "directory"}),
        "{report}"
    );
    let not_checked = report["not_checked"]
        .as_array()
        .unwrap_or_else(|| panic!("not_checked is a list: {report}"));
    assert!(
        not_checked
            .iter()
            .filter_map(Value::as_str)
            .all(|entry| !entry.contains("package")),
        "{report}"
    );
}

// --- G7 fixtures ---

// The configuration's resource packages: the one instance's.
fn configured(path: &Path) -> BTreeMap<String, PathBuf> {
    BTreeMap::from([(ALIAS.to_owned(), path.to_path_buf())])
}

// Writes the package as a source package directory `<parent>/<name>`.
fn package_directory(parent: &Path, name: &str) -> PathBuf {
    let directory = parent.join(name);
    exec_i6::write_pack(
        &directory,
        &exec_i6::I6Pack {
            package_id: PACKAGE_ID,
            prerequisite: None,
            resolution: exec_i6::RESOLUTION,
        },
    );
    directory
}

// The package's files, by their `/`-separated relative paths.
fn package_files(directory: &Path) -> Vec<(&'static str, Vec<u8>)> {
    PACKAGE_FILES
        .iter()
        .map(|name| {
            let path = name
                .split('/')
                .fold(directory.to_path_buf(), |parent, segment| {
                    parent.join(segment)
                });
            (*name, fs::read(&path).expect("read a package file"))
        })
        .collect()
}

// The package as a ZIP container: its files at the archive's root.
fn zip_container(directory: &Path, out: &Path) {
    let mut zip = ZipWriter::new(File::create(out).expect("create the ZIP container"));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in package_files(directory) {
        zip.start_file(name, options).expect("start a ZIP entry");
        zip.write_all(&bytes).expect("write a ZIP entry");
    }
    zip.finish().expect("finish the ZIP container");
}

// The package as the single JSON container (`actingcommand.package.content-json.v1`).
fn json_container(directory: &Path, out: &Path) {
    let files = package_files(directory)
        .into_iter()
        .map(|(name, bytes)| {
            (
                name.to_owned(),
                Value::String(String::from_utf8(bytes).expect("a UTF-8 package file")),
            )
        })
        .collect::<Map<String, Value>>();
    fs::write(
        out,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.package.content-json.v1",
            "files": files
        }))
        .expect("the container JSON"),
    )
    .expect("write the JSON container");
}
