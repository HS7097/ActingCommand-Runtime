// SPDX-License-Identifier: AGPL-3.0-only

// STORE-FG-2 (0.12.29; design `rd-1029/design-store-fg.md` §2.2, §5 row T10; Alice's question 95,
// option A): `store_foreground.allowed_application_ids` parses as a list of Android package names,
// defaults to `[]` from the defaults file and allows nothing then; a value that is no list, an
// entry that is no string, a malformed entry, more than sixteen entries and an entry listed twice
// are refused (`config_value_invalid` naming the key); and no instruction may change it
// (`config_key_not_settable`), so the list in effect stays the file's. The ids are made up.

use super::*;
use crate::store_foreground::{ALLOWED_APPLICATION_IDS, MAX_ALLOWED_APPLICATION_IDS};
use actingcommand_contract::{ConfigSettingChange, RuntimeConfigManifest};
use serde_json::{Value, json};

const STORE: &str = "test.store.alpha";
const OTHER_STORE: &str = "test.store.beta";

const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

// The configuration a start renders from `file`, over the shipped defaults.
fn effective(file: Value) -> EffectiveConfig {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(&mechanism_rows(), &[], &defaults, &file, &|_| None)
        .expect("every key resolves");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("the configuration in effect")
}

fn section(ids: Value) -> Value {
    json!({"store_foreground": {"allowed_application_ids": ids}})
}

#[test]
fn store_fg2_t10_the_store_allowlist_parses_defaults_to_empty_and_is_not_settable() {
    // Default: no store, from the defaults file; the parameter row is not settable.
    let default = effective(json!({}));
    default.check(&context()).expect("the default passes");
    let allowlist = default.store_foreground().expect("the section in effect");
    assert!(allowlist.allowed_application_ids.is_empty());
    assert!(!allowlist.allows(STORE));
    let parameter = default
        .manifest()
        .parameters
        .into_iter()
        .find(|parameter| parameter.key == ALLOWED_APPLICATION_IDS)
        .expect("the parameter row");
    assert!(!parameter.settable);
    assert_eq!(
        parameter.source,
        actingcommand_contract::ConfigParameterSource::Default
    );

    // Listed stores, compared for equality only.
    let listed = effective(section(json!([STORE, OTHER_STORE])));
    listed.check(&context()).expect("two stores pass");
    let allowlist = listed.store_foreground().expect("the section in effect");
    assert_eq!(allowlist.allowed_application_ids, [STORE, OTHER_STORE]);
    assert!(allowlist.allows(STORE) && allowlist.allows(OTHER_STORE));
    assert!(!allowlist.allows("test.store"));
    assert!(!allowlist.allows("test.store.alpha.x"));

    // Refusals, each naming the key.
    let too_many = (0..=MAX_ALLOWED_APPLICATION_IDS)
        .map(|index| json!(format!("test.store.s{index}")))
        .collect::<Vec<_>>();
    for (ids, problem) in [
        (json!("test.store.alpha"), ConfigValueProblem::WrongType),
        (json!([7]), ConfigValueProblem::WrongType),
        (json!(["store"]), ConfigValueProblem::OutOfRange),
        (json!(["test..store"]), ConfigValueProblem::OutOfRange),
        (json!(["1test.store"]), ConfigValueProblem::OutOfRange),
        (json!(["test.store-alpha"]), ConfigValueProblem::OutOfRange),
        (json!([""]), ConfigValueProblem::OutOfRange),
        (Value::Array(too_many), ConfigValueProblem::OutOfRange),
        (json!([STORE, STORE]), ConfigValueProblem::SectionCheck),
    ] {
        let config = if ids.is_array() {
            effective(section(ids.clone()))
        } else {
            // A value that is no list is refused where the file layer reads it.
            let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
            let refused = render_parameters(
                &mechanism_rows(),
                &[],
                &defaults,
                &section(ids.clone()),
                &|_| None,
            )
            .expect_err("a value that is no list");
            assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
            assert_eq!(refused.config_key(), ALLOWED_APPLICATION_IDS);
            assert_eq!(refused.problem(), Some(problem), "{ids}");
            continue;
        };
        let refused = config
            .check(&context())
            .expect_err("a broken store allowlist is refused");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{ids}"
        );
        assert_eq!(refused.config_key(), ALLOWED_APPLICATION_IDS, "{ids}");
        assert_eq!(refused.problem(), Some(problem), "{ids}");
    }
    let sixteen = (0..MAX_ALLOWED_APPLICATION_IDS)
        .map(|index| json!(format!("test.store.s{index}")))
        .collect::<Vec<_>>();
    effective(section(Value::Array(sixteen)))
        .check(&context())
        .expect("sixteen stores pass");

    // A game is never its own store (actingd's assembly reads the instances' applications).
    let allowlist = listed.store_foreground().expect("the section in effect");
    allowlist
        .refuse_instance_applications(["neutral.application"])
        .expect("an instance whose application is no listed store");
    let refused = allowlist
        .refuse_instance_applications(["neutral.application", OTHER_STORE])
        .expect_err("an instance whose application is a listed store");
    assert_eq!(refused.config_key(), ALLOWED_APPLICATION_IDS);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));

    // No instruction changes it, not even to empty it; the list in effect stays the file's.
    for value in [json!([STORE, "test.store.gamma"]), json!([])] {
        let refused = listed
            .apply(
                &[ConfigSettingChange {
                    key: ALLOWED_APPLICATION_IDS.to_owned(),
                    value: Some(value.clone()),
                }],
                &InstructionStamp {
                    set_unix_ms: 1,
                    set_in_owner_epoch: "epoch".to_owned(),
                    request_id: "request".to_owned(),
                    channel: None,
                },
                &context(),
            )
            .expect_err("the store allowlist is not settable");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigKeyNotSettable,
            "{value}"
        );
        assert_eq!(refused.config_key(), ALLOWED_APPLICATION_IDS, "{value}");
    }
    assert_eq!(
        listed
            .store_foreground()
            .expect("the section in effect")
            .allowed_application_ids,
        [STORE, OTHER_STORE]
    );
}
