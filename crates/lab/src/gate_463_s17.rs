// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-7 (contracts/lab-recording.md, "A recording holds its instance"; register §1.424): a
// recording holds its instance, and the recording library keeps the hold's facts the `record`
// command hands it in (the session header's `hold`) and says what they mean; it asks no Runtime.
// - `s17_the_session_header_carries_the_hold_and_the_lab_reads_it`: a header with an own hold, as
//   the `record` command writes it: the Lab's own reading of the header (`read_old_record`, the
//   view every Lab command takes) carries the hold; the Lab recording starts on it; `record
//   status`'s hold is then `own` with its reason, `{state: own, reason}` (fix round 2: every
//   assertion fails when the header's hold is not read).
// - `s17_record_status_shows_each_of_the_four_states`: own (an active session under its own
//   pause), foreign with the other party's reason, none (no hold, and no header), kept (an own
//   pause the end could not release, and one of an ended session whose release did not get as far).
// - `s17_what_a_recording_holds_comes_from_what_record_start_met`: the pause it set is its own; a
//   pause the instance showed is another party's unless it is the replaced recording's own (taken
//   over); with nothing listening the replaced recording's own pause is carried over, nothing
//   else; at the end an own pause is kept until its release, another party's is dropped.
// - `s17_a_recording_without_a_hold_is_unchanged`: what keeps a header without a hold as it was
//   (fix round 2: every assertion fails when it is taken away): `record status` of a header
//   without the member is exactly `{state: none}`, no `reason`; a hold not kept writes no `kept`,
//   one kept writes it; a hold read without `kept` is not kept. (The `record` command's header
//   without the member, byte for byte, is pinned by actinglab's gate_463_s17, whose start with
//   nothing listening writes exactly the members it wrote before the slice.)
// Temporary state directories only; no Runtime, no device, no clock read by the tests.

use crate::recording::store::{old_record_path, read_old_record};
use crate::{
    LAB_RECORDING_HOLD_PREFIX, RecordStartOptions, RecordingHold, RecordingHoldFound,
    RecordingHoldState, RecordingHoldView, RecordingLock, record_hold_status, record_start,
};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s17";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const OWN: &str = "lab-record-1700000000000";
const OTHER: &str = "operator";

/// A state directory of its own.
struct Bench {
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        let bench = Self {
            temp: TempDir::new().expect("temporary directory"),
        };
        fs::create_dir_all(bench.state()).expect("state directory");
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    /// The session header with `status` and, when given, `hold`, as the `record` command writes
    /// it.
    fn header(&self, status: &str, hold: Option<Value>) {
        let mut header = json!({
            "schema_version": "session-record-v0",
            "record_id": RECORD_ID,
            "task_id": TASK_ID,
            "instance": INSTANCE,
            "status": status,
            "holder": null,
            "lease_id": null,
            "started_at_unix_ms": STARTED_AT,
            "updated_at_unix_ms": STARTED_AT,
            "steps": []
        });
        if let Some(hold) = hold {
            header["hold"] = hold;
        }
        let bytes = serde_json::to_vec_pretty(&header).expect("session JSON");
        fs::write(old_record_path(&self.state(), INSTANCE), bytes).expect("session file");
    }

    fn lock(&self) -> RecordingLock {
        RecordingLock::acquire(&self.state(), INSTANCE, "gate_463_s17", "Lab-CLI")
            .expect("recording lock")
    }

    fn start(&self) {
        let started =
            record_start(&self.lock(), &RecordStartOptions::default()).expect("record start");
        assert_eq!(started.status, "active", "{started:?}");
    }

    fn hold(&self) -> RecordingHoldView {
        record_hold_status(&self.state(), INSTANCE).expect("record status's hold")
    }

    /// The hold of the header as the Lab's own reading of it (`read_old_record`) carries it.
    fn read_hold(&self) -> Option<RecordingHold> {
        read_old_record(&self.state(), INSTANCE)
            .expect("the session header reads")
            .expect("a session header")
            .hold
    }
}

fn hold(reason: &str, own: bool, kept: bool) -> RecordingHold {
    RecordingHold {
        reason: reason.to_owned(),
        own,
        kept,
    }
}

fn view(state: RecordingHoldState, reason: Option<&str>) -> RecordingHoldView {
    RecordingHoldView {
        state,
        reason: reason.map(str::to_owned),
    }
}

#[test]
fn s17_the_session_header_carries_the_hold_and_the_lab_reads_it() {
    let held = Bench::new();
    held.header("active", Some(json!({"reason": OWN, "own": true})));
    // The Lab's own reading of the header carries the hold the command wrote.
    assert_eq!(
        held.read_hold(),
        Some(hold(OWN, true, false)),
        "the Lab reads the header's hold"
    );
    // The Lab recording starts on such a header, and the hold stays the header's.
    held.start();
    assert_eq!(held.read_hold(), Some(hold(OWN, true, false)));
    assert_eq!(
        held.hold(),
        view(RecordingHoldState::Own, Some(OWN)),
        "the header's hold is the recording's own"
    );
    assert_eq!(
        json!(held.hold()),
        json!({"state": "own", "reason": OWN}),
        "record status shows its state and reason"
    );
}

