// SPDX-License-Identifier: AGPL-3.0-only

// #402 HS-1 (register §1.267; design `rd-1031/backend-switch/model-backend-switch.md` §2.1, §3.6,
// §3.8, §7 HS-1; coordinator work order `brief-hs-1.md`): each instance's `touch_backend` and
// `capture_backend` rows, read only until HS-2 brings the switch, and their whole-instance check:
// the Nemu pair, a fixture with no backend, one instance and only its backend keys per request, a
// start that drops the restored backends an instance's check refuses, and the console's advanced
// tier. The kind rules here are the design's table, as the host fills them in from the device
// layer (`runtime-host` `backend_kind_rules`, whose own tests are `gate_hs1` there). The ids are
// made up.

use super::*;
use crate::instance::check_backends;
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// An emulator instance.
const EMULATOR: &str = "instance_0123456789abcdef0123456789abcdef";
/// Another emulator instance.
const OTHER: &str = "instance_fedcba9876543210fedcba9876543210";
/// A fixture instance: its entry names no backend.
const FIXTURE: &str = "instance_00112233445566778899aabbccddeeff";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

/// The backends of each kind of device by the design's table (HS-M §2.1): an emulator has every
/// touch backend and every capture backend but `scrcpy`; a handset has no `nemu_ipc` and no
/// `droidcast_raw`.
fn rules() -> BackendKindRules {
    BackendKindRules {
        emulator: DeviceBackends::admitting(|_| true, |capture| capture != "scrcpy"),
        handset: DeviceBackends::admitting(
            |touch| touch != "nemu_ipc",
            |capture| matches!(capture, "adb" | "scrcpy"),
        ),
    }
}

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE).with_backends(rules())
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn touch(instance: &str) -> String {
    instance_key(instance, TOUCH_BACKEND)
}

fn capture(instance: &str) -> String {
    instance_key(instance, CAPTURE_BACKEND)
}

