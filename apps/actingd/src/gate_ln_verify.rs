// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-VERIFY (brief rd-1031/brief-ln-verify.md, test 6; `contracts/ledger-maintenance.md`):
// the live v0.12.35 installer and acsetup pass exactly `ledger-maintenance verify --config
// <config>`, so those arguments alone give the bounded check, on a real closed root: its status word
// is `verified-sqlite-bounded`, and `--full` gives `verified-sqlite`. How many records it
// authenticates comes from the file's `ledger_observer.open_verify_tail_records`, as at the start,
// and a value the mechanism does not take refuses it.

use super::{ActingdError, config, ledger_maintenance};
use actingcommand_contract::InstallConfigPurpose;
use actingcommand_contract::outcome::Outcome;
use actingcommand_runtime_config::EffectiveConfig;
use actingcommand_runtime_host::{
    ExecutionBackendRegistry, ProviderAssembly, RuntimeHost, RuntimeHostConfig,
};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

const SALT: &str = "0123456789abcdef";

/// A closed formal root: a Runtime without instances started on it and closed.
fn closed_root(root: &Path) {
    let provider = ExecutionBackendRegistry::from_assembly(ProviderAssembly {
        instances: Vec::new(),
        vision: None,
        discovery: None,
    })
    .expect("a registry without instances");
    RuntimeHost::start(
        RuntimeHostConfig::new(root, SALT.as_bytes()).with_frame_retention_enabled(false),
        Arc::new(provider),
    )
    .expect("runtime host")
    .close()
    .expect("close the runtime host");
}

/// The configuration file of `state_root`, with `extra`'s members, in `directory`.
fn config_file(directory: &Path, state_root: &Path, extra: Value) -> std::path::PathBuf {
    let mut file = json!({
        "schema_version": "actingcommand.actingd.config.v1",
        "state_root": state_root,
        "bind_host": "127.0.0.1",
        "secret_fingerprint_salt": SALT,
        "instances": [],
    });
    if let (Some(members), Value::Object(extra)) = (file.as_object_mut(), extra) {
        members.extend(extra);
    }
    let path = directory.join("actingd.config.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&file).expect("the configuration file's JSON"),
    )
    .expect("the configuration file");
    path
}

/// `actingd ledger-maintenance verify --config <path>` and `flags`: the report's status word, or
/// the refusal.
fn verify(path: &Path, flags: &[&str]) -> Result<(Outcome, Value), ActingdError> {
    let mut arguments = vec![
        OsString::from("ledger-maintenance"),
        OsString::from("verify"),
        OsString::from("--config"),
        path.as_os_str().to_owned(),
    ];
    arguments.extend(flags.iter().map(OsString::from));
    let reported = ledger_maintenance::run(arguments)?;
    Ok((reported.outcome, Value::Object(reported.data)))
}

#[test]
fn ln_verify_t6_the_arguments_of_the_installer_give_the_bounded_check() {
    let temp = TempDir::new().expect("tempdir");
    let state_root = temp.path().join("state");
    std::fs::create_dir(&state_root).expect("the state root");
    closed_root(&state_root);
    let path = config_file(temp.path(), &state_root, json!({}));

    let (outcome, report) = match verify(&path, &[]) {
        Ok(reported) => reported,
        Err(error) => panic!("the installer's arguments pass a sound root: {error}"),
    };
    assert_eq!(outcome.code().as_str(), "ledger_maintenance_completed");
    assert_eq!(outcome.category().exit_code(), 0);
    assert_eq!(report["status"], json!("verified-sqlite-bounded"));
    assert_eq!(report["activated"], json!(false));
    assert_eq!(report["ledger"]["state"], json!("ready"));

    let (_, report) = match verify(&path, &["--full"]) {
        Ok(reported) => reported,
        Err(error) => panic!("--full passes a sound root: {error}"),
    };
    assert_eq!(report["status"], json!("verified-sqlite"));
}

/// The tail in effect for the file at `path`.
fn tail_of(path: &Path) -> u64 {
    let file = match config::load_for(path, InstallConfigPurpose::CandidateCheck) {
        Ok(file) => file,
        Err(code) => panic!("the configuration file loads: {code}"),
    };
    let manifest = match file.maintenance_manifest() {
        Ok(manifest) => manifest,
        Err(code) => panic!("the manifest: {code}"),
    };
    EffectiveConfig::from_manifest(&manifest)
        .expect("the configuration in effect")
        .ledger_observer()
        .expect("the ledger_observer section")
        .open_verify_tail_records
}

#[test]
fn ln_verify_t6_the_tail_is_the_files_and_a_value_out_of_range_refuses() {
    let temp = TempDir::new().expect("tempdir");
    let state_root = temp.path().join("state");
    std::fs::create_dir(&state_root).expect("the state root");

    // The file names none: the default. It names one: that one.
    let path = config_file(temp.path(), &state_root, json!({}));
    assert_eq!(tail_of(&path), 4_096);
    let path = config_file(
        temp.path(),
        &state_root,
        json!({"ledger_observer": {"open_verify_tail_records": 7}}),
    );
    assert_eq!(tail_of(&path), 7);

    // A value the key does not take refuses the bounded check before the root is touched.
    let path = config_file(
        temp.path(),
        &state_root,
        json!({"ledger_observer": {"open_verify_tail_records": 0}}),
    );
    let refused = verify(&path, &[]).err().expect("the value is refused");
    assert_eq!(refused.to_string(), "maintenance_config_invalid");
    assert!(
        !state_root.join("owner.lock").exists(),
        "the root is untouched"
    );
}
