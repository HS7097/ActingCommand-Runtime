// SPDX-License-Identifier: AGPL-3.0-only

//! G2, inventory key K8: the install-control inputs (Workflow #352 §4 and its I3 interface
//! comment; `distribution/windows/INSTALL.md`, "A/B installation inputs and Host control").
//! The requirement names no refusal code, so the rows assert only acceptance or refusal.

use actingcommand_contract::{
    InstallHeldStartup, InstallSelection, InstallTransitionAction, InstallTransitionPhase,
    InstallTransitionStatus,
};
use serde_json::{Value, json};

const TRANSITION_ID: &str = "transition-01";
const REQUEST_ID: &str = "request_00000000000000000000000000000001";

fn ticket() -> Value {
    json!({
        "target": {
            "owner_epoch": "epoch_00000000000000000000000000000001",
            "pid": 4242,
            "started_at_unix_ms": 1_760_000_000_000_u64,
        },
        "transition_id": TRANSITION_ID,
        "request_id": REQUEST_ID,
    })
}

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

/// Workflow #352 L76-84: root admission is closed while draining, drained, held, preparing or
/// failed, and open once released or aborted. No wildcard: a new phase must be classified here.
fn admission_closed(phase: InstallTransitionPhase) -> bool {
    match phase {
        InstallTransitionPhase::Draining
        | InstallTransitionPhase::Drained
        | InstallTransitionPhase::Held
        | InstallTransitionPhase::Preparing
        | InstallTransitionPhase::Failed => true,
        InstallTransitionPhase::Released | InstallTransitionPhase::Aborted => false,
    }
}

fn status(phase: &str, admission_closed: bool) -> InstallTransitionStatus {
    serde_json::from_value(json!({
        "ticket": ticket(),
        "phase": phase,
        "admission_closed": admission_closed,
        "timeout_ms": 60_000,
    }))
    .expect("decode the transition status")
}

#[test]
fn gate_install_status_admission_is_closed_except_when_released_or_aborted() {
    for (phase, wire) in [
        (InstallTransitionPhase::Draining, "draining"),
        (InstallTransitionPhase::Drained, "drained"),
        (InstallTransitionPhase::Held, "held"),
        (InstallTransitionPhase::Preparing, "preparing"),
        (InstallTransitionPhase::Released, "released"),
        (InstallTransitionPhase::Aborted, "aborted"),
        (InstallTransitionPhase::Failed, "failed"),
    ] {
        let closed = admission_closed(phase);
        let matching = status(wire, closed);
        assert_eq!(matching.phase, phase, "{wire}");
        assert!(
            matching.validate().is_ok(),
            "{wire} with admission_closed {closed} is accepted"
        );
        assert!(
            status(wire, !closed).validate().is_err(),
            "{wire} with admission_closed {} is refused",
            !closed
        );
    }
}

#[test]
fn gate_install_timeouts_accept_600000_ms_and_refuse_600001_ms() {
    // INSTALL.md: drain, held and release deadlines are bounded at 600 seconds.
    for (timeout_ms, accepted) in [(600_000_u64, true), (600_001, false)] {
        let begin_drain: InstallTransitionAction = serde_json::from_value(json!({
            "action": "begin_drain",
            "transition_id": TRANSITION_ID,
            "timeout_ms": timeout_ms,
        }))
        .expect("decode begin_drain");
        let release: InstallTransitionAction = serde_json::from_value(json!({
            "action": "release",
            "ticket": ticket(),
            "timeout_ms": timeout_ms,
        }))
        .expect("decode release");
        let held: InstallHeldStartup = serde_json::from_value(json!({
            "transition_id": TRANSITION_ID,
            "request_id": REQUEST_ID,
            "timeout_ms": timeout_ms,
        }))
        .expect("decode the held input");

        assert_eq!(
            begin_drain.validate().is_ok(),
            accepted,
            "begin_drain {timeout_ms}"
        );
        assert_eq!(release.validate().is_ok(), accepted, "release {timeout_ms}");
        assert_eq!(held.validate().is_ok(), accepted, "held {timeout_ms}");
    }
}