/// The file layer a start renders from these `instances[]` entries, as actingd hands it over.
fn from_entries(entries: &[Value]) -> Result<EffectiveConfig, ConfigFailure> {
    let ids = entries
        .iter()
        .filter_map(|entry| entry["instance_id"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let parameters = render_parameters(
        &mechanism_rows(),
        &ids,
        &defaults,
        &json!({ "instances": entries }),
        &|_| None,
    )?;
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
}

/// An emulator entry naming these backends.
fn emulator(id: &str, touch: &str, capture: &str) -> Value {
    json!({"instance_id": id, "alias": "node", "touch_backend": touch, "capture_backend": capture})
}

fn assert_refused(refused: &ConfigFailure, key: &str, problem: ConfigValueProblem, case: &str) {
    assert_eq!(
        refused.failure_code(),
        ConfigCode::ConfigValueInvalid,
        "{case}"
    );
    assert_eq!(refused.config_key(), key, "{case}");
    assert_eq!(refused.problem(), Some(problem), "{case}");
}

#[test]
fn hs1_backend_rows_read_only_until_hs2() {
    // The rows: a word of the device layer's explicit backends, no default (a fixture has
    // none), since 0.12.39. #402 HS-2 (register §1.270) makes both settable: the switch is HS-2's
    // (adapted from "not settable, read once at assembly", as the requirement changed).
    for (member, tokens) in [
        (TOUCH_BACKEND, TOUCH_BACKEND_TOKENS),
        (CAPTURE_BACKEND, CAPTURE_BACKEND_TOKENS),
    ] {
        let key = instance_key(EMULATOR, member);
        let (row, instance) = find_row(&key).expect("a row of the key table");
        assert_eq!(instance, Some(EMULATOR), "{key}");
        assert_eq!(row.file_path, FilePath::InstanceMember, "{key}");
        assert_eq!(row.value_type, ValueType::String, "{key}");
        assert_eq!(row.range, KeyRange::Tokens(tokens), "{key}");
        assert_eq!(row.default, DefaultSource::None, "{key}");
        assert_eq!(row.settable, Settable::Yes, "{key}");
        assert_eq!(row.since, "0.12.39", "{key}");
    }
    assert_eq!(
        TOUCH_BACKEND_TOKENS,
        ["adb_shell_input", "maatouch", "minitouch", "nemu_ipc"]
    );
    assert_eq!(
        CAPTURE_BACKEND_TOKENS,
        ["adb", "droidcast_raw", "nemu_ipc", "scrcpy"]
    );

    // The file layer reports both, explicit and (HS-2) settable.
    let config = from_entries(&[emulator(EMULATOR, "maatouch", "nemu_ipc")])
        .expect("the file layer renders");
    config.check(&context()).expect("the file layer passes");
    for (key, value) in [
        (touch(EMULATOR), "maatouch"),
        (capture(EMULATOR), "nemu_ipc"),
    ] {
        let row = config
            .manifest()
            .parameters
            .into_iter()
            .find(|parameter| parameter.key == key)
            .expect("the row is reported");
        assert_eq!(row.value, FactScalar::String(value.to_owned()), "{key}");
        assert_eq!(row.source, ConfigParameterSource::Explicit, "{key}");
        assert!(row.settable, "{key}");
    }

    // HS-2: an instruction on either key, or on both keys of the instance, applies; a clear
    // returns the key to its file value.
    for (changes, touch_now, capture_now) in [
        (
            vec![set(&touch(EMULATOR), json!("adb_shell_input"))],
            "adb_shell_input",
            "nemu_ipc",
        ),
        (
            vec![set(&capture(EMULATOR), json!("adb"))],
            "maatouch",
            "adb",
        ),
        (
            vec![
                set(&touch(EMULATOR), json!("nemu_ipc")),
                set(&capture(EMULATOR), json!("nemu_ipc")),
            ],
            "nemu_ipc",
            "nemu_ipc",
        ),
        (
            vec![ConfigSettingChange {
                key: touch(EMULATOR),
                value: None,
            }],
            "maatouch",
            "nemu_ipc",
        ),
    ] {
        let applied = config
            .apply(&changes, &stamp(), &context())
            .unwrap_or_else(|refused| panic!("{changes:?}: {refused:?}"));
        assert_eq!(
            applied.config.value(&touch(EMULATOR)),
            Some(&FactScalar::String(touch_now.to_owned())),
            "{changes:?}"
        );
        assert_eq!(
            applied.config.value(&capture(EMULATOR)),
            Some(&FactScalar::String(capture_now.to_owned())),
            "{changes:?}"
        );
    }
    assert!(config.override_rows().is_empty());
}

#[test]
fn hs1_nemu_touch_without_nemu_capture_refused_pair() {
    // Nemu touch beside another capture backend: refused for the touch key, at start as through
    // the whole-instance check of an instruction.
    for capture in ["adb", "droidcast_raw"] {
        let case = format!("nemu_ipc touch with {capture} capture");
        let config = from_entries(&[emulator(EMULATOR, "nemu_ipc", capture)]).expect(&case);
        let refused = config.check(&context()).expect_err(&case);
        assert_refused(
            &refused,
            &touch(EMULATOR),
            ConfigValueProblem::BackendPairRequired,
            &case,
        );
    }

    // The pair itself passes, and Nemu capture beside every other touch backend passes: the
    // pairing binds touch to capture, never capture to touch.
    for touch_backend in TOUCH_BACKEND_TOKENS {
        let config = from_entries(&[emulator(EMULATOR, touch_backend, "nemu_ipc")])
            .expect("the file layer renders");
        config
            .check(&context())
            .unwrap_or_else(|refused| panic!("{touch_backend} with nemu_ipc: {refused:?}"));
    }
}

#[test]
fn hs1_fixture_backend_instruction_refused() {
    // A fixture's entry names no backend, so its file layer has neither row.
    let config = from_entries(&[
        emulator(EMULATOR, "adb_shell_input", "nemu_ipc"),
        json!({"instance_id": FIXTURE, "alias": "node.fixture"}),
    ])
    .expect("the file layer renders");
    config.check(&context()).expect("the file layer passes");
    assert!(!config.file_names_backends(FIXTURE));
    assert!(config.file_names_backends(EMULATOR));
    assert_eq!(config.value(&touch(FIXTURE)), None);
    assert_eq!(config.value(&capture(FIXTURE)), None);

    // An instruction naming a fixture's backend is refused, and nothing changes.
    for changes in [
        vec![set(&touch(FIXTURE), json!("adb_shell_input"))],
        vec![set(&capture(FIXTURE), json!("adb"))],
    ] {
        let refused = config
            .apply(&changes, &stamp(), &context())
            .expect_err("a fixture has no backend");
        assert_eq!(refused.config_key(), changes[0].key, "{changes:?}");
    }
    assert!(config.override_rows().is_empty());

    // The whole-instance check refuses any backend of a fixture, whatever the kind rules admit:
    // every word of both lists, as an instruction would set it over the fixture's file layer.
    let everything = BackendKindRules {
        emulator: DeviceBackends::admitting(|_| true, |_| true),
        handset: DeviceBackends::admitting(|_| true, |_| true),
    };
    for (key, tokens) in [
        (touch(FIXTURE), TOUCH_BACKEND_TOKENS),
        (capture(FIXTURE), CAPTURE_BACKEND_TOKENS),
    ] {
        for word in tokens {
            let case = format!("{key} = {word}");
            let refused = check_backends(
                FIXTURE,
                |read: &str| {
                    if read == key {
                        Some(FactScalar::String((*word).to_owned()))
                    } else {
                        config.value(read).cloned()
                    }
                },
                config.file_names_backends(FIXTURE),
                &everything,
            )
            .expect_err(&case);
            assert_refused(
                &refused,
                &key,
                ConfigValueProblem::BackendKindUnsupported,
                &case,
            );
        }
    }
}

#[test]
fn hs1_backend_change_not_alone_refused() {
    let config = from_entries(&[
        emulator(EMULATOR, "adb_shell_input", "nemu_ipc"),
        emulator(OTHER, "maatouch", "adb"),
    ])
    .expect("the file layer renders");
    let cooldown = instance_key(EMULATOR, STUCK_RECOVERY_COOLDOWN_SECS);
    let parallel = crate::scheduling::PARALLEL_ENABLED;
    // A backend key beside another key of its instance, another section's key or the other
    // instance's backend key: refused whole, naming the first key that is not a backend key of
    // the instance the first backend key names.
    for (changes, named) in [
        (
            vec![
                set(&touch(EMULATOR), json!("nemu_ipc")),
                set(&cooldown, json!(120)),
            ],
            cooldown.clone(),
        ),
        (
            vec![
                set(&cooldown, json!(120)),
                set(&capture(EMULATOR), json!("nemu_ipc")),
            ],
            cooldown.clone(),
        ),
        (
            vec![
                set(&touch(EMULATOR), json!("nemu_ipc")),
                set(parallel, json!(false)),
            ],
            parallel.to_owned(),
        ),
        (
            vec![
                set(&touch(EMULATOR), json!("maatouch")),
                set(&capture(OTHER), json!("nemu_ipc")),
            ],
            capture(OTHER),
        ),
    ] {
        let refused = config
            .apply(&changes, &stamp(), &context())
            .expect_err("not alone");
        assert_refused(
            &refused,
            &named,
            ConfigValueProblem::BackendSwitchNotAlone,
            &format!("{changes:?}"),
        );
    }

    // One instance's backend keys alone, one or both, are never refused as not alone, and a
    // request without a backend key is as before.
    for changes in [
        vec![set(&touch(EMULATOR), json!("nemu_ipc"))],
        vec![
            set(&touch(EMULATOR), json!("nemu_ipc")),
            set(&capture(EMULATOR), json!("nemu_ipc")),
        ],
    ] {
        let problem = config
            .apply(&changes, &stamp(), &context())
            .err()
            .and_then(|refused| refused.problem());
        assert_ne!(
            problem,
            Some(ConfigValueProblem::BackendSwitchNotAlone),
            "{changes:?}"
        );
    }
    let applied = config
        .apply(&[set(&cooldown, json!(120))], &stamp(), &context())
        .expect("a per-instance key without a backend key applies");
    assert_eq!(
        applied.config.value(&cooldown),
        Some(&FactScalar::Integer(120))
    );
}

#[test]
fn hs1_ui_tier_backend_rows_advanced() {
    for instance in [EMULATOR, OTHER, FIXTURE] {
        assert_eq!(ui_tier(&touch(instance)), UiTier::Advanced, "{instance}");
        assert_eq!(ui_tier(&capture(instance)), UiTier::Advanced, "{instance}");
    }
    let advanced = UI_TIERS
        .iter()
        .filter(|(_, tier)| *tier == UiTier::Advanced)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    assert!(advanced.contains(&KeyName::PerInstance(TOUCH_BACKEND)));
    assert!(advanced.contains(&KeyName::PerInstance(CAPTURE_BACKEND)));
    assert_eq!(advanced.len(), MAX_ADVANCED_KEYS, "the two fill the eight");
}

/// One recorded `host.config_overrides` row of `key`, set to `value` over the file's explicit
/// `base` (both canonical JSON).
fn recorded(key: &str, value: &Value, base: &Value) -> BTreeMap<String, FactScalar> {
    let text = |value: &Value| FactScalar::String(canonical_json(value));
    BTreeMap::from([
        (
            OVERRIDE_CONFIG_KEY.to_owned(),
            FactScalar::String(key.to_owned()),
        ),
        (OVERRIDE_VALUE_JSON.to_owned(), text(value)),
        (OVERRIDE_BASE_JSON.to_owned(), text(base)),
        (
            OVERRIDE_BASE_SOURCE.to_owned(),
            FactScalar::String(ConfigParameterSource::Explicit.as_str().to_owned()),
        ),
        (OVERRIDE_SET_UNIX_MS.to_owned(), FactScalar::TimestampMs(1)),
        (
            OVERRIDE_SET_IN_OWNER_EPOCH.to_owned(),
            FactScalar::String("epoch".to_owned()),
        ),
        (
            OVERRIDE_REQUEST_ID.to_owned(),
            FactScalar::String("request".to_owned()),
        ),
    ])
}

#[test]
fn hs1_restore_drops_pair_breaking_rows() {
    // This start's file: EMULATOR adb touch beside adb capture, OTHER Nemu capture; each names a
    // per-instance key of its own besides.
    let mut entry = emulator(EMULATOR, "adb_shell_input", "adb");
    entry["stuck_recovery_cooldown_secs"] = json!(600);
    let mut other = emulator(OTHER, "maatouch", "nemu_ipc");
    other["stuck_recovery"] = json!(true);
    let mut config = from_entries(&[entry, other]).expect("the file layer renders");
    config.check(&context()).expect("the file layer passes");

    // Recorded by an earlier start: EMULATOR's Nemu touch, which breaks the pair over this
    // file's adb capture; OTHER's scrcpy capture, which an emulator does not have; and one
    // other per-instance value of each.
    let cooldown = instance_key(EMULATOR, STUCK_RECOVERY_COOLDOWN_SECS);
    let recovery = instance_key(OTHER, STUCK_RECOVERY);
    let rows = vec![
        recorded(
            &touch(EMULATOR),
            &json!("nemu_ipc"),
            &json!("adb_shell_input"),
        ),
        recorded(&cooldown, &json!(120), &json!(600)),
        recorded(&capture(OTHER), &json!("scrcpy"), &json!("nemu_ipc")),
        recorded(&recovery, &json!(false), &json!(true)),
    ];
    let notes = config
        .restore(&rows, &context())
        .expect("the record has the row shape");

    // The backend values go, each `config_override_dropped`; the instances' other values stay.
    assert_eq!(
        notes
            .iter()
            .map(|note| (note.note, note.config_key.clone()))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            (ConfigCode::ConfigOverrideDropped, touch(EMULATOR)),
            (ConfigCode::ConfigOverrideDropped, capture(OTHER)),
        ])
    );
    assert_eq!(
        config.value(&touch(EMULATOR)),
        Some(&FactScalar::String("adb_shell_input".to_owned()))
    );
    assert_eq!(
        config.value(&capture(OTHER)),
        Some(&FactScalar::String("nemu_ipc".to_owned()))
    );
    assert_eq!(config.value(&cooldown), Some(&FactScalar::Integer(120)));
    assert_eq!(config.value(&recovery), Some(&FactScalar::Boolean(false)));
    let kept = config
        .override_rows()
        .into_iter()
        .filter_map(|row| match row.get(OVERRIDE_CONFIG_KEY) {
            Some(FactScalar::String(key)) => Some(key.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(kept, BTreeSet::from([cooldown, recovery]));
    config
        .check(&context())
        .expect("the configuration in effect passes");
}
