// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S4 (register, "Slice #453 S4"; contracts/install-cli.md, "Fresh root"; coordinator
// work order rd-1114/brief-453-s4-install-new.md, "Tests"): the rescue row `new` builds a fresh
// root from a release of the neutral release fixture (`fixture.rs`), its Runtime member carrying
// the first configuration's template the repository ships, on the fake host, into temporary
// roots:
// - `s4_new_builds_exactly_the_build_the_configuration_the_state_folder_and_the_state_file`: the
//   tree is the build (live and kept), the configuration, the empty state folder `<new_root>\state`,
//   `state.json` and the installer's own lock, journal and end marker, nothing else; the
//   configuration is the template with the four members filled, and passed the build's
//   check-config before it was renamed into place;
// - `s4_new_puts_a_given_state_root_where_it_is_named_and_none_under_the_root`;
// - `s4_new_salt_is_32_bytes_as_text_new_each_run_and_nowhere_else`: the salt is 64 lowercase hex
//   digits, two runs give two salts, and its text is in no other file of the root, not in the
//   answer's line and not in any program line the fake host logged;
// - `s4_new_refuses_a_root_or_a_state_root_that_is_not_empty_and_changes_nothing`;
// - `s4_new_refuses_a_build_that_does_not_verify_and_writes_nothing`;
// - `s4_new_refuses_an_empty_actingd_config_edge_and_writes_nothing`;
// - `s4_new_a_first_configuration_check_config_refuses_ends_refused_with_its_part_kept`;
// - `s4_new_a_crash_after_each_step_resumes_to_the_same_tree`;
// - `s4_new_undo_after_each_step_sets_the_written_items_aside_and_deletes_nothing`;
// - `s4_new_mints_no_identity_and_writes_no_state_root_record`;
// - `s4_new_with_watchdog_registers_it_last_and_starts_nothing`.
// No sleep and no clock read: the fake host's clock moves one millisecond per read; its programs
// are fakes; no Runtime is started.

use super::fixture::{self, BuildSpec, C1, FakeHost, tree, word};
use super::new;
use crate::agent_channel::Answer as Answered;
use crate::rescue;
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The first configuration's template the Runtime member ships.
const TEMPLATE: &str = include_str!("../../../../distribution/windows/actingd.config.first.json");

/// A neutral build whose Runtime member carries the template.
fn build_spec() -> BuildSpec {
    let mut spec = BuildSpec::new(C1);
    spec.runtime
        .files
        .push((new::TEMPLATE_FILE.to_owned(), TEMPLATE.as_bytes().to_vec()));
    spec
}

/// A release folder of `spec` and a place for the new root, below `base`; the host's work folder
/// outside it.
struct Setup {
    _temp: TempDir,
    host: FakeHost,
    base: PathBuf,
    release: PathBuf,
    root: PathBuf,
}

fn setup(spec: &BuildSpec) -> Setup {
    let temp = TempDir::new().expect("tempdir");
    let base = temp.path().join("base");
    let release = base.join("release");
    spec.write_release(&release);
    let host = FakeHost::new(&temp.path().join("work"));
    let root = base.join("fresh");
    Setup {
        _temp: temp,
        host,
        base,
        release,
        root,
    }
}

/// The row `new` with `params` as the Lab's rescue word: its line and exit code.
fn new_line(host: &FakeHost, params: Value) -> (Value, u8) {
    let Value::Object(params) = params else {
        panic!("the parameters are an object");
    };
    match rescue::answer_new(host, Initiator::LabRescue, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("new: a line, not {other:?}"),
    }
}

/// The row with the new root and the release only.
fn plain_params(setup: &Setup) -> Value {
    json!({"new_root": word(&setup.root), "build": word(&setup.release)})
}

/// Every entry below `dir`: a file with its bytes, a folder with none.
fn whole_tree(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        for entry in fs::read_dir(&folder).expect("list a folder") {
            let entry = entry.expect("a folder entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().expect("an entry type").is_dir() {
                entries.insert(relative.clone(), None);
                pending.push((entry.path(), relative));
            } else {
                entries.insert(relative, Some(fs::read(entry.path()).expect("read a file")));
            }
        }
    }
    entries
}

