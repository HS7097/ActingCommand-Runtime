// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-3a follow-up (wave 14 ruling NAV-3a item 5; model M-373 v1.1 §4.3; exec key
// 71): `record stop`'s `prerequisite_entry_example` no longer prints an entry of the retired
// actingd key `prerequisite_packages` (`{package_id, package_path}`, which a configuration now
// refuses with `config_decode_failed`). It prints where the package goes in its game's
// standard-pack registry, the one place another package finds it as a prerequisite package
// (contracts/lab-recording.md, "Output"): the game root's `bundle.json`, the `packs[]` entry that
// lists it, and its directory, the game root plus its `package_id`. The generating stop and the
// stop of the stopped recording (`already_generated`) print the same entry. The recording is test
// plan E-5's synthetic EXEC-I5 recording (`gate_e5_record_stop`): game `neutral`, server `test`,
// task `neutral_task`.

use crate::RecordStopOptions;
use crate::gate_e5_record_stop::Recorded;
use serde_json::json;

#[test]
fn nav3afu_record_stop_prints_the_registry_entry_of_its_package() {
    let recorded = Recorded::new();
    let expected = json!({
        "bundle": "<standard_packs.root>/neutral/bundle.json",
        "pack": {
            "package_id": "neutral.test.neutral_task",
            "server": "test",
            "entry_task_id": "neutral_task",
            "path": "packs/neutral.test.neutral_task"
        },
        "directory": "<standard_packs.root>/neutral/neutral.test.neutral_task"
    });

    let generated = recorded
        .stop(&RecordStopOptions::default())
        .unwrap_or_else(|(code, details)| panic!("record stop: {code}: {details}"));
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(generated["prerequisite_entry_example"], expected);

    let again = recorded
        .stop(&RecordStopOptions::default())
        .unwrap_or_else(|(code, details)| panic!("record stop again: {code}: {details}"));
    assert_eq!(again["lab_status"], "already_generated", "{again}");
    assert_eq!(again["prerequisite_entry_example"], expected);
}
