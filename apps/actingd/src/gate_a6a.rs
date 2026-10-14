// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #378 A6a (model sections 7.4, 7.6, 8.2; apps gate 48): each daemon outcome is one
// developer line and one `OUTCOME actingd:` line written together, at the level and with the
// exit code of its category; a start refusal goes to the fallback log only when nobody keeps
// stderr, with `install_root`; and every subcommand exit is one report line with its outcome.

use super::*;
use actingcommand_contract::outcome::Category;
use serde_json::Value;

// The two lines of one outcome: the developer line and the envelope of the OUTCOME line.
fn split(lines: &str) -> (String, Value) {
    let mut lines = lines.lines();
    let developer = lines.next().expect("the developer line").to_owned();
    let envelope = lines
        .next()
        .and_then(|line| line.strip_prefix("OUTCOME actingd: "))
        .map(|envelope| serde_json::from_str(envelope).expect("the envelope"))
        .expect("the OUTCOME line");
    assert_eq!(lines.next(), None, "exactly two lines");
    (developer, envelope)
}

#[test]
fn a6a_each_outcome_is_one_developer_line_and_one_outcome_line_by_its_category() {
    let rows = [
        (
            Outcome::new(ActingdCode::RuntimeReady).pid(7_u32),
            "INFO",
            Category::Success,
            0,
        ),
        (
            Outcome::new(ActingdCode::RuntimeStopped),
            "INFO",
            Category::Info,
            0,
        ),
        (
            Outcome::new(ActingdCode::EnvOverrideIgnored).variable("ACTINGCOMMAND_ADB_PATH"),
            "WARN",
            Category::Warning,
            0,
        ),
        (
            Outcome::new(ActingdCode::UsageInvalid),
            "ERROR",
            Category::Error,
            1,
        ),
        (
            Outcome::new(ActingdCode::AdbInstallPinInvalid),
            "FATAL",
            Category::Fatal,
            2,
        ),
    ];
    for (outcome, level, category, exit_code) in rows {
        let code = outcome.code().as_str().to_owned();
        for developer in [None, Some("developer\ntext")] {
            let (line, envelope) = split(&daemon_lines(&outcome, developer));
            assert!(
                line.starts_with(&format!("{level} actingd: {code}")),
                "{line}"
            );
            assert!(!line.contains('\n'), "{line}");
            if developer.is_some() {
                assert!(line.ends_with("developer text"), "{line}");
            }
            assert_eq!(envelope["schema_version"], "actingcommand.outcome.v1");
            assert_eq!(envelope["code"], code.as_str());
            assert_eq!(envelope["category"], category.as_str());
            assert_eq!(outcome.category(), category);
            assert_eq!(outcome.category().exit_code(), exit_code, "{code}");
        }
    }
}