/// The files of the build as a root holds them below `prefix` (`runtime\`, `tools\`).
fn build_files(spec: &BuildSpec, prefix: &str) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for (half, spec_half) in [("runtime", &spec.runtime), ("tools", &spec.tools)] {
        for (path, bytes) in spec_half.entries() {
            files.insert(format!("{prefix}{half}/{path}"), bytes);
        }
    }
    files
}

fn read_value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read a JSON file")).expect("JSON")
}

/// The one key of `files` that starts with `prefix`.
fn one_key<'a>(files: &'a BTreeMap<String, Vec<u8>>, prefix: &str) -> &'a str {
    let keys = files
        .keys()
        .filter(|key| key.starts_with(prefix))
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 1, "one {prefix}: {keys:?}");
    keys[0].as_str()
}

fn same_folder(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).expect("a folder") == fs::canonicalize(right).expect("a folder")
}

fn is_empty_folder(folder: &Path) -> bool {
    folder.is_dir()
        && fs::read_dir(folder)
            .expect("list the folder")
            .next()
            .is_none()
}

#[test]
fn s4_new_builds_exactly_the_build_the_configuration_the_state_folder_and_the_state_file() {
    let spec = build_spec();
    let setup = setup(&spec);
    let (line, exit) = new_line(&setup.host, plain_params(&setup));
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    let values = &line["outcome"]["values"];
    assert_eq!(values["operation"], "install_new", "{line}");
    assert_eq!(values["commit_sha"], C1, "{line}");
    assert_eq!(values["installation_changed"], true, "{line}");
    let operation_id = line["operation_id"].as_str().expect("the operation id");
    assert_eq!(values["install_operation_id"], operation_id, "{line}");

    let root = &setup.root;
    let files = tree(root);
    // The build, live and kept, byte for byte.
    let mut expected = build_files(&spec, "");
    expected.extend(build_files(&spec, &format!("install/builds/{C1}/")));
    for (path, bytes) in &expected {
        assert_eq!(files.get(path), Some(bytes), "{path}");
    }
    // And nothing else but the configuration, the state file, the kept build's release record
    // and the installer's own lock, journal and end marker.
    let journal = format!("install/operations/{operation_id}.json");
    let marker = one_key(&files, "install/current.json.finished-").to_owned();
    let mut names = expected.keys().cloned().collect::<Vec<_>>();
    names.extend([
        "actingd.config.json".to_owned(),
        "install/state.json".to_owned(),
        format!("install/builds/{C1}/release.json"),
        "install/writer.lock".to_owned(),
        journal.clone(),
        marker,
    ]);
    names.sort();
    assert_eq!(files.keys().cloned().collect::<Vec<_>>(), names);
    // The empty state folder, the default `<new_root>\state`, and no other folder of the root
    // but the build's and the installer's.
    assert!(is_empty_folder(&root.join("state")));

    // The configuration: the template with the state root, the salt, the authoring switch.
    let config = read_value(&root.join("actingd.config.json"));
    let state_root = config["state_root"].as_str().expect("the state root");
    assert!(Path::new(state_root).is_absolute(), "{state_root}");
    assert!(same_folder(Path::new(state_root), &root.join("state")));
    let salt = config["secret_fingerprint_salt"]
        .as_str()
        .expect("the salt");
    let mut template: Value = serde_json::from_str(TEMPLATE).expect("the template");
    template["state_root"] = json!(state_root);
    template["secret_fingerprint_salt"] = json!(salt);
    template["lab_module_authoring"] = json!(true);
    assert_eq!(config, template);
    assert_eq!(line["state_root"], state_root, "{line}");
    assert_eq!(line["lab_options_enabled"], json!(["lab_module_authoring"]));

    // Checked by the new live build's check-config, with the root, before it was renamed in.
    let checks = setup
        .host
        .log
        .borrow()
        .iter()
        .filter(|line| line.contains("check-config"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(checks.len(), 1, "{checks:?}");
    assert!(
        checks[0].starts_with("live actingcommand-actingd.exe check-config --config ")
            && checks[0].contains("actingd.config.json.new-")
            && checks[0].contains("--install-root"),
        "{checks:?}"
    );

    // `state.json`: the live build this operation installed, the kept build, no pack, the
    // configuration's SHA-256 (a release without facts is a local build).
    let state = read_value(&root.join("install").join("state.json"));
    let config_sha256 = format!(
        "sha256:{}",
        fixture::sha256_hex(&fs::read(root.join("actingd.config.json")).expect("config"))
    );
    assert_eq!(
        state,
        json!({
            "schema_version": "actingcommand.install-state.v1",
            "live": {"commit": C1, "operation_id": operation_id},
            "builds": [{"commit": C1, "tag": null, "origin": "local_build", "operation_id": operation_id}],
            "packs": [],
            "config_sha256": config_sha256,
        })
    );
    assert_eq!(line["config_sha256"], config_sha256, "{line}");
    // The rows that follow, from the new root's own program.
    for row in ["place", "instance_add", "up"] {
        assert!(line["next"][row].is_array(), "{row}: {line}");
    }
    // The journal: kind `new`, its initiator, the steps in the work order's order, every one done.
    let recorded = read_value(&root.join(&journal));
    assert_eq!(recorded["kind"], "new");
    assert_eq!(recorded["initiator"], Initiator::LabRescue.as_str());
    let kinds = recorded["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .map(|step| {
            assert_eq!(step["state"], "done", "{step}");
            step["kind"].as_str().expect("a kind").to_owned()
        })
        .collect::<Vec<_>>();
    let files_count = expected.len() / 2;
    let mut order = vec!["copy_build", "verify_tree", "place_build"];
    order.extend(std::iter::repeat_n("stage_file", files_count));
    order.extend(std::iter::repeat_n("swap_file", files_count));
    order.extend([
        "verify_tree",
        "write_config",
        "create_state_root",
        "record_state",
    ]);
    assert_eq!(kinds, order);
}

#[test]
fn s4_new_puts_a_given_state_root_where_it_is_named_and_none_under_the_root() {
    let setup = setup(&build_spec());
    let given = setup.base.join("elsewhere-state");
    let (line, exit) = new_line(
        &setup.host,
        json!({
            "new_root": word(&setup.root),
            "build": word(&setup.release),
            "new_state_root": word(&given),
        }),
    );
    assert_eq!(exit, 0, "{line}");
    assert!(is_empty_folder(&given));
    assert!(!setup.root.join("state").exists());
    let config = read_value(&setup.root.join("actingd.config.json"));
    let state_root = config["state_root"].as_str().expect("the state root");
    assert!(Path::new(state_root).is_absolute(), "{state_root}");
    assert!(same_folder(Path::new(state_root), &given));
}

#[test]
fn s4_new_salt_is_32_bytes_as_text_new_each_run_and_nowhere_else() {
    let mut salts = Vec::new();
    for _ in 0..2 {
        let setup = setup(&build_spec());
        let (line, exit) = new_line(&setup.host, plain_params(&setup));
        assert_eq!(exit, 0, "{line}");
        let config = read_value(&setup.root.join("actingd.config.json"));
        let salt = config["secret_fingerprint_salt"]
            .as_str()
            .expect("the salt")
            .to_owned();
        assert_eq!(salt.len(), 64, "32 bytes as hexadecimal text");
        assert!(
            salt.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "lowercase hexadecimal"
        );
        // Nowhere but in the configuration: the journal, the end marker, the state file, the
        // kept build, the answer's line, every program line the host logged.
        for (path, bytes) in tree(&setup.root) {
            if path == "actingd.config.json" {
                continue;
            }
            assert!(
                !String::from_utf8_lossy(&bytes).contains(&salt),
                "the salt is in {path}"
            );
        }
        assert!(
            !line.to_string().contains(&salt),
            "the salt is in the answer"
        );
        for logged in setup.host.log.borrow().iter() {
            assert!(!logged.contains(&salt), "the salt is in a logged line");
        }
        salts.push(salt);
    }
    assert_ne!(salts[0], salts[1], "two runs, two salts");
}

#[test]
fn s4_new_refuses_a_root_or_a_state_root_that_is_not_empty_and_changes_nothing() {
    // A new root that holds a file.
    let filled = setup(&build_spec());
    fs::create_dir_all(&filled.root).expect("the root");
    fs::write(filled.root.join("neutral.txt"), b"neutral").expect("a file");
    let before = whole_tree(&filled.base);
    let (line, exit) = new_line(&filled.host, plain_params(&filled));
    assert_eq!(exit, 1, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_refused", "{line}");
    assert_eq!(line["outcome"]["values"]["operation"], "install_new");
    assert_eq!(
        line["outcome"]["values"]["install_refusal"],
        "root_not_empty"
    );
    assert!(line["outcome"]["values"]["path"].is_string(), "{line}");
    assert_eq!(whole_tree(&filled.base), before);

    // A state root that holds a file; the new root absent.
    let used_state = setup(&build_spec());
    let used = used_state.base.join("used-state");
    fs::create_dir_all(&used).expect("the state root");
    fs::write(used.join("neutral.txt"), b"neutral").expect("a file");
    let before = whole_tree(&used_state.base);
    let (line, exit) = new_line(
        &used_state.host,
        json!({
            "new_root": word(&used_state.root),
            "build": word(&used_state.release),
            "new_state_root": word(&used),
        }),
    );
    assert_eq!(exit, 1, "{line}");
    assert_eq!(
        line["outcome"]["values"]["install_refusal"],
        "state_root_not_empty"
    );
    assert!(line["outcome"]["values"]["path"].is_string(), "{line}");
    assert_eq!(whole_tree(&used_state.base), before);
    assert!(!used_state.root.exists());
}

#[test]
fn s4_new_refuses_a_build_that_does_not_verify_and_writes_nothing() {
    let mut spec = build_spec();
    spec.runtime.altered.push("actingctl.exe".to_owned());
    let setup = setup(&spec);
    let before = whole_tree(&setup.base);
    let (line, exit) = new_line(&setup.host, plain_params(&setup));
    assert_eq!(exit, 1, "{line}");
    assert_eq!(
        line["outcome"]["values"]["install_refusal"],
        "release_check_failed"
    );
    let checks = line["outcome"]["values"]["release_checks"]
        .as_array()
        .expect("the failed checks");
    assert!(checks.contains(&json!("file_mismatch")), "{line}");
    assert!(
        line["failures"]
            .as_array()
            .is_some_and(|failures| !failures.is_empty()),
        "{line}"
    );
    assert_eq!(whole_tree(&setup.base), before);
    assert!(!setup.root.exists());
}

#[test]
fn s4_new_refuses_an_empty_actingd_config_edge_and_writes_nothing() {
    let mut spec = build_spec();
    spec.runtime.speaks["actingd-config"] = json!([90, 99]);
    spec.tools.speaks["actingd-config"] = json!([90, 99]);
    let setup = setup(&spec);
    let before = whole_tree(&setup.base);
    let (line, exit) = new_line(&setup.host, plain_params(&setup));
    assert_eq!(exit, 1, "{line}");
    assert_eq!(
        line["outcome"]["values"]["install_refusal"],
        "interface_edge"
    );
    assert_eq!(line["edges"][0]["interface"], "actingd-config", "{line}");
    assert_eq!(line["edges"][0]["result"], "fail", "{line}");
    assert_eq!(line["edges"][0]["parties"][1]["range"], json!([90, 99]));
    assert_eq!(whole_tree(&setup.base), before);
    assert!(!setup.root.exists());
}

#[test]
fn s4_new_a_first_configuration_check_config_refuses_ends_refused_with_its_part_kept() {
    let setup = setup(&build_spec());
    setup
        .host
        .programs
        .refuse_check_config
        .borrow_mut()
        .push("live".to_owned());
    let (line, exit) = new_line(&setup.host, plain_params(&setup));
    assert_eq!(exit, 1, "{line}");
    assert_eq!(
        line["outcome"]["values"]["install_refusal"],
        "config_refused_by_target"
    );
    assert!(line["next"]["actingctl_undo"].is_array(), "{line}");
    let root = &setup.root;
    let files = tree(root);
    let part = one_key(&files, "actingd.config.json.new-");
    assert!(part.ends_with(".part"), "{part}");
    assert!(!files.contains_key("actingd.config.json"));
    assert!(!files.contains_key("install/state.json"));
    assert!(!root.join("state").exists());
    // The operation ended (refused): no unfinished operation, its write_config step begun.
    assert!(!root.join("install").join("current.json").exists());
    let operation_id = line["operation_id"].as_str().expect("the operation id");
    let recorded = read_value(
        &root
            .join("install")
            .join("operations")
            .join(format!("{operation_id}.json")),
    );
    assert!(recorded["result"].is_object(), "{recorded}");
    let write_config = recorded["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .find(|step| step["kind"] == "write_config")
        .expect("write_config");
    assert_eq!(write_config["state"], "started", "{write_config}");
}

/// A root's files with the names an operation's milliseconds make (`op-<ms>`, `finished-<ms>`)
/// and the values a run makes itself (the salt, the state root's path, the operation's id) taken
/// out, and the journal's content left out.
fn normalized(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    for (path, bytes) in tree(root) {
        let name = if path.starts_with("install/operations/") {
            "install/operations/op-N.json".to_owned()
        } else if path.starts_with("install/current.json.finished-") {
            "install/current.json.finished-N".to_owned()
        } else {
            path.clone()
        };
        let bytes = match path.as_str() {
            "actingd.config.json" => {
                let mut config: Value = serde_json::from_slice(&bytes).expect("config JSON");
                let members = config.as_object_mut().expect("an object");
                members.remove("secret_fingerprint_salt");
                members.remove("state_root");
                serde_json::to_vec(&config).expect("JSON")
            }
            "install/state.json" => {
                let mut state: Value = serde_json::from_slice(&bytes).expect("state JSON");
                state["live"]["operation_id"] = Value::Null;
                state["builds"][0]["operation_id"] = Value::Null;
                state["config_sha256"] = Value::Null;
                serde_json::to_vec(&state).expect("JSON")
            }
            _ if name.starts_with("install/operations/") => Vec::new(),
            _ => bytes,
        };
        assert!(
            files.insert(name.clone(), bytes).is_none(),
            "one {name}: {path}"
        );
    }
    files
}

#[test]
fn s4_new_a_crash_after_each_step_resumes_to_the_same_tree() {
    let reference = setup(&build_spec());
    let (line, exit) = new_line(&reference.host, plain_params(&reference));
    assert_eq!(exit, 0, "{line}");
    let expected = normalized(&reference.root);
    let operation_id = line["operation_id"].as_str().expect("the operation id");
    let steps = read_value(
        &reference
            .root
            .join("install")
            .join("operations")
            .join(format!("{operation_id}.json")),
    )["steps"]
        .as_array()
        .expect("the steps")
        .len();
    for step in 1..=steps {
        let setup = setup(&build_spec());
        setup.host.crash_after.set(Some(step));
        let (line, exit) = new_line(&setup.host, plain_params(&setup));
        assert_eq!(exit, 2, "step {step}: {line}");
        assert_eq!(
            line["outcome"]["code"], "install_operation_interrupted",
            "step {step}: {line}"
        );
        assert!(line["next"]["actingctl_resume"].is_array(), "{line}");
        let operation_id = line["outcome"]["values"]["install_operation_id"]
            .as_str()
            .expect("the operation id")
            .to_owned();
        let resumed = fixture::run(
            &setup.host,
            &[
                "resume",
                "--root",
                word(&setup.root),
                "--operation",
                operation_id.as_str(),
            ],
        );
        assert_eq!(
            fixture::code(&resumed),
            "install_command_done",
            "step {step}: {:?}",
            resumed.outcome
        );
        assert_eq!(normalized(&setup.root), expected, "step {step}");
        assert!(is_empty_folder(&setup.root.join("state")), "step {step}");
    }
}

#[test]
fn s4_new_undo_after_each_step_sets_the_written_items_aside_and_deletes_nothing() {
    let reference = setup(&build_spec());
    let (line, _) = new_line(&reference.host, plain_params(&reference));
    let operation_id = line["operation_id"].as_str().expect("the operation id");
    let steps = read_value(
        &reference
            .root
            .join("install")
            .join("operations")
            .join(format!("{operation_id}.json")),
    )["steps"]
        .as_array()
        .expect("the steps")
        .len();
    // The contents of every file but the journal and the operation's marker, sorted.
    let contents = |root: &Path| {
        let mut contents = tree(root)
            .into_iter()
            .filter(|(path, _)| {
                !path.starts_with("install/operations/")
                    && !path.starts_with("install/current.json")
            })
            .map(|(_, bytes)| bytes)
            .collect::<Vec<_>>();
        contents.sort();
        contents
    };
    for step in 1..=steps {
        let setup = setup(&build_spec());
        setup.host.crash_after.set(Some(step));
        let (line, _) = new_line(&setup.host, plain_params(&setup));
        let operation_id = line["outcome"]["values"]["install_operation_id"]
            .as_str()
            .expect("the operation id")
            .to_owned();
        let before = contents(&setup.root);
        let undone = fixture::run(
            &setup.host,
            &[
                "undo",
                "--root",
                word(&setup.root),
                "--operation",
                operation_id.as_str(),
            ],
        );
        assert_eq!(
            fixture::code(&undone),
            "install_command_done",
            "step {step}: {:?}",
            undone.outcome
        );
        // Every byte the operation wrote is still there, under a name set aside.
        assert_eq!(contents(&setup.root), before, "step {step}");
        let files = tree(&setup.root);
        assert!(!files.contains_key("actingd.config.json"), "step {step}");
        assert!(!files.contains_key("install/state.json"), "step {step}");
        assert!(!setup.root.join("state").exists(), "step {step}");
        assert!(
            !setup.root.join("install").join("builds").join(C1).exists(),
            "step {step}"
        );
        for path in files.keys() {
            if path.starts_with("runtime/")
                || path.starts_with("tools/")
                || path.starts_with("install/builds/")
                || path.starts_with("actingd.config.json")
            {
                assert!(
                    path.contains(".rolledback-") || path.contains(".abandoned-"),
                    "step {step}: {path} is not set aside"
                );
            }
        }
        assert!(
            !setup.root.join("install").join("current.json").exists(),
            "step {step}"
        );
    }
}

#[test]
fn s4_new_mints_no_identity_and_writes_no_state_root_record() {
    let setup = setup(&build_spec());
    let (line, exit) = new_line(&setup.host, plain_params(&setup));
    assert_eq!(exit, 0, "{line}");
    assert!(!setup.root.join("install").join("identity.json").exists());
    assert!(!setup.root.join("state").join("installation.json").exists());
    assert!(is_empty_folder(&setup.root.join("state")));
}

#[test]
fn s4_new_with_watchdog_registers_it_last_and_starts_nothing() {
    let setup = setup(&build_spec());
    let (line, exit) = new_line(
        &setup.host,
        json!({
            "new_root": word(&setup.root),
            "build": word(&setup.release),
            "watchdog": true,
        }),
    );
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert!(line["watchdog"].is_object(), "{line}");
    let log = setup.host.log.borrow().clone();
    assert_eq!(
        log.last().map(String::as_str),
        Some("watchdog install"),
        "{log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|line| line.starts_with("watchdog"))
            .count(),
        1,
        "{log:?}"
    );
    // Nothing started: the fake Runtime was never spawned, paused or asked to close.
    assert!(setup.host.runtime.calls.borrow().is_empty());
    assert!(!setup.host.runtime.running.get());
    assert!(is_empty_folder(&setup.root.join("state")));
}
