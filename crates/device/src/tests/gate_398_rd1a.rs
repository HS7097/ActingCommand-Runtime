// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1a (model-398 v1.1 §12, rows T1–T3): a device serial never leaves the device
//! layer. Foreign adb text keeps a bound serial only as its label token, `Debug` and `Display`
//! show only the label, and a target adb does not answer for never lists the other devices.
//! Every serial is a neutral fake and every adb answer is written by hand; no process runs.

use crate::adb::recovery::{command_evidence, ensure_input_device_with_commands};
use crate::maatouch::unavailable_device_error;
use crate::{
    AdbConfig, CaptureBackendConfig, CaptureSelectionContext, CommandOutput, DeviceError,
    DeviceSerial, DeviceTarget,
};
use std::time::Duration;

const SERIAL: &str = "TESTSERIAL0001";
const OTHER_SERIAL: &str = "TESTSERIAL0002";
const ALIAS: &str = "neutral.handset";
const LABEL: &str = "handset:neutral.handset";
const TOKEN: &str = "[device:handset:neutral.handset]";

fn output(stdout: &str, stderr: &str) -> CommandOutput {
    CommandOutput {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        stdout_lossy_decode: false,
        stderr_lossy_decode: false,
    }
}

/// No spelling of `serial` (as given or lowercase) is in `text`.
fn assert_unnamed(text: &str, serial: &str) {
    assert!(!text.contains(serial), "{text}");
    assert!(
        !text
            .to_ascii_lowercase()
            .contains(&serial.to_ascii_lowercase()),
        "{text}"
    );
}

/// T1 (§3.5 rules 4 and 5): adb names the bound serial in every answer of the input target's
/// connection attempt. The kept stdout, stderr and error text carry the label token instead and
/// say so with `raw_text_redacted`; the endpoint is the label.
#[test]
fn rd1a_t1_adb_text_keeps_a_bound_serial_only_as_its_label_token() {
    let serial = DeviceSerial::handset(SERIAL, ALIAS);
    let missing = format!("error: device '{SERIAL}' not found\n");
    let result = ensure_input_device_with_commands(
        serial.adb_arg(),
        Duration::from_secs(12),
        |args, _remaining| {
            if args.first() == Some(&"connect") {
                return Ok(output(&format!("failed to connect to '{SERIAL}'\n"), ""));
            }
            Err(DeviceError::fatal(format!(
                "adb {} failed with exit code 1\nstdout:\n\nstderr:\n{missing}",
                args.join(" ")
            ))
            .with_adb_command(command_evidence(&output("", &missing), false, Some(1))))
        },
    );
    let error = match result {
        Ok(_) => panic!("a device adb cannot find is not ready"),
        Err(error) => error,
    };
    assert_unnamed(error.message(), SERIAL);
    assert!(error.message().contains(TOKEN), "{}", error.message());
    let report = error
        .adb_recovery()
        .expect("the connection attempt is kept");
    assert_eq!(report.endpoint.text, LABEL);
    assert_unnamed(&report.initial_error.text, SERIAL);
    assert!(report.steps.iter().any(|step| step.command.is_some()));
    for step in &report.steps {
        if let Some(text) = &step.error {
            assert_unnamed(&text.text, SERIAL);
        }
        if let Some(command) = &step.command {
            assert_unnamed(&command.stdout.text, SERIAL);
            assert_unnamed(&command.stderr.text, SERIAL);
            assert!(command.raw_text_redacted, "{:?}", step.phase);
            assert!(
                command.stdout.text.contains(TOKEN) || command.stderr.text.contains(TOKEN),
                "{:?}",
                step.phase
            );
        }
    }
}

/// T2 (§3.5 rule 1): `Debug` and `Display` of the serial, of a target addressed by it and of the
/// capture selection opened on that target show the label, never the serial.
#[test]
fn rd1a_t2_debug_and_display_show_only_the_label() {
    let serial = DeviceSerial::handset(SERIAL, ALIAS);
    let target = DeviceTarget {
        serial: Some(serial.clone()),
        ..DeviceTarget::default()
    };
    let config = CaptureBackendConfig::new(AdbConfig::default(), target.clone());
    let selection = CaptureSelectionContext::opened("neutral-adb".to_owned(), &target, &config);
    for text in [
        format!("{serial:?}"),
        format!("{serial}"),
        format!("{target:?}"),
        format!("{target}"),
        format!("{selection:?}"),
    ] {
        assert_unnamed(&text, SERIAL);
    }
    assert_eq!(format!("{serial}"), LABEL);
    assert_eq!(format!("{target}"), LABEL);
    assert_eq!(selection.configured_serial.as_deref(), Some(LABEL));
    assert_eq!(selection.selected_serial, LABEL);
}

/// T3 (§3.5 rule 3): when adb does not answer for a MaaTouch target, the error reports how many
/// devices `adb devices -l` listed and how many are in each state, and names none of them.
#[test]
fn rd1a_t3_an_unavailable_target_reports_device_counts_not_serials() {
    let listing = output(
        &format!(
            "List of devices attached\n\
             {SERIAL}         device usb:1-1 product:neutral model:neutral device:neutral transport_id:7\n\
             {OTHER_SERIAL}         offline transport_id:8\n\
             127.0.0.1:16384        device product:neutral model:neutral device:neutral transport_id:3\n\n"
        ),
        "",
    );
    let error = unavailable_device_error(
        LABEL,
        &DeviceError::transient("get-state did not answer"),
        Ok(listing),
    );
    let text = format!("{} {error}", error.message());
    assert_unnamed(&text, SERIAL);
    assert_unnamed(&text, OTHER_SERIAL);
    assert!(error.message().contains("listed=3"), "{}", error.message());
    assert!(error.message().contains("device=2"), "{}", error.message());
    assert!(error.message().contains("offline=1"), "{}", error.message());
}
