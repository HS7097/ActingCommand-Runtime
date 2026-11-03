// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-3 (register §1.272; coordinator work order `rd-1031/brief-r3.md`): each
// instance's `start_emulator` is settable while the Runtime runs, `true` only on an instance the
// host names as bound by discovery (its check context). A start restores an instruction value as
// the other instance keys are and checks it against this start's instances: a `true` of an
// instance that is no longer bound by discovery no longer fits and is dropped alone
// (`config_override_dropped`). The host's own naming of its instances is tested in runtime-host
// (`gate_r3` there). The ids are made up.

use super::*;
use actingcommand_contract::{ConfigSettingChange, FactScalar, RuntimeConfigManifest};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// An instance bound by discovery at the earlier start and not at this one.
const MOVED: &str = "instance_0123456789abcdef0123456789abcdef";
/// An instance bound by discovery at both starts.
const STAYED: &str = "instance_fedcba9876543210fedcba9876543210";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

/// The context a start checks with, the host naming `discovery_bound` as bound by discovery.
fn context(discovery_bound: &[&str]) -> CheckContext {
    CheckContext::new(5_000, DEVICE).with_discovery_bound(
        discovery_bound
            .iter()
            .map(|instance| (*instance).to_owned())
            .collect(),
    )
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

/// The file layer a start renders from the two entries, which name no `start_emulator`.
fn file_layer() -> EffectiveConfig {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let parameters = render_parameters(
        &mechanism_rows(),
        &[MOVED.to_owned(), STAYED.to_owned()],
        &defaults,
        &json!({"instances": [
            {"instance_id": MOVED, "alias": "node.moved"},
            {"instance_id": STAYED, "alias": "node.stayed"},
        ]}),
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
}

#[test]
fn r3_restore_drops_a_value_that_no_longer_fits() {
    let moved_start = instance_key(MOVED, START_EMULATOR);
    let stayed_start = instance_key(STAYED, START_EMULATOR);
    let moved_gap = instance_key(MOVED, idle::MIN_GAP_MS);

    // The earlier start: both instances bound by discovery. A person turned both emulators'
    // start on while the Runtime ran, and set MOVED's idle gap.
    let earlier = context(&[MOVED, STAYED]);
    let applied = file_layer()
        .apply(
            &[
                set(&moved_start, json!(true)),
                set(&stayed_start, json!(true)),
                set(&moved_gap, json!(3_600_000)),
            ],
            &stamp(),
            &earlier,
        )
        .expect("both instances are bound by discovery");
    let rows = applied.config.override_rows();
    assert_eq!(rows.len(), 3, "{rows:?}");

    // A start whose instances are as they were restores every value as it was.
    let mut same = file_layer();
    let notes = same
        .restore(&rows, &earlier)
        .expect("the record has the row shape");
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(same.value(&moved_start), Some(&FactScalar::Boolean(true)));
    assert_eq!(same.value(&stayed_start), Some(&FactScalar::Boolean(true)));

    // This start: MOVED is no longer bound by discovery (its entry is a fixture's, a handset's or
    // bound by its port now), so its `start_emulator` no longer fits. That value alone goes,
    // `config_override_dropped`; MOVED's other value and STAYED's stay.
    let now = context(&[STAYED]);
    let mut config = file_layer();
    let notes = config
        .restore(&rows, &now)
        .expect("the record has the row shape");
    assert_eq!(
        notes
            .iter()
            .map(|note| (note.note, note.config_key.clone()))
            .collect::<Vec<_>>(),
        [(ConfigCode::ConfigOverrideDropped, moved_start.clone())]
    );
    assert_eq!(
        config.value(&moved_start),
        None,
        "MOVED's emulator is no instance's the Runtime starts"
    );
    assert_eq!(
        config.value(&stayed_start),
        Some(&FactScalar::Boolean(true))
    );
    assert_eq!(
        config.value(&moved_gap),
        Some(&FactScalar::DurationMs(3_600_000))
    );
    let kept = config
        .override_rows()
        .into_iter()
        .filter_map(|row| match row.get(OVERRIDE_CONFIG_KEY) {
            Some(FactScalar::String(key)) => Some(key.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(kept, BTreeSet::from([stayed_start, moved_gap]));
    config
        .check(&now)
        .expect("the configuration in effect passes");
}
