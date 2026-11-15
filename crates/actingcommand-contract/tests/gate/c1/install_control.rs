// SPDX-License-Identifier: AGPL-3.0-only

//! G2, inventory key K8: the install-control inputs (Workflow #352 §4 and its I3 interface
//! comment; `distribution/windows/INSTALL.md`, "A/B installation inputs"). The requirement names no
//! refusal code, so the rows assert only acceptance or refusal. Workflow #434 LAB-6S4a (0.12.63):
//! the installer's transition and the held start are gone (`install-control` [0, 0]), and with
//! them the rows of its phases and its 600 s bound; the selection's row stays.

use actingcommand_contract::InstallSelection;
use serde_json::{Value, json};

fn file(path: &str) -> Value {
    json!({ "path": path, "sha256": "0".repeat(64) })
}

/// A selection of slot `A`, generation 7: `members` is the slot's `MEMBERS.json`, `config` and
/// `provider` lie under `install/generations/7/` (I3: members fixed at `<slot>/MEMBERS.json`,
/// config and provider inside `install/generations/<generation>/`).
fn selection(members: &str, config: &str, provider: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.install-selection.v1",
        "slot": "A",
        "generation": 7,
        "members": file(members),
        "config": file(config),
        "provider": file(provider),
    }))
    .expect("encode the selection")
}

#[test]
fn gate_install_selection_refuses_a_slot_generation_or_provider_mismatch() {
    let members = "A/MEMBERS.json";
    let config = "install/generations/7/config.json";
    let provider = "install/generations/7/provider.json";
    InstallSelection::from_json(&selection(members, config, provider))
        .expect("the baseline selection is accepted");

    for (row, bytes) in [
        (
            "members names the other slot",
            selection("B/MEMBERS.json", config, provider),
        ),
        (
            "config lies outside the generation",
            selection(members, "install/generations/8/config.json", provider),
        ),
        (
            "provider lies outside the generation",
            selection(members, config, "install/generations/8/provider.json"),
        ),
    ] {
        assert!(InstallSelection::from_json(&bytes).is_err(), "{row}");
    }
}
