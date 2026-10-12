// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A2a, part 2 (brief-a2a.md, tests T1 and T4): every refusal of actingd's
//! `ledger-maintenance` arguments and of its `--install-held` input carries an active registered
//! code of category `error` (test plan A-1, the registry half), and an ignored environment
//! override is warned about as the fixed code `env_override_ignored` with the variable as a value
//! (decision D6).

use super::{ActingdError, config, ledger_maintenance, parse_install_held_arguments};
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    IdentifierIssuer, InstallHeldStartup, InstallTransitionTicket, RuntimeShutdownTarget,
};
use actingcommand_device::EnvOverrides;
use std::ffi::{OsStr, OsString};

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// The code a refusal carries: with no stage and no detail its `Display` is the bare code.
fn refusal_code(error: &ActingdError) -> String {
    error.to_string()
}

#[track_caller]
fn assert_registered_error(input: &str, code: &str) {
    let catalog =
        actingcommand_contract::outcome::catalog_json().expect("the embedded catalog parses");
    let entry = &catalog["codes"][code];
    assert_eq!(
        entry["status"], "active",
        "{input}: the refusal code {code} is not an active registered code"
    );
    assert_eq!(
        entry["category"], "error",
        "{input}: the refusal code {code} is not of category error"
    );
}

/// `--install-held` input as the installer passes it: the held startup's JSON.
fn held(transition_id: &str, timeout_ms: u64, previous: Option<&str>) -> String {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let held = InstallHeldStartup {
        transition_id: transition_id.to_owned(),
        request_id: *issuer.mint_request_id().expect("request id").transport(),
        timeout_ms,
        previous: previous.map(|previous| InstallTransitionTicket {
            target: RuntimeShutdownTarget {
                owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
                pid: 4_242,
                started_at_unix_ms: 1_800_000_000_000,
            },
            transition_id: previous.to_owned(),
            request_id: *issuer.mint_request_id().expect("request id").transport(),
        }),
    };
    serde_json::to_string(&held).expect("held startup JSON")
}

/// #378 A2a T1 (brief-a2a.md; test plan A-1 `apps` 12, the registry half; ruling c-items:14).
#[test]
fn gate_ledger_maintenance_and_install_held_refusals_carry_registered_error_codes() {
    for (input, values) in [
        ("too few arguments", vec!["ledger-maintenance", "verify"]),
        (
            "an unknown operation",
            vec!["ledger-maintenance", "compact", "--config", "actingd.json"],
        ),
        (
            "an unknown option",
            vec!["ledger-maintenance", "verify", "--state-root", "state"],
        ),
        (
            "no --config",
            vec!["ledger-maintenance", "verify", "--backup", "backup.sqlite"],
        ),
    ] {
        let refused = ledger_maintenance::run(arguments(&values))
            .expect_err("ledger-maintenance refuses the arguments");
        assert_registered_error(
            &format!("ledger-maintenance with {input}"),
            &refusal_code(&refused),
        );
    }

    for (input, value) in [
        ("a value that is not JSON", "not json".to_owned()),
        ("an empty transition id", held("", 60_000, None)),
        ("timeout 0", held("transition-a2a", 0, None)),
        (
            "a previous ticket of another transition",
            held("transition-a2a", 60_000, Some("transition-other")),
        ),
    ] {
        let refused = parse_install_held_arguments(arguments(&[
            "--config",
            "actingd.json",
            "--install-held",
            value.as_str(),
        ]))
        .expect_err("--install-held refuses the input");
        assert_registered_error(
            &format!("--install-held with {input}"),
            &refusal_code(&refused),
        );
    }
}

/// #378 A2a T4 (brief-a2a.md; M378:81, SPEC:54, decision D6).
#[test]
fn gate_an_ignored_env_override_is_warned_as_the_fixed_code_with_the_variable() {
    let set = [EnvOverrides::VARIABLES[0], EnvOverrides::VARIABLES[3]];
    let (_, ignored) = config::env_overrides(false, |name| {
        set.contains(&name)
            .then(|| OsStr::new("configured").to_owned())
    });
    let warnings = config::env_override_warnings(&ignored);
    assert_eq!(
        warnings.len(),
        set.len(),
        "one warning per set and ignored variable"
    );
    let catalog =
        actingcommand_contract::outcome::catalog_json().expect("the embedded catalog parses");
    let entry = &catalog["codes"]["env_override_ignored"];
    assert_eq!(entry["category"], "warning", "{entry}");
    assert_eq!(entry["values"]["variable"], "required", "{entry}");
    for (warning, variable) in warnings.iter().zip(set) {
        assert_eq!(warning.code().as_str(), "env_override_ignored");
        assert_eq!(warning.category(), Category::Warning);
        assert_eq!(
            warning
                .value("variable")
                .and_then(serde_json::Value::as_str),
            Some(variable)
        );
        assert!(
            !warning.code().as_str().contains(variable),
            "the code spelling carries no variable"
        );
    }
}