#[test]
fn s17_record_status_shows_each_of_the_four_states() {
    let cases = [
        (
            "active",
            Some(json!({"reason": OWN, "own": true})),
            view(RecordingHoldState::Own, Some(OWN)),
        ),
        (
            "active",
            Some(json!({"reason": OTHER, "own": false})),
            view(RecordingHoldState::Foreign, Some(OTHER)),
        ),
        ("active", None, view(RecordingHoldState::Unheld, None)),
        (
            "archived",
            Some(json!({"reason": OWN, "own": true, "kept": true})),
            view(RecordingHoldState::Kept, Some(OWN)),
        ),
        (
            "stopped",
            Some(json!({"reason": OWN, "own": true, "kept": true})),
            view(RecordingHoldState::Kept, Some(OWN)),
        ),
        // An ended session whose release did not get as far as writing `kept`.
        (
            "archived",
            Some(json!({"reason": OWN, "own": true})),
            view(RecordingHoldState::Kept, Some(OWN)),
        ),
    ];
    for (status, given, expected) in cases {
        let bench = Bench::new();
        bench.header(status, given.clone());
        assert_eq!(bench.hold(), expected, "{status} {given:?}");
    }
    // Without a header the recording holds nothing.
    assert_eq!(Bench::new().hold(), view(RecordingHoldState::Unheld, None));
    // The four words of the state.
    assert_eq!(
        [
            RecordingHoldState::Own,
            RecordingHoldState::Foreign,
            RecordingHoldState::Unheld,
            RecordingHoldState::Kept
        ]
        .map(|state| json!(state)),
        [json!("own"), json!("foreign"), json!("none"), json!("kept")]
    );
    assert_eq!(
        json!(view(RecordingHoldState::Unheld, None)),
        json!({"state": "none"})
    );
}

#[test]
fn s17_what_a_recording_holds_comes_from_what_record_start_met() {
    assert_eq!(RecordingHold::reason_for(STARTED_AT), OWN);
    assert!(OWN.starts_with(LAB_RECORDING_HOLD_PREFIX));
    let replaced_own = hold(OWN, true, false);
    let replaced_kept = hold(OWN, true, true);
    let replaced_foreign = hold(OWN, false, false);
    let paused = RecordingHoldFound::Paused("lab-record-1700000000001".to_owned());
    let shown_own = RecordingHoldFound::Shown(OWN.to_owned());
    let shown_other = RecordingHoldFound::Shown(OTHER.to_owned());
    let cases = [
        // The pause the start set is its own, whatever it replaced.
        (
            &paused,
            None,
            Some(hold("lab-record-1700000000001", true, false)),
        ),
        (
            &paused,
            Some(&replaced_own),
            Some(hold("lab-record-1700000000001", true, false)),
        ),
        // A pause the instance showed: another party's...
        (&shown_other, None, Some(hold(OTHER, false, false))),
        (
            &shown_other,
            Some(&replaced_own),
            Some(hold(OTHER, false, false)),
        ),
        (&shown_own, None, Some(hold(OWN, false, false))),
        (
            &shown_own,
            Some(&replaced_foreign),
            Some(hold(OWN, false, false)),
        ),
        // ... unless it is the replaced recording's own, kept or not: taken over.
        (
            &shown_own,
            Some(&replaced_own),
            Some(hold(OWN, true, false)),
        ),
        (
            &shown_own,
            Some(&replaced_kept),
            Some(hold(OWN, true, false)),
        ),
        // Nothing listening: the replaced recording's own pause is carried over, nothing else.
        (&RecordingHoldFound::NoRuntime, None, None),
        (
            &RecordingHoldFound::NoRuntime,
            Some(&replaced_foreign),
            None,
        ),
        (
            &RecordingHoldFound::NoRuntime,
            Some(&replaced_kept),
            Some(hold(OWN, true, false)),
        ),
    ];
    for (found, replaced, expected) in cases {
        let held = RecordingHold::at_start(found, replaced);
        assert_eq!(held, expected, "{found:?} {replaced:?}");
        let taken_over = held.as_ref().is_some_and(|held| held.taken_over(replaced));
        let expect_taken_over = !matches!(found, RecordingHoldFound::Paused(_))
            && expected.as_ref().is_some_and(|expected| expected.own);
        assert_eq!(taken_over, expect_taken_over, "{found:?} {replaced:?}");
    }
    // At the end: an own pause kept until its release, another party's dropped.
    assert_eq!(replaced_own.ended(), Some(hold(OWN, true, true)));
    assert_eq!(replaced_kept.ended(), Some(hold(OWN, true, true)));
    assert_eq!(hold(OTHER, false, false).ended(), None);
}

#[test]
fn s17_a_recording_without_a_hold_is_unchanged() {
    let bench = Bench::new();
    bench.header("active", None);
    // `record status` of a header without the member: exactly `none`, without a `reason` member.
    assert_eq!(json!(bench.hold()), json!({"state": "none"}));
    // A hold that is not kept writes no `kept`; one that is writes it.
    assert_eq!(
        json!(hold(OWN, true, false)),
        json!({"reason": OWN, "own": true})
    );
    assert_eq!(
        json!(hold(OWN, true, true)),
        json!({"reason": OWN, "own": true, "kept": true})
    );
    assert_eq!(
        serde_json::from_value::<RecordingHold>(json!({"reason": OTHER, "own": false}))
            .expect("a hold without kept"),
        hold(OTHER, false, false)
    );
}
