// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7, G9; inventory-apps key 12): actingd's subcommand
// arguments. `unlock-owner`, `suspended` and `ledger-maintenance` refuse a malformed argument
// list before they read any file, each with its own registered code (contracts/outcome-codes,
// the actingd fragment; Workflow #378 A2a), and the refusal is an error: one report line with
// its outcome and exit 1 (Workflow #378 model §7.4, §7.6). The registry half of the
// ledger-maintenance row is `gate_a2a`. Workflow #434 LAB-6S4a (0.12.62): `--install-held` went
// with the held start; the start's arguments that name it are a usage error.
//
// G15 (inventory-apps key 39): `check-config` refuses each vision root of
// tests/support/vision_folders.rs at its vision stage with the code vision-provider-check's model
// folder report gives the same root (that program's `gate_382_a1`), and passes the valid one.

use super::*;
use actingcommand_contract::IdentifierIssuer;
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
        // #422 LN-VERIFY: `--full`, a flag without a value, at most once, as `--frame-query`.
        (
            maintenance,
            &[
                "ledger-maintenance",
                "verify",
                "--config",
                "a.json",
                "--full",
                "--full",
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
                "--frame-query",
                "--frame-query",
            ],
            ActingdCode::MaintenanceOptionInvalid,
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

// Workflow #434 LAB-6S4a (0.12.62): the held start is gone. `--install-held` is no option of the
// start any more: in every place, with or without its value, the start's arguments are a usage
// error, exit 1, before any file is read.
#[test]
fn gate_install_held_is_refused_as_a_usage_error() {
    let held = r#"{"transition_id":"transition-g9","request_id":"request_00000000000000000000000000000001"}"#;
    for values in [
        vec!["--config", "actingd.json", "--install-held", held],
        vec!["--install-held", held, "--config", "actingd.json"],
        vec!["--config", "actingd.json", "--install-held"],
    ] {
        let refused = match parse_arguments(arguments(&values)) {
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
                Some(outcome.code().as_str().to_owned())
            }
        };
        assert_eq!(
            refused.as_deref(),
            expected.map(|code| code.code().as_str()),
            "{case}"
        );
    }
}