#[test]
fn a6a_a_start_refusal_goes_to_the_fallback_log_only_when_stderr_is_not_kept() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let logs = temp.path().join("ActingCommand").join("logs");
    let install_root = temp.path().join("install");
    let refused = ActingdError::config(ActingdCode::UsageInvalid);

    // A file or a pipe keeps stderr: nothing is appended to the fallback log.
    let mut stderr = Vec::new();
    let category = write_start_refusal(
        &refused,
        Some(&install_root),
        &mut stderr,
        false,
        Some(&logs),
        1_800_000_000_000,
    );
    assert_eq!(category, Category::Error);
    let (line, envelope) = split(&String::from_utf8(stderr).expect("stderr text"));
    assert!(line.starts_with("ERROR actingd: usage_invalid"), "{line}");
    assert_eq!(envelope["code"], "usage_invalid");
    assert_eq!(
        envelope["values"]["install_root"],
        install_root.to_string_lossy().as_ref()
    );
    assert!(!logs.exists());

    // A console, or no stderr at all: the same lines are appended to the fallback log too.
    let mut stderr = Vec::new();
    let category = write_start_refusal(
        &refused,
        Some(&install_root),
        &mut stderr,
        true,
        Some(&logs),
        1_800_000_000_001,
    );
    assert_eq!(category, Category::Error);
    let written =
        std::fs::read_to_string(logs.join("actingd-1800000000001.log")).expect("the fallback log");
    assert_eq!(written.as_bytes(), stderr.as_slice());

    // A refusal before the installation is resolved has no install_root; a log that cannot be
    // written is one more developer line, and the refusal's category still decides the exit.
    let mut stderr = Vec::new();
    let category = write_start_refusal(&refused, None, &mut stderr, true, None, 1_800_000_000_002);
    assert_eq!(category, Category::Error);
    let text = String::from_utf8(stderr).expect("stderr text");
    let mut lines = text.lines();
    let (_, envelope) = split(&format!(
        "{}\n{}\n",
        lines.next().expect("the developer line"),
        lines.next().expect("the OUTCOME line")
    ));
    assert_eq!(envelope["values"].get("install_root"), None);
    assert!(
        lines
            .next()
            .is_some_and(|line| line.starts_with("WARN actingd: actingd_fallback_log_failed")),
        "{text}"
    );
    assert_eq!(lines.next(), None, "{text}");
}

#[test]
fn a6a_every_subcommand_exit_is_one_report_line_with_its_outcome() {
    let arguments = |values: &[&str]| {
        values
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    };
    let missing = tempfile::TempDir::new()
        .expect("tempdir")
        .path()
        .join("missing.json");
    let missing = missing.to_str().expect("config path");
    let rows = [
        (
            check_config::SCHEMA_VERSION,
            ActingdCode::CheckConfigReportEncodeFailed,
            check_config::run(arguments(&["check-config", "--state-root", "x"])),
            "check_config_option_invalid",
            None,
            None,
        ),
        (
            owner_unlock::SCHEMA_VERSION,
            ActingdCode::UnlockOwnerReportEncodeFailed,
            owner_unlock::run(arguments(&[
                "unlock-owner",
                "--config",
                missing,
                "--actor",
                "operator",
            ])),
            "config_unavailable",
            Some("config_load"),
            Some(("journal_appended", Value::Bool(false))),
        ),
        (
            ledger_maintenance::SCHEMA_VERSION,
            ActingdCode::MaintenanceReceiptEncodeFailed,
            ledger_maintenance::run(arguments(&["ledger-maintenance", "verify"])),
            "maintenance_usage_invalid",
            None,
            None,
        ),
        (
            suspended::SCHEMA_VERSION,
            ActingdCode::SuspendedReportEncodeFailed,
            Ok(Reported::new(Outcome::new(ActingdCode::SuspendedReported))
                .with("through_sequence", Value::from(3))),
            "suspended_reported",
            None,
            Some(("through_sequence", Value::from(3))),
        ),
    ];
    for (schema_version, encode_failed, result, code, stage, member) in rows {
        let (outcome, developer, line) =
            report_line(schema_version, encode_failed, result, Detail::Full);
        assert!(!line.contains('\n'), "{line}");
        let report: Value = serde_json::from_str(&line).expect("one JSON line");
        assert_eq!(report["schema_version"], schema_version, "{line}");
        assert_eq!(report["outcome"]["code"], code, "{line}");
        assert_eq!(outcome.code().as_str(), code);
        assert_eq!(
            report["outcome"]["values"]
                .get("stage")
                .and_then(Value::as_str),
            stage,
            "{line}"
        );
        assert_eq!(developer.is_some(), outcome.category() != Category::Success);
        for removed in ["status", "error", "code", "operation", "detail"] {
            assert_eq!(report.get(removed), None, "{removed}: {line}");
        }
        if let Some((member, value)) = member {
            assert_eq!(report[member], value, "{line}");
        }
        let expected_exit = if code == "suspended_reported" { 0 } else { 1 };
        assert_eq!(outcome.category().exit_code(), expected_exit, "{line}");
    }
}
