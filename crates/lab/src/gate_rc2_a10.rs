// SPDX-License-Identifier: AGPL-3.0-only

// #411 A10 (v0.12.1-rc.2; register §1.102; exec gate key 85): a `recording.json` that was read
// and does not decode as `actingcommand.lab-recording.v1` makes the Lab part unavailable, reason
// `lab_recording_unreadable`, exactly as an identity mismatch does (contracts/lab-recording.md,
// "Files"): `record status` answers `{status: "unavailable", reason}`, a step action refuses with
// `record_lab_unavailable` and that reason, and `record stop` stops the session with no Lab
// output. The file is never repaired, rewritten, moved or deleted. Requirement basis:
// rd-1019/plan-0121rc2/brief-A10.md, "Requirement" 1 and 2; coordinator decision 2 of 10-19.
//
// The state directory holds the session file `record start` writes and the Lab recording this
// build's `record start` creates for it, then made undecodable in one of two forms: an unknown
// member on `artifact` (a stopped recording otherwise valid), or the file cut off mid-object.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    LAB_RECORD_MARK_SCHEMA, LabOcr, LabRecording, LabStatus, MarkRequest, RecordStartOptions,
    RecordStopOptions, RecordStopOutcome, RecordingLock, StepAction, StepActionKind, record_mark,
    record_start, record_status, record_stop, record_stop_close,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0001";
const TASK_ID: &str = "neutral_task";
const STARTED_AT_UNIX_MS: u64 = 1_700_000_000_000;
const UNREADABLE: &str = "lab_recording_unreadable";

#[derive(Debug, Clone, Copy)]
enum Undecodable {
    UnknownArtifactMember,
    CutOff,
}

// An active session of `INSTANCE` whose Lab recording does not decode. The lock is dropped
// before the directory.
struct Fixture {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Fixture {
    fn new(form: Undecodable) -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        fs::write(
            old_record_path(&state, INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": STARTED_AT_UNIX_MS
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock =
            RecordingLock::acquire(&state, INSTANCE, "gate_rc2_a10").expect("recording lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
        record_start(&lock, &RecordStartOptions::default()).expect("record start");
        let path = recording_path(&state, RECORD_ID);
        let written = fs::read(&path).expect("the recording record start wrote");
        let bytes = match form {
            Undecodable::UnknownArtifactMember => with_unknown_artifact_member(&written, &path),
            Undecodable::CutOff => written[..written.len() / 2].to_vec(),
        };
        fs::write(path, bytes).expect("undecodable recording");
        Self { lock, ocr, temp }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    fn recording_file(&self) -> PathBuf {
        recording_path(&self.state(), RECORD_ID)
    }
}

// The recording stopped with a generated package whose `artifact` carries one member this
// build does not write; without that member the file decodes.
fn with_unknown_artifact_member(written: &[u8], path: &Path) -> Vec<u8> {
    let mut recording: Value = serde_json::from_slice(written).expect("recording JSON");
    let package = path
        .parent()
        .expect("lab directory")
        .join("out")
        .join("neutral_package.zip");
    recording["status"] = json!("stopped");
    recording["artifact"] = json!({
        "container": "zip",
        "path": package.display().to_string(),
        "lab_dir_path": null,
        "byte_count": 1,
        "package_id": "neutral_package",
        "generated_at_unix_ms": STARTED_AT_UNIX_MS
    });
    serde_json::from_value::<LabRecording>(recording.clone())
        .expect("without the unknown member the recording decodes");
    recording["artifact"]["retired_member"] = json!("value");
    let mut bytes = serde_json::to_vec_pretty(&recording).expect("recording bytes");
    bytes.push(b'\n');
    bytes
}

fn unavailable(reason: &str) -> LabStatus {
    LabStatus::Unavailable {
        status: "unavailable".to_owned(),
        reason: reason.to_owned(),
    }
}

// Requirement 1: `record status` answers its session with `lab: {status: "unavailable",
// reason: "lab_recording_unreadable"}` for either form of an undecodable file.
#[test]
fn a10_status_answers_an_undecodable_recording_as_unavailable_with_the_session_record() {
    for form in [Undecodable::UnknownArtifactMember, Undecodable::CutOff] {
        let fixture = Fixture::new(form);

        let lab = record_status(&fixture.state(), INSTANCE).unwrap_or_else(|error| {
            panic!(
                "{form:?}: record status: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });

        assert_eq!(lab, Some(unavailable(UNREADABLE)), "{form:?}");
    }
}

// Requirements 1 and 2: a step action refuses with `record_lab_unavailable` and the reason, as
// for a mismatch; `record stop` (`record_stop`, then `record_stop_close` after the session file
// is stopped) has no Lab output; the file's bytes are the same before and after the three calls.
#[test]
fn a10_step_and_stop_treat_an_undecodable_recording_as_unavailable_and_leave_it_unchanged() {
    let fixture = Fixture::new(Undecodable::UnknownArtifactMember);
    let before = fs::read(fixture.recording_file()).expect("recording file before");

    let refused = record_mark(
        &fixture.lock,
        &MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step_action: Some(StepAction {
                kind: StepActionKind::CloseStep,
                step: None,
            }),
            ..MarkRequest::default()
        },
        false,
        &fixture.ocr,
    )
    .expect_err("a step action on an undecodable recording is refused");
    let stopped = record_stop(&fixture.lock, &RecordStopOptions::default(), &fixture.ocr)
        .unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    let closed = record_stop_close(&fixture.lock).unwrap_or_else(|error| {
        panic!(
            "record stop close: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });

    assert_eq!(
        refused.code, "record_lab_unavailable",
        "{}",
        refused.message
    );
    assert_eq!(
        refused
            .details
            .as_ref()
            .and_then(|details| details.get("reason")),
        Some(&json!(UNREADABLE)),
        "{:?}",
        refused.details
    );
    assert_eq!(
        stopped,
        RecordStopOutcome {
            lab_status: None,
            lab: None
        }
    );
    assert_eq!(closed, None);
    let after = fs::read(fixture.recording_file()).expect("recording file after");
    assert_eq!(after, before, "the undecodable recording is left as it is");
}
