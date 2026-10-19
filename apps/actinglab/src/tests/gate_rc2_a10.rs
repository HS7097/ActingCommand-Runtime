// SPDX-License-Identifier: AGPL-3.0-only

// #411 A10 (v0.12.1-rc.2; register §1.102; exec gate key 85): `record status` answers its session
// when the Lab recording under the session's record id does not decode. A recording another
// session left under the same record id (an explicit `--record-id`), written with an `artifact`
// member this build does not write, stays where it is: `record start` keeps the session with the
// Lab part unavailable, and `record status` answers exit 0 with its `record` and `lab: {status:
// "unavailable", reason: "lab_recording_unreadable"}` (contracts/lab-recording.md, "Files" and
// "record status"). Requirement basis: rd-1019/plan-0121rc2/brief-A10.md, "Requirement" 1.

use super::*;

const INSTANCE: &str = "neutral-instance";
const RECORD_ID: &str = "record_0001";
const TASK_ID: &str = "neutral_task";

// The stopped recording an earlier session of `RECORD_ID` left in `lab_dir`, with one
// `artifact` member this build does not write; without that member it decodes.
fn earlier_recording(lab_dir: &std::path::Path) -> Vec<u8> {
    let mut recording = json!({
        "schema_version": "actingcommand.lab-recording.v1",
        "record_id": RECORD_ID,
        "record_started_at_unix_ms": 1_700_000_000_000_u64,
        "task_id": TASK_ID,
        "instance": INSTANCE,
        "status": "stopped",
        "game": null,
        "server": null,
        "locale": null,
        "defaults": {
            "template_threshold": 0.95,
            "color_max_distance": 20,
            "match_metric": "ccoeff_normed"
        },
        "coordinate_space": null,
        "created_at_unix_ms": 1_700_000_000_000_u64,
        "updated_at_unix_ms": 1_700_000_000_000_u64,
        "steps": [],
        "artifact": {
            "container": "zip",
            "path": lab_dir.join("out").join("neutral_package.zip").display().to_string(),
            "lab_dir_path": null,
            "byte_count": 1,
            "package_id": "neutral_package",
            "generated_at_unix_ms": 1_700_000_000_000_u64
        }
    });
    serde_json::from_value::<actingcommand_lab::LabRecording>(recording.clone())
        .expect("without the unknown member the recording decodes");
    recording["artifact"]["retired_member"] = json!("value");
    serde_json::to_vec_pretty(&recording).unwrap()
}

#[test]
fn a10_record_status_with_an_undecodable_recording_exits_zero_with_its_record() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    set_config_env(temp.path().join("config.json"));
    let state_dir = temp.path().join("session");
    let lab_dir = state_dir
        .join("record-artifacts")
        .join(RECORD_ID)
        .join("lab");
    fs::create_dir_all(&lab_dir).unwrap();
    fs::write(lab_dir.join("recording.json"), earlier_recording(&lab_dir)).unwrap();

    let start = run_cli(
        [
            "--json",
            "--instance",
            INSTANCE,
            "record",
            "start",
            "--state-dir",
            state_dir.to_str().unwrap(),
            "--task-id",
            TASK_ID,
            "--record-id",
            RECORD_ID,
        ],
        true,
    );
    let status = run_cli(
        [
            "--json",
            "--instance",
            INSTANCE,
            "record",
            "status",
            "--state-dir",
            state_dir.to_str().unwrap(),
        ],
        true,
    );
    set_missing_config_env();

    assert_eq!(start.exit_code(), 0, "{}", start.envelope_json());
    assert_eq!(status.exit_code(), 0, "{}", status.envelope_json());
    let data = status.envelope.data.as_ref().unwrap();
    assert_eq!(
        data.pointer("/record/record_id").and_then(Value::as_str),
        Some(RECORD_ID),
        "{data}"
    );
    assert_eq!(
        data.pointer("/lab/status").and_then(Value::as_str),
        Some("unavailable"),
        "{data}"
    );
    assert_eq!(
        data.pointer("/lab/reason").and_then(Value::as_str),
        Some("lab_recording_unreadable"),
        "{data}"
    );
}
