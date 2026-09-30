// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #288 A2a evidence on CI, through the real actingd binary.
// For a neutral source fixture and for the default package of the public umbrella release
// asset (unsealed): check-config and daemon startup on a digest-named `resource_package`
// directory; E4 (a changed JSON value, a removed image); a name that differs from the content;
// a directory with another name (not checked, as before); E7 (a ZIP `resource_package`, as
// before); `startup_package` echoing the content-directory reference for a digest-named locator.

use super::*;
use std::collections::BTreeMap;
use std::process::ExitStatus;

const RELEASE: &str =
    "https://github.com/HS7097/ActingCommand/releases/download/build-r651fead-uf5cb9d9";
const BUNDLE_SUFFIX: &str = "-bundle-3ff697b.zip";
const DIRECTORY_NOT_CHECKED: &str = "resource_package_directory_declarations";

fn report(line: impl AsRef<str>) {
    // Direct handle writes are not captured by the test harness, so they reach the CI log.
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "ONE-OFF-A2A {}", line.as_ref());
}

fn digest_of(files: &BTreeMap<String, Vec<u8>>) -> String {
    actingcommand_contract::content_directory_digest(files.iter().map(|(path, bytes)| {
        let mut hash = [0_u8; 32];
        hash.copy_from_slice(&Sha256::digest(bytes));
        (path.as_str(), hash)
    }))
}

fn write_tree(root: &Path, files: &BTreeMap<String, Vec<u8>>) {
    for (path, bytes) in files {
        let target = root.join(path);
        fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
        fs::write(target, bytes).expect("write file");
    }
}

fn read_zip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("open zip");
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("zip entry");
        if file.is_dir() {
            continue;
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content).expect("read zip entry");
        entries.insert(file.name().to_owned(), content);
    }
    entries
}

fn download(url: &str) -> Vec<u8> {
    let output = Command::new("curl")
        .args(["-L", "-f", "-sS", url])
        .output()
        .expect("run curl");
    assert!(
        output.status.success(),
        "download {url}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fixture_instance(root: &Path, resource_package: &Path) -> PathBuf {
    let state_root = root.join("state");
    fs::create_dir_all(&state_root).expect("state root");
    let config = root.join("actingd.json");
    let value = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": PROCESS_TEST_SALT,
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id(),
            "fixture_backend": {
                "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                "max_inputs": 1
            },
            "resource_package": resource_package
        }]
    });
    fs::write(
        &config,
        serde_json::to_vec_pretty(&value).expect("config json"),
    )
    .expect("config");
    config
}

fn check_config(config: &Path) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_actingcommand-actingd"))
        .args([
            "check-config",
            "--config",
            config.to_str().expect("config path"),
        ])
        .output()
        .expect("run check-config");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "check-config JSON: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), report)
}

