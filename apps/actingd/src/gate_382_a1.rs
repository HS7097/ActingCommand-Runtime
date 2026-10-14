// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7, G9; inventory-apps key 12): actingd's subcommand
// arguments. `unlock-owner`, `suspended` and `ledger-maintenance` refuse a malformed argument
// list before they read any file, each with its own registered code (contracts/outcome-codes,
// the actingd fragment; Workflow #378 A2a), and the refusal is an error: one report line with
// its outcome and exit 1 (Workflow #378 model §7.4, §7.6). The registry half of the
// ledger-maintenance and `--install-held` rows is `gate_a2a`. `--install-held <json>` is taken
// only right after `--config <path>` (distribution/windows/INSTALL.md, "Host installation
// control"), and its deadline is bounded at 600 s (`MAX_INSTALL_TIMEOUT_MS`).
//
// G15 (inventory-apps key 39): `check-config` refuses each vision root of
// tests/support/vision_folders.rs at its vision stage with the code vision-provider-check's model
// folder report gives the same root (that program's `gate_382_a1`), and passes the valid one.

use super::*;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::{IdentifierIssuer, InstallHeldStartup, MAX_INSTALL_TIMEOUT_MS};
use actingcommand_vision_ffi::VisionListingCode;
use std::ffi::OsString;

#[path = "../../../tests/support/vision_folders.rs"]
mod vision_folders;

type Subcommand = fn(Vec<OsString>) -> Result<Reported, ActingdError>;

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// The refusal `run` gives for `values`; a run that goes on fails the test.
fn refusal(run: Subcommand, values: &[&str]) -> ActingdError {
    match run(arguments(values)) {
        Ok(_) => panic!("{values:?} is taken"),
        Err(error) => error,
    }
}

/// `--install-held` input as the installer passes it, with `timeout_ms` as its deadline.
fn held(timeout_ms: u64) -> String {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    serde_json::to_string(&InstallHeldStartup {
        transition_id: "transition-g9".to_owned(),
        request_id: *issuer.mint_request_id().expect("request id").transport(),
        timeout_ms,
        previous: None,
    })
    .expect("held startup JSON")
}

#[test]
fn gate_each_subcommand_refuses_its_malformed_arguments_with_its_code_and_exit_1() {
    let unlock: Subcommand = owner_unlock::run;
    let suspended: Subcommand = suspended::run;
    let maintenance: Subcommand = ledger_maintenance::run;
    let eleven = [
        "ledger-maintenance",
        "verify",
        "--config",
        "a.json",
        "--backup",
        "b.sqlite",
        "--target",
        "t",
        "--artifact-root",
        "r",
        "extra",
    ];
    let rows: &[(Subcommand, &[&str], ActingdCode)] = &[
        // unlock-owner: its five codes.
        (
            unlock,
            &[
                "unlock-owner",
                "--config",
                "a.json",
                "--actor",
                "operator",
                "--confirm-resources-released",
                "extra",
            ],
            ActingdCode::UnlockOwnerUsageInvalid,
        ),
        (
            unlock,
            &["unlock-owner", "--config", "a.json", "--force"],
            ActingdCode::UnlockOwnerOptionInvalid,
        ),
        (
            unlock,
            &["unlock-owner", "--config", "a.json", "--config", "b.json"],
            ActingdCode::UnlockOwnerOptionInvalid,
        ),
        (
            unlock,
            &[
                "unlock-owner",
                "--confirm-resources-released",
                "--confirm-resources-released",
            ],
            ActingdCode::UnlockOwnerOptionInvalid,
        ),
        (
            unlock,
            &["unlock-owner", "--config", ""],
            ActingdCode::UnlockOwnerOptionInvalid,
        ),
        (
            unlock,
            &["unlock-owner", "--actor", "operator"],
            ActingdCode::UnlockOwnerConfigMissing,
        ),
        (
            unlock,
            &["unlock-owner", "--config", "a.json"],
            ActingdCode::UnlockOwnerActorMissing,
        ),
        (
            unlock,
            &["unlock-owner", "--config", "a.json", "--actor", "ops/team"],
            ActingdCode::UnlockOwnerActorInvalid,
        ),
        // suspended: `--config <path>` once, nothing else.
        (
            suspended,
            &["suspended", "--config", "a.json", "extra"],
            ActingdCode::SuspendedUsageInvalid,
        ),
        (
            suspended,
            &["suspended", "--state-root", "state"],
            ActingdCode::SuspendedOptionInvalid,
        ),
        (
            suspended,
            &["suspended", "--config"],
            ActingdCode::SuspendedOptionInvalid,
        ),
        (
            suspended,
            &["suspended", "--config", ""],
            ActingdCode::SuspendedOptionInvalid,
        ),
        (
            suspended,
            &["suspended"],
            ActingdCode::SuspendedConfigMissing,
        ),
        // ledger-maintenance: `<operation> --config <path>` and its options, each once.
        (
            maintenance,
            &["ledger-maintenance", "verify"],
            ActingdCode::MaintenanceUsageInvalid,
        ),
        (maintenance, &eleven, ActingdCode::MaintenanceUsageInvalid),
        (
            maintenance,
            &["ledger-maintenance", "compact", "--config", "a.json"],
            ActingdCode::MaintenanceOperationInvalid,
        ),
        (
            maintenance,
            &["ledger-maintenance", "verify", "--state-root", "state"],
            ActingdCode::MaintenanceOptionInvalid,
        ),
        (
            maintenance,
            &[
                "ledger-maintenance",
                "verify",
                "--config",
                "a.json",
                "--config",
                "b.json",
            ],
            ActingdCode::MaintenanceOptionInvalid,
        ),
        (
            maintenance,
            &[
                "ledger-maintenance",
                "verify",
                "--config",
                "a.json",
                "--backup",
            ],
            ActingdCode::MaintenanceOptionInvalid,
        ),
        (
            maintenance,
            &["ledger-maintenance", "verify", "--config", ""],
            ActingdCode::MaintenanceOptionInvalid,
        ),
        (
            maintenance,
            &["ledger-maintenance", "verify", "--backup", "b.sqlite"],
            ActingdCode::MaintenanceConfigMissing,
        ),
    ];
    for (run, values, code) in rows {
        let outcome = refusal(*run, values).outcome();
        assert_eq!(
            outcome.code().as_str(),
            code.code().as_str(),
            "{values:?}: {outcome}"
        );
        assert_eq!(outcome.category().exit_code(), 1, "{values:?}: {outcome}");
    }
}

