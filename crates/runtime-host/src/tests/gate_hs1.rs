// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-1 (register §1.267; design `rd-1031/backend-switch/model-backend-switch.md` §2.1, §7
//! HS-1): the backends each kind of device has, which the host fills into the configuration's
//! check context from the device layer's own rule (`backend_kind_rules`), refuse a backend the
//! instance's kind does not have as `config_value_invalid` (`backend_kind_unsupported`) for its
//! key: a handset's Nemu IPC (touch and capture) and DroidCast, an emulator's scrcpy. Every other
//! backend of the kind passes. The ids are made up.

use actingcommand_contract::RuntimeConfigManifest;
use actingcommand_runtime_config::{
    CAPTURE_BACKEND, CAPTURE_BACKEND_TOKENS, CheckContext, ConfigCode, ConfigFailure,
    ConfigValueProblem, DefaultsFile, EffectiveConfig, TOUCH_BACKEND, TOUCH_BACKEND_TOKENS,
    instance_key, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const INSTANCE: &str = "instance_0123456789abcdef0123456789abcdef";

/// The context the host checks the configuration with: its own heartbeat bound does not matter
/// here, the device layer's bounds and backends do.
fn context() -> CheckContext {
    CheckContext::new(5_000, crate::recovery_ladder_device_bounds())
        .with_backends(crate::backend_kind_rules())
}

/// The file layer of one instance entry naming `device` (none for an emulator) and these
/// backends, as actingd hands it over, checked whole.
fn check(device: Option<Value>, touch: &str, capture: &str) -> Result<(), ConfigFailure> {
    let mut entry = json!({"instance_id": INSTANCE, "alias": "node",
                           "touch_backend": touch, "capture_backend": capture});
    if let Some(device) = device {
        entry["device"] = device;
    }
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let parameters = render_parameters(
        &mechanism_rows(),
        &[INSTANCE.to_owned()],
        &defaults,
        &json!({ "instances": [entry] }),
        &|_| None,
    )
    .expect("every key resolves");
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table")
    .check(&context())
}

fn assert_kind_refused(refused: &ConfigFailure, member: &str, case: &str) {
    assert_eq!(
        refused.failure_code(),
        ConfigCode::ConfigValueInvalid,
        "{case}"
    );
    assert_eq!(
        refused.config_key(),
        instance_key(INSTANCE, member),
        "{case}"
    );
    assert_eq!(
        refused.problem(),
        Some(ConfigValueProblem::BackendKindUnsupported),
        "{case}"
    );
}

#[test]
fn hs1_handset_nemu_refused_kind() {
    let handset = || Some(json!({"kind": "handset", "transport": "usb"}));
    let rules = crate::backend_kind_rules().handset;
    for (touch, has) in [
        ("adb_shell_input", true),
        ("maatouch", true),
        ("minitouch", true),
        ("nemu_ipc", false),
    ] {
        assert_eq!(rules.has_touch(touch), has, "a handset's {touch}");
    }
    for (capture, has) in [
        ("adb", true),
        ("droidcast_raw", false),
        ("nemu_ipc", false),
        ("scrcpy", true),
    ] {
        assert_eq!(rules.has_capture(capture), has, "a handset's {capture}");
    }

    // Nemu touch on a handset, beside capture it has, and Nemu or DroidCast capture beside touch
    // it has: refused for the key of the backend it does not have.
    let case = "a handset's nemu_ipc touch";
    let refused = check(handset(), "nemu_ipc", "adb").expect_err(case);
    assert_kind_refused(&refused, TOUCH_BACKEND, case);
    for capture in ["nemu_ipc", "droidcast_raw"] {
        let case = format!("a handset's {capture} capture");
        let refused = check(handset(), "adb_shell_input", capture).expect_err(&case);
        assert_kind_refused(&refused, CAPTURE_BACKEND, &case);
    }

    // Every pair of backends a handset has passes.
    for touch in ["adb_shell_input", "maatouch", "minitouch"] {
        for capture in ["adb", "scrcpy"] {
            check(handset(), touch, capture)
                .unwrap_or_else(|refused| panic!("a handset's {touch} and {capture}: {refused:?}"));
        }
    }
}

#[test]
fn hs1_emulator_scrcpy_refused_kind() {
    let rules = crate::backend_kind_rules().emulator;
    for touch in TOUCH_BACKEND_TOKENS {
        assert!(rules.has_touch(touch), "an emulator's {touch}");
    }
    for capture in CAPTURE_BACKEND_TOKENS {
        assert_eq!(
            rules.has_capture(capture),
            *capture != "scrcpy",
            "an emulator's {capture}"
        );
    }

    // scrcpy capture on an emulator, whether its entry names no `device` or `kind` `emulator`:
    // refused for the capture key.
    for device in [None, Some(json!({"kind": "emulator"}))] {
        let case = format!("scrcpy capture on an emulator, device {device:?}");
        let refused = check(device, "adb_shell_input", "scrcpy").expect_err(&case);
        assert_kind_refused(&refused, CAPTURE_BACKEND, &case);
    }

    // Every touch backend beside Nemu capture, and adb or DroidCast capture beside touch other
    // than Nemu, passes on an emulator.
    for touch in TOUCH_BACKEND_TOKENS {
        check(None, touch, "nemu_ipc")
            .unwrap_or_else(|refused| panic!("an emulator's {touch} and nemu_ipc: {refused:?}"));
    }
    for capture in ["adb", "droidcast_raw"] {
        check(None, "maatouch", capture)
            .unwrap_or_else(|refused| panic!("an emulator's maatouch and {capture}: {refused:?}"));
    }
}