/// Starts the daemon and waits until it is ready (then stops it) or exits by itself.
fn run_daemon(config: &Path, state_root: &Path) -> (bool, Option<ExitStatus>, String) {
    let child = start_actingd(config);
    let mut child = ChildGuard(child);
    let started = Instant::now();
    loop {
        if let Ok(bytes) = fs::read(state_root.join(RUNTIME_INFO_FILE))
            && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&bytes)
            && info.pid() == child.0.id()
        {
            child.0.kill().expect("stop actingd");
            child.0.wait().expect("wait actingd");
            return (true, None, String::new());
        }
        if let Some(status) = child.0.try_wait().expect("process state") {
            let mut stderr = String::new();
            if let Some(pipe) = child.0.stderr.as_mut() {
                pipe.read_to_string(&mut stderr).expect("read stderr");
            }
            return (false, Some(status), stderr.trim().to_owned());
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "actingd neither ready nor exited"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn instance_entry(report: &Value) -> Value {
    report["instances"][0]["resource_package"].clone()
}

/// check-config and startup for one resource package; returns the check-config report.
fn exercise(label: &str, case: &str, root: &Path, package: &Path) -> (bool, Value) {
    let config = fixture_instance(root, package);
    let (ok, checked) = check_config(&config);
    report(format!(
        "{label} {case}: check-config ok={ok} status={} resource_package={} not_checked={} error={}",
        checked["status"],
        instance_entry(&checked),
        checked["not_checked"],
        checked["error"]
    ));
    let (ready, status, stderr) = run_daemon(&config, &root.join("state"));
    report(format!(
        "{label} {case}: daemon ready={ready} exit={status:?} runtime_info={} stderr={stderr}",
        root.join("state").join(RUNTIME_INFO_FILE).exists()
    ));
    assert_eq!(ok, ready, "{label} {case}: check-config and startup agree");
    (ok, checked)
}

fn exercise_fixture(label: &str, root: &Path, files: &BTreeMap<String, Vec<u8>>, zip: &[u8]) {
    let digest = digest_of(files);
    report(format!("{label}: {} files, digest {digest}", files.len()));

    // A digest-named directory: admitted in full by check-config and by startup.
    let base = root.join("admitted");
    let package = base.join("packages").join(&digest);
    write_tree(&package, files);
    let (ok, checked) = exercise(label, "digest-named directory", &base, &package);
    if ok {
        assert_eq!(instance_entry(&checked)["kind"], "directory");
        assert!(
            !checked["not_checked"]
                .as_array()
                .expect("not_checked")
                .iter()
                .any(|entry| entry == DIRECTORY_NOT_CHECKED)
        );
    }

    // E4 (a): one JSON value changed and saved in the installed directory.
    let mut changed = files.clone();
    let mut control: Value =
        serde_json::from_slice(&changed["control.json"]).expect("control json");
    let steps = control["max_steps"].as_u64().expect("max_steps");
    control["max_steps"] = json!(steps + 1);
    changed.insert(
        "control.json".to_owned(),
        serde_json::to_vec_pretty(&control).expect("control"),
    );
    // E4 (b): one image removed, as after an interrupted copy.
    let mut missing = files.clone();
    let image = missing
        .keys()
        .find(|path| path.ends_with(".png"))
        .cloned()
        .expect("an image");
    missing.remove(&image);
    let other = if digest.starts_with('0') {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    };
    for (case, tree, name, loader_code) in [
        (
            "E4a changed control.json max_steps",
            &changed,
            digest.as_str(),
            "content_directory_digest_mismatch",
        ),
        (
            "E4b removed image",
            &missing,
            digest.as_str(),
            "content_directory_digest_mismatch",
        ),
        // The reference is the directory's own name, so another digest name refuses the
        // content as a digest mismatch.
        (
            "name of another digest",
            files,
            other.as_str(),
            "content_directory_digest_mismatch",
        ),
    ] {
        let case_root = root.join(case.split(' ').next().expect("case").to_lowercase());
        let package = case_root.join("packages").join(name);
        write_tree(&package, tree);
        let (ok, checked) = exercise(label, case, &case_root, &package);
        assert!(!ok, "{label} {case} refused");
        assert_eq!(checked["error"]["code"], "resource_package_invalid");
        assert_eq!(checked["error"]["stage"], "resource_package");
        assert_eq!(checked["error"]["detail"]["loader_code"], loader_code);
    }

    // A directory with another name: existence only, listed as not checked (unchanged).
    let author_root = root.join("author");
    let author = author_root.join("work");
    write_tree(&author, files);
    let (ok, checked) = exercise(label, "non-digest directory", &author_root, &author);
    assert!(ok);
    assert!(
        checked["not_checked"]
            .as_array()
            .expect("not_checked")
            .iter()
            .any(|entry| entry == DIRECTORY_NOT_CHECKED)
    );

    // E7: a ZIP resource package, admitted as before.
    let zip_root = root.join("zip");
    fs::create_dir_all(&zip_root).expect("zip root");
    let zip_path = zip_root.join("package.zip");
    fs::write(&zip_path, zip).expect("write zip");
    let (ok, checked) = exercise(label, "E7 ZIP file", &zip_root, &zip_path);
    report(format!(
        "{label} E7 ZIP file: admitted={ok} kind={}",
        instance_entry(&checked)["kind"]
    ));

    // startup_package on a device entry (check-config only; nothing is started or probed).
    let directory_echo = startup_echo(&root.join("startup-directory"), &package, &digest);
    report(format!(
        "{label} startup_package directory: {directory_echo}"
    ));
    assert_eq!(
        directory_echo["expected_sha256"],
        json!({
            "schema_version": "actingcommand.package.content-directory.v1",
            "sha256": digest
        })
    );
    let zip_echo = startup_echo(&root.join("startup-zip"), &zip_path, &hex(zip));
    report(format!("{label} startup_package ZIP: {zip_echo}"));
    assert_eq!(zip_echo["expected_sha256"], json!(hex(zip)));
}

/// check-config of one device entry declaring `startup_package`; its echoed declaration.
fn startup_echo(root: &Path, package: &Path, expected_sha256: &str) -> Value {
    fs::create_dir_all(root).expect("startup root");
    let config = root.join("actingd.json");
    let value = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": root.join("state"),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": PROCESS_TEST_SALT,
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id(),
            "application_id": "neutral.application",
            "adb_path": "adb",
            "touch_backend": "maatouch",
            "capture_backend": "adb",
            "push_touch_tool": false,
            "startup_package": {"package": package, "expected_sha256": expected_sha256}
        }]
    });
    fs::write(&config, serde_json::to_vec_pretty(&value).expect("json")).expect("config");
    let (ok, checked) = check_config(&config);
    assert!(ok, "{checked}");
    checked["instances"][0]["startup_package"].clone()
}