#[test]
fn gate_install_held_is_taken_only_after_config_and_bounded_at_600_s() {
    // Right after `--config <path>`: the held startup, and the configuration path alone remains.
    let at_limit = held(MAX_INSTALL_TIMEOUT_MS);
    let (remaining, startup) = match parse_install_held_arguments(arguments(&[
        "--config",
        "actingd.json",
        "--install-held",
        at_limit.as_str(),
    ])) {
        Ok(parsed) => parsed,
        Err(error) => panic!("a 600 s held start is refused: {}", error.outcome()),
    };
    assert_eq!(
        startup.map(|startup| startup.timeout_ms),
        Some(MAX_INSTALL_TIMEOUT_MS)
    );
    assert!(matches!(
        parse_arguments(remaining),
        Ok(path) if path == Path::new("actingd.json")
    ));

    // Past 600 s: refused with the contract's code, an error.
    let over = held(MAX_INSTALL_TIMEOUT_MS + 1);
    let refused = match parse_install_held_arguments(arguments(&[
        "--config",
        "actingd.json",
        "--install-held",
        over.as_str(),
    ])) {
        Ok(_) => panic!("a held start past 600 s is taken"),
        Err(error) => error.outcome(),
    };
    assert_eq!(
        refused.code().as_str(),
        RuntimeContractCode::InvalidInstallTimeout.code().as_str(),
        "{refused}"
    );
    assert_eq!(refused.category().exit_code(), 1, "{refused}");

    // Anywhere else, or without its value, it is no held start and the start's arguments are a
    // usage error.
    for values in [
        vec![
            "--install-held",
            at_limit.as_str(),
            "--config",
            "actingd.json",
        ],
        vec!["--config", "actingd.json", "--install-held"],
    ] {
        let (remaining, startup) = match parse_install_held_arguments(arguments(&values)) {
            Ok(parsed) => parsed,
            Err(error) => panic!("{values:?}: {}", error.outcome()),
        };
        assert!(startup.is_none(), "{values:?}");
        let refused = match parse_arguments(remaining) {
            Ok(path) => panic!("{values:?} starts with {}", path.display()),
            Err(error) => error.outcome(),
        };
        assert_eq!(
            refused.code().as_str(),
            ActingdCode::UsageInvalid.code().as_str(),
            "{values:?}: {refused}"
        );
        assert_eq!(refused.category().exit_code(), 1, "{values:?}: {refused}");
    }
}

#[test]
fn gate_check_config_refuses_each_vision_root_with_the_model_folder_reports_code() {
    let base = tempfile::TempDir::new().expect("tempdir");
    let instance_id = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    for (case, expected) in vision_folders::CASES {
        let root = vision_folders::vision_root(base.path(), case);
        let path = base.path().join(format!("{case}.json"));
        let config = serde_json::json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": base.path().join(format!("state-{case}")),
            "bind_host": "127.0.0.1",
            "secret_fingerprint_salt": "0123456789abcdef",
            "vision": {"execution_provider": "cpu", "root": root},
            "instances": [{
                "alias": "neutral.fixture",
                "instance_id": instance_id.transport(),
                "fixture_backend": {
                    "frames": [{"width": 1, "height": 1, "rgb": [1, 2, 3]}],
                    "max_inputs": 0
                }
            }]
        });
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&config).expect("configuration JSON"),
        )
        .expect("write the configuration");
        let refused = match check_config::run(arguments(&[
            "check-config",
            "--config",
            path.to_str().expect("configuration path"),
        ])) {
            Ok(_) => None,
            Err(error) => {
                let outcome = error.outcome();
                assert_eq!(
                    outcome.value("stage").and_then(serde_json::Value::as_str),
                    Some(ActingdLocation::ConfigVision.location().as_str()),
                    "{case}: {outcome}"
                );
                Some(outcome.code())
            }
        };
        assert_eq!(refused, expected.map(VisionListingCode::code), "{case}");
    }
}
