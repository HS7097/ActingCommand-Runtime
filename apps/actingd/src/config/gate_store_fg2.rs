// SPDX-License-Identifier: AGPL-3.0-only

// STORE-FG-2 (0.12.29; design `rd-1029/design-store-fg.md` §2.2, §5 row T10; Alice's question 95,
// option A; `contracts/runtime-config.md`, "The `store_foreground` section"): the start-up
// assembly reports `store_foreground.allowed_application_ids` (default `[]` from the defaults
// file, not settable), takes a list of store applications, and refuses the start and check-config
// (which assembles the same way) when the list names a configured instance's own
// `application_id` (`config_value_invalid`, `section_check`, naming the key): a game is never its
// own store. The ids are made up.

use super::*;
use actingcommand_contract::{ConfigParameterSource, FactScalar, IdentifierIssuer};
use actingcommand_runtime_config::store_foreground::ALLOWED_APPLICATION_IDS;
use serde_json::{Value, json};
use tempfile::TempDir;

/// A made-up store application.
const STORE: &str = "test.store.alpha";
/// The configured instance's own application.
const GAME: &str = "neutral.application";

/// An actingd configuration of one device instance whose application is [`GAME`], with this
/// `store_foreground` section when one is given.
fn configuration(root: &TempDir, store_foreground: Option<Value>) -> ActingdConfigFile {
    let id = IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id");
    let mut config = json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.path(),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": [{
            "alias": "neutral.device", "instance_id": id.transport(),
            "application_id": GAME, "adb_path": root.path().join("missing-adb.exe"),
            "connect": false, "host": "127.0.0.1", "port": 16384,
            "touch_backend": "adb_shell_input", "capture_backend": "adb"
        }]
    });
    if let Some(section) = store_foreground {
        config["store_foreground"] = section;
    }
    serde_json::from_value::<ActingdConfigFile>(config).expect("typed config")
}

/// The manifest row of the key.
fn row(assembly: &RuntimeAssembly) -> &actingcommand_contract::ConfigParameter {
    assembly
        .manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == ALLOWED_APPLICATION_IDS)
        .expect("the manifest reports the store allowlist")
}

#[test]
fn store_fg2_t10_the_allowlist_defaults_to_empty_and_never_names_an_instances_own_application() {
    let root = TempDir::new().expect("tempdir");
    // A file that does not name the section: `[]`, from the defaults file, and no instruction may
    // change it.
    let assembly = configuration(&root, None)
        .assemble()
        .expect("runtime assembly");
    let reported = row(&assembly);
    assert_eq!(reported.value, FactScalar::String("[]".to_owned()));
    assert_eq!(reported.source, ConfigParameterSource::Default);
    assert!(!reported.settable, "only the configuration file widens it");

    // A store the file lists is taken as it is written.
    let assembly = configuration(&root, Some(json!({"allowed_application_ids": [STORE]})))
        .assemble()
        .expect("a listed store is taken");
    let reported = row(&assembly);
    assert_eq!(reported.value, FactScalar::String(format!("[\"{STORE}\"]")));
    assert_eq!(reported.source, ConfigParameterSource::Explicit);

    // The instance's own application, alone or beside a store, and a malformed entry: the start
    // and check-config refuse, naming the key.
    for (ids, problem) in [
        (json!([GAME]), "section_check"),
        (json!([STORE, GAME]), "section_check"),
        (json!(["not a package"]), "out_of_range"),
    ] {
        let section = json!({ "allowed_application_ids": ids });
        let Err((code, outcome)) =
            configuration(&root, Some(section.clone())).assemble_or_refusal()
        else {
            panic!("{section} is refused");
        };
        assert_eq!(
            code,
            actingcommand_runtime_config::ConfigCode::ConfigValueInvalid.code(),
            "{section}"
        );
        assert_eq!(
            outcome.value("config_key"),
            Some(&json!(ALLOWED_APPLICATION_IDS)),
            "{section}"
        );
        assert_eq!(
            outcome.value("config_value_problem"),
            Some(&json!(problem)),
            "{section}"
        );
    }
}