fn neutral_source_fixture() -> BTreeMap<String, Vec<u8>> {
    let png: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9,
        251, 3, 253, 167, 89, 75, 221, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let mut files = BTreeMap::new();
    files.insert(
        "control.json".to_owned(),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.test.return_home",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 1280, "height": 720},
            "entry_task_id": "return_home",
            "capture_interval_ms": 50,
            "step_timeout_ms": 50,
            "timeout_ms": 1000,
            "max_steps": 2
        }))
        .expect("control"),
    );
    files.insert(
        "resources/operations/resources.json".to_owned(),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "1.0",
            "resources": [],
            "resource_count": 0
        }))
        .expect("resources"),
    );
    files.insert(
        "resources/operations/return_home/assets/HOME.png".to_owned(),
        png.to_vec(),
    );
    files.insert(
        "resources/operations/return_home/task.json".to_owned(),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.3",
            "task_id": "return_home",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "x-fixture",
            "goal": "neutral fixture",
            "coordinate_space": {"width": 1280, "height": 720},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 20.0},
            "anchors": [{
                "id": "home",
                "template": "assets/HOME.png",
                "region": {"mode": "rect", "rect": {"x": 20, "y": 20, "width": 30, "height": 30}},
                "threshold": 0.8,
                "color_check": null
            }],
            "entry_page": "home",
            "target_page": "home",
            "operations": [{
                "id": "home_noop",
                "purpose": "neutral fixture",
                "from": "home",
                "to": null,
                "click": {"kind": "point", "x": 1, "y": 1},
                "verify_template": null,
                "guard": {
                    "page_id": "home",
                    "target_id": "page/home",
                    "expected_rect": {"x": 1, "y": 1, "width": 1, "height": 1},
                    "verify_template": "assets/HOME.png"
                },
                "consumes": [],
                "produces": []
            }]
        }))
        .expect("task"),
    );
    files
}

/// The default package of the public umbrella release asset, unsealed: the sealed files minus
/// the six derived documents, and the sealed ZIP itself.
fn released_default_package() -> (String, BTreeMap<String, Vec<u8>>, Vec<u8>) {
    let sums = String::from_utf8(download(&format!("{RELEASE}/SHA256SUMS"))).expect("sums");
    let (expected, name) = sums
        .lines()
        .filter_map(|line| line.split_once("  "))
        .find(|(_, name)| name.ends_with(BUNDLE_SUFFIX))
        .expect("bundle asset listed");
    let asset = download(&format!("{RELEASE}/{name}"));
    assert_eq!(hex(&asset), expected, "asset hash");
    let outer = read_zip(&asset);
    let applications: Value =
        serde_json::from_slice(&outer["applications.json"]).expect("applications");
    let index: Value = serde_json::from_slice(&outer["bundle.json"]).expect("index");
    let default = applications["servers"]
        .as_object()
        .expect("servers")
        .values()
        .find_map(|server| server["default_package_id"].as_str())
        .expect("default package")
        .to_owned();
    let pack = index["packs"]
        .as_array()
        .expect("packs")
        .iter()
        .find(|pack| pack["package_id"] == default.as_str())
        .expect("default pack");
    let zip = outer[pack["path"].as_str().expect("path")].clone();
    let mut files = read_zip(&zip);
    let control: Value = serde_json::from_slice(&files["control.json"]).expect("control");
    let stem = format!(
        "{}.{}",
        control["game"].as_str().expect("game"),
        control["server"].as_str().expect("server")
    );
    for path in [
        "resources/manifest.json".to_owned(),
        format!("resources/recognition/{stem}.pack.json"),
        format!("resources/recognition/{stem}.pages.json"),
        format!("resources/navigation/{stem}.navigation.json"),
        "resources/operations/operations.index.json".to_owned(),
        "resources/operations/operations.primitives.json".to_owned(),
    ] {
        assert!(files.remove(&path).is_some(), "sealed {path}");
    }
    (default, files, zip)
}

#[test]
fn one_off_a2a_check_config_and_startup_admit_digest_named_directories() {
    let root = TempDir::new().expect("tempdir");
    exercise_fixture(
        "neutral",
        &root.path().join("neutral"),
        &neutral_source_fixture(),
        &neutral_contained_task_package(),
    );
    let (default, files, zip) = released_default_package();
    exercise_fixture(
        &format!("released {default}"),
        &root.path().join("released"),
        &files,
        &zip,
    );
}
