// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S5, model M-PKG v1.1 §4.6 S5-T4: `actinglab package digest` is retired. It is
// an unknown package command: a usage error (`validation_failed`, exit 2), whatever package it
// is given.

use super::*;

#[test]
fn s5_t4_package_digest_is_an_unknown_package_command() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("package");
    fs::create_dir(&package).unwrap();
    fs::write(
        package.join("control.json"),
        br#"{"package_id":"neutral.test.task"}"#,
    )
    .unwrap();

    let result = run_cli(
        [
            "--json",
            "package",
            "digest",
            "--package",
            package.to_str().unwrap(),
        ],
        true,
    );

    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().expect("usage error");
    assert_eq!(error.code, "validation_failed");
    assert!(
        error.message.contains("unknown package command: digest"),
        "{}",
        error.message
    );
}
