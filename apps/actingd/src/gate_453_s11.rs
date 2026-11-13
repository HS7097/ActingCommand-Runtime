// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S11 (contracts/state-root-installation.md; contracts/actingd-check-config.md, the
// report member `state_root_installation`; the coordinator's rulings 2 and 7 of the work order
// rd-1114/brief-453-s11-state-root-record.md): check-config judges the state root's installation
// record as the start would and never mints or writes. Its installation root is the root
// `--install-root` names, else the directory of `--config`, each only when it holds the
// installer's own `install\state.json`:
// - no installation root: `not_installed`, and a record in the state root is not read;
// - an unrecorded state root: `unclaimed`;
// - the record of this installation (named by `--install-root`, the configuration elsewhere):
//   `claimed_by_this`;
// - another installation's record: refused `state_root_installation_mismatch`, stage
//   `config_state_root`;
// - a record that is no record: refused `state_root_installation_record_invalid`, stage
//   `config_state_root`, the file as `path`;
// and after all five no identity file was minted and no state root changed.

use super::*;
use serde_json::{Value, json};
use std::fs;

const THIS_ID: &str = "ffeeddccbbaa99887766554433221100";
const OTHER_ID: &str = "00112233445566778899aabbccddeeff";

/// A configuration at `path` naming `state` as its state root, without instances.
fn write_config(path: &Path, state: &Path) {
    fs::create_dir_all(path.parent().expect("a folder")).expect("the configuration folder");
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": state,
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "instances": []
        }))
        .expect("the configuration JSON"),
    )
    .expect("write the configuration");
}

/// An installation root: an install root (`runtime\BUILD-MANIFEST.json`) whose `install` folder
/// holds the installer's own `state.json`, and, when given, the identity `installation_id`.
fn installation_root(root: &Path, installation_id: Option<&str>) {
    fs::create_dir_all(root.join("runtime")).expect("the runtime folder");
    fs::write(root.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("the manifest");
    fs::create_dir_all(root.join("install")).expect("the install folder");
    fs::write(root.join("install").join("state.json"), b"{}").expect("the installer's state");
    if let Some(installation_id) = installation_id {
        fs::write(
            root.join("install").join("identity.json"),
            serde_json::to_vec(&json!({
                "schema_version": "actingcommand.installation-identity.v1",
                "installation_id": installation_id,
                "minted_at_unix_ms": 1_800_000_000_000_u64,
                "minted_by": {"pid": 4242, "build_commit": null}
            }))
            .expect("identity JSON"),
        )
        .expect("the identity file");
    }
}

/// A state root holding a record of the installation `installation_id`.
fn recorded_state_root(state: &Path, installation_id: &str) {
    fs::create_dir_all(state).expect("the state root");
    fs::write(
        state.join("installation.json"),
        serde_json::to_vec(&json!({
            "schema_version": "actingcommand.state-root-installation.v1",
            "installation_id": installation_id,
            "claimed_at_unix_ms": 1_800_000_000_000_u64,
            "claimed_by": {
                "pid": 4242,
                "owner_epoch": "epoch_0123456789abcdef0123456789abcdef",
                "build_commit": null
            }
        }))
        .expect("record JSON"),
    )
    .expect("the record");
}

/// `actingd check-config --config <config> [--install-root <root>]`.
fn check(config: &Path, install_root: Option<&Path>) -> Result<Reported, ActingdError> {
    let mut words = vec![
        std::ffi::OsString::from("check-config"),
        std::ffi::OsString::from("--config"),
        config.as_os_str().to_owned(),
    ];
    if let Some(root) = install_root {
        words.push(std::ffi::OsString::from("--install-root"));
        words.push(root.as_os_str().to_owned());
    }
    check_config::run(words)
}

/// The report's `state_root_installation` of a check-config that passed.
fn reported(result: Result<Reported, ActingdError>) -> Value {
    let reported =
        result.unwrap_or_else(|error| panic!("check-config passes: {}", error.outcome()));
    reported
        .data
        .get("report")
        .expect("the report")
        .get("state_root_installation")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The names of every file below `folder`, each with its bytes.
fn files(folder: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(current) = folders.pop() {
        for entry in fs::read_dir(&current).expect("list a folder") {
            let path = entry.expect("a folder entry").path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let bytes = fs::read(&path).expect("read a file");
                files.insert(path, bytes);
            }
        }
    }
    files
}

#[test]
fn s11_check_config_reports_each_state_of_the_state_root_and_never_writes() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().join("root");
    installation_root(&root, Some(THIS_ID));
    let bare = temp.path().join("bare");
    installation_root(&bare, None);

    // No installation root: the configuration lies in a folder without the installer's state
    // file; a record in its state root is not read.
    let other_state = temp.path().join("other-state");
    recorded_state_root(&other_state, OTHER_ID);
    let plain_config = temp.path().join("plain").join("actingd.config.json");
    write_config(&plain_config, &other_state);
    let before = files(temp.path());
    assert_eq!(reported(check(&plain_config, None)), json!("not_installed"));

    // An unrecorded state root, the configuration in the installation root without an identity.
    let empty_state = temp.path().join("empty-state");
    let bare_config = bare.join("actingd.config.json");
    write_config(&bare_config, &empty_state);
    assert_eq!(reported(check(&bare_config, None)), json!("unclaimed"));

    // This installation's record, the root named by `--install-root`, the configuration elsewhere.
    let this_state = temp.path().join("this-state");
    recorded_state_root(&this_state, THIS_ID);
    let elsewhere_config = temp.path().join("elsewhere").join("actingd.config.json");
    write_config(&elsewhere_config, &this_state);
    assert_eq!(
        reported(check(&elsewhere_config, Some(&root))),
        json!("claimed_by_this")
    );

    // Another installation's record: the start's refusal.
    let root_config = root.join("actingd.config.json");
    write_config(&root_config, &other_state);
    let refused = check(&root_config, None).expect_err("another installation's state root");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "state_root_installation_mismatch",
        "{outcome}"
    );
    assert_eq!(outcome.value("stage"), Some(&json!("config_state_root")));
    assert_eq!(
        outcome.value("path"),
        Some(&json!(other_state.to_string_lossy()))
    );

    // A record that is no record.
    let broken_state = temp.path().join("broken-state");
    fs::create_dir_all(&broken_state).expect("the state root");
    fs::write(broken_state.join("installation.json"), b"[]").expect("a broken record");
    write_config(&root_config, &broken_state);
    let refused = check(&root_config, None).expect_err("a record that is no record");
    let outcome = refused.outcome();
    assert_eq!(
        outcome.code().as_str(),
        "state_root_installation_record_invalid",
        "{outcome}"
    );
    assert_eq!(outcome.value("stage"), Some(&json!("config_state_root")));
    assert_eq!(
        outcome.value("path"),
        Some(&json!(
            broken_state.join("installation.json").to_string_lossy()
        ))
    );

    // Nothing minted, nothing claimed: every file the checks found is as it was, and the only
    // files added are the configurations and the broken record this test wrote.
    let after = files(temp.path());
    for (path, bytes) in &before {
        assert_eq!(after.get(path), Some(bytes), "{}", path.display());
    }
    let added = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        added,
        BTreeSet::from([
            bare_config,
            elsewhere_config,
            root_config,
            this_state.join("installation.json"),
            broken_state.join("installation.json"),
        ]),
        "no identity minted in {} and no record written",
        bare.display()
    );
}
