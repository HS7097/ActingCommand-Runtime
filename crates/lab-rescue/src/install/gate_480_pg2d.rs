// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2d (0.12.74; register §1.481; coordinator work order
// `rd-1118/brief-480-pg2d-pause-records-resume-cause.md`, item 5): PG-2c's leftover test of the
// ruling that had none (fix round 2, item 10; the review of #1826's last round, "第 10 条"): the
// answer of an instance's addition whose pause of the added instance the Runtime answered before
// the instance was quiet. On `gate_421_inst5`'s root and the fake host of `fixture.rs`, the fake
// Runtime answering the added instance's pause so (`unquiet_pause`: paused, standing, and the
// module's client takes it as `scheduling_hold_not_quiet`): the command ends `post_check_failed`
// for the added instance, caused by `scheduling_hold_not_quiet`; its step `pause_added` says the
// pause was set and is not quiet (`set`, `not_quiet`); `next` names no pause by hand (one would
// only answer `scheduling_already_paused`); the pause stands and was asked once; the hold stays.
// No device, no emulator, no sleep.

use super::fixture::FakeHost;
use super::gate_421_inst5::{ADDED, add, calls, running, top_code, top_value};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn pg2d_an_added_instance_whose_pause_stands_names_no_pause_by_hand() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = running(temp.path(), "standing-added", &host);
    *host.runtime.unquiet_pause.borrow_mut() = vec![ADDED.to_owned()];

    let (line, exit) = add(&host, &root, ADDED, 2);

    assert_eq!(exit, 1, "{line}");
    assert_eq!(top_code(&line), "install_command_refused", "{line}");
    assert_eq!(top_value(&line, "install_refusal"), "post_check_failed");
    assert_eq!(top_value(&line, "alias"), ADDED);
    assert!(
        line["outcome"]
            .to_string()
            .contains("\"scheduling_hold_not_quiet\""),
        "the refusal's cause: {line}"
    );
    let step = line["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["step"] == "pause_added"))
        .unwrap_or_else(|| panic!("the step pause_added: {line}"));
    assert_eq!(step["result"]["set"], json!(true), "{line}");
    assert_eq!(step["result"]["not_quiet"], json!(true), "{line}");
    assert!(
        line["next"]["pause_added"].is_null(),
        "no pause by hand is named: {line}"
    );
    assert_eq!(line["hold"]["kept"], json!(true), "{line}");

    // The pause the Runtime answered stands, asked once.
    assert_eq!(
        host.runtime.pause_of(ADDED).as_deref(),
        Some("instance.added")
    );
    let prefix = format!("pause {ADDED} ");
    let pauses_of_added = calls(&host)
        .into_iter()
        .filter(|call| call.starts_with(prefix.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        pauses_of_added,
        [format!("pause {ADDED} instance.added not quiet")],
        "not sent again"
    );
}
