// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-7 (contracts/lab-recording.md, "A recording holds its instance"; register §1.424): a
// recording holds its instance. `record start` pauses the instance's scheduling with a reason of
// its own and keeps it in the session header; the end of the recording resumes only that pause,
// found by its reason. On actinglab's command line, against the runtime client's scripted Runtime
// on loopback in a temporary state root; every request frame of every connection is written down
// in the order the product sends it (each connection's handshake `health`, `bind_channel` first).
// - `s17_record_start_pauses_the_instance_with_its_own_reason`: one connection, `status` then
//   `pause_scheduling` of the instance with `lab-record-<the header's started_at_unix_ms>` and the
//   `pause` row's drain (300000 ms); the header keeps `{reason, own: true}`, the answer's `hold` is
//   `own` with the Runtime's `paused`.
// - `s17_an_instance_paused_by_another_party_is_held_by_that_pause_and_never_resumed`: the status
//   shows the instance paused with `operator`: `status` only, nothing paused; the header keeps
//   `{reason: operator, own: false}`, `record status` shows `foreign`; `record archive` makes no
//   connection at all, answers `release: not_own` and the header keeps no hold.
// - `s17_with_no_runtime_the_recording_starts_holding_nothing`: nothing listens (fix round 2: the
//   word `no_runtime_listening`): the start answers `hold {state: none, warnings:
//   [no_runtime_listening]}`, its header has exactly the members a header had before the slice
//   (no `hold`), and the archive has nothing to release.
// - `s17_a_refused_pause_starts_nothing`: the pause is denied: the start fails; the header it wrote
//   before the pause does not stay when there was none, and a stopped header that was there is put
//   back byte for byte (fix round 2); no recording directory; no connection beyond the two.
// - `s17_the_archive_releases_its_own_pause_found_by_its_reason_after_a_restart`: a second scripted
//   Runtime (a new owner epoch, the pause restored at revision 1, another instance paused by
//   another party beside it): `status`, then one `resume_scheduling` of the recording's instance
//   expecting that epoch and revision 1; the header keeps no hold.
// - `s17_a_release_that_fails_keeps_the_hold_and_names_the_resume`: the resume is denied: no
//   connection beyond the one; the manifest and the archived header stand, the failure's details
//   carry `hold {kept: true}` and (fix round 2) `next.release`, `record archive --record-id <id>`
//   with the instance and the state directory, never an unconditional resume; `record status`
//   shows `kept`; named again with no Runtime answering, the archive tries the release again and
//   answers `runtime_not_running` with the same `next.release`.
// - `s17_a_refused_stop_keeps_the_hold`: `record stop --timeout-ms 0` is refused before anything:
//   no connection, the header still active with its own hold.
// - `s17_record_start_force_takes_the_hold_over`: a second `record start --force` finds the
//   instance paused with the first recording's reason: `status` only, the new header holds that
//   pause as its own (`taken_over`), one pause in all.
// Fix round 2 (the coordinator's brief rd-1115/brief-463-s17-fix2.md):
// - `s17_a_lost_answer_to_the_pause_leaves_the_hold_for_record_stop`: the connection ends after
//   `pause_scheduling` was sent: the start fails (`runtime_not_running`) with `hold {reason, own:
//   true}` in its details and the header stays, active and naming that hold; the next `record
//   stop` sends `status`, then one `resume_scheduling` expecting the pause the status shows, and
//   answers `stopped` with `hold {reason, own: true, release: resumed, resumed}`; the header is
//   stopped without a hold.
// - `s17_record_reopen_carries_an_own_hold_to_the_reopened_recording`: two recordings archived
//   while nothing listened; the header (the second's) kept an own pause; `record reopen` of the
//   first writes its header with that hold, `record status` shows `kept`; its `record archive`
//   sends `status` and one `resume_scheduling` of that pause, and the header keeps no hold.
// - `s17_record_start_after_an_end_that_kept_its_hold_takes_it_over`: an archived header that kept
//   its own pause, the instance shown paused with that reason: a `record start` without `--force`
//   sends `status` only and holds the pause as its own (`taken_over`).
// - `s17_the_status_read_before_a_press_names_what_it_found`: the read every `--record` press and
//   application operation makes, each on a connection of its own: not paused,
//   `instance_not_held`; paused by another party, nothing; a status that gets no answer, and no
//   Runtime at all, `instance_hold_unread`.
// The train of stage one (rd-1115/brief-463-train-s1b.md, i: the words and the path fix round 2
// added without a test):
// - `s17_train_nothing_listening_at_the_runtime_address_starts_holding_nothing`: the runtime-info
//   names a loopback address where nothing listens any more (a scripted Runtime that stopped):
//   `record start` goes on, exit 0, `hold {state: none, warnings: [no_runtime_listening],
//   runtime_error}`, and the header holds no hold.
// - `s17_train_force_over_a_header_that_cannot_be_read_says_so`: `record start --force` over a
//   session header that is no JSON goes on, exit 0, its `hold.warnings` ending with
//   `replaced_header_unread` and `hold.replaced_header {path, error}`, `path` the header's as the
//   answer's own `path` names it.
// The fix round after the train's review (rd-1115/brief-463-train-s1-fix.md; the coordinator's
// ruling: a recording never starts unheld because the Lab could not tell):
// - `s17_fix_an_unresolved_state_root_starts_nothing`: the state root's variable is white space:
//   `record start` fails `validation_failed`, as the Lab's other commands answer it, and writes no
//   session header and no recording directory.
// - `s17_fix_a_probe_that_times_out_fails_the_start_as_a_failed_handshake`: after the client's
//   failed connect, a plain connection that timed out is "something may listen", as one that was
//   accepted (whose handshake failed): the same failure, `runtime_not_running`; only a refused one
//   is "nothing listens".
// - `s17_fix_a_handshake_that_fails_starts_nothing`: the Runtime drops the `BindChannel` and the
//   client's one reconnect meets another owner epoch: `record start` fails `runtime_not_running`,
//   writes no session header and no recording directory; the wire: the handshake, `health` on the
//   reconnect, and one plain connection that sends nothing.
// - `s17_fix_a_stop_whose_release_fails_names_the_retry`: `record stop` of a recording that holds
//   its own pause, the resume denied: the stop stands (the header stopped, its hold kept), the
//   failure's details carry `hold {kept: true}` and `next.release`, `record stop` with the instance
//   and the state directory.
// No device, no clock, no sleep; made-up aliases.

use super::gate_434_lab5b4::{Isolated, printed};
use super::*;
use crate::commands::{Probed, not_reached};
use actingcommand_contract::{
    InstancePauseStage, InstancePauseState, OwnerEpoch, RUNTIME_INFO_FILE,
    RuntimeControlPlaneStatus, RuntimeErrorCode, RuntimeErrorProjection, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeReceiptState, RuntimeRequest, RuntimeResult, SchedulingDrainSummary,
    SchedulingPauseExpectation, SchedulingPauseReason, SchedulingPauseScope,
    SchedulingResumeCaptureCheck, SchedulingResumeSelfCheck, SchedulingResumeTouchCheck,
};
use actingcommand_runtime_client::RuntimeClient;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use std::io;

/// The made-up instance the recordings are of, and another one beside it.
const INSTANCE: &str = "neutral.a";
const BESIDE: &str = "neutral.b";
const TASK: &str = "neutral_task";
const RECORD_ID: &str = "record_s17";
/// Another party's pause reason.
const OTHER: &str = "operator";
/// An own pause an earlier recording's end kept.
const KEPT: &str = "lab-record-1700000000000";
/// The requests of every connection's handshake.
const HANDSHAKE: [&str; 2] = ["health", "bind_channel"];

/// A temporary place: a missing configuration, no initiator variable, the session directory and
/// the Runtime state root a scripted Runtime may be started on (none runs there yet). The state
/// root's variable is put back first, then the isolated place's.
struct Place {
    _runtime_env: RuntimeStateEnvGuard,
    isolated: Isolated,
}

impl Place {
    fn new() -> Self {
        let isolated = Isolated::new();
        let runtime = isolated.temp.path().join("runtime");
        fs::create_dir_all(&runtime).expect("the Runtime state root");
        Self {
            _runtime_env: use_runtime_state_root(&runtime),
            isolated,
        }
    }

    fn runtime(&self) -> PathBuf {
        self.isolated.temp.path().join("runtime")
    }

    fn session(&self) -> String {
        self.isolated.path("session")
    }

    /// What `actinglab --json --instance neutral.a record <action> --state-dir <session> <more>`
    /// prints: its exit code and its line.
    fn record(&self, action: &str, more: &[&str]) -> (u8, Value) {
        let session = self.session();
        let mut line = vec![
            "--json",
            "--instance",
            INSTANCE,
            "record",
            action,
            "--state-dir",
            session.as_str(),
        ];
        line.extend(more);
        let exit = printed(&line);
        let answered = exit
            .line
            .clone()
            .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
        (exit.code, answered)
    }

    fn start(&self, record_id: &str, more: &[&str]) -> (u8, Value) {
        let mut words = vec!["--task-id", TASK, "--record-id", record_id];
        words.extend(more);
        self.record("start", &words)
    }

    fn header_path(&self) -> PathBuf {
        Path::new(&self.session()).join(format!("record-{INSTANCE}.json"))
    }

    /// The session header as written.
    fn header(&self) -> Value {
        serde_json::from_slice(&fs::read(self.header_path()).expect("the session header"))
            .expect("the session header's JSON")
    }

    /// Writes `header` as the session header; its bytes.
    fn write_header(&self, header: &Value) -> Vec<u8> {
        fs::create_dir_all(self.session()).expect("the session directory");
        let bytes = serde_json::to_vec_pretty(header).expect("the session header's JSON");
        fs::write(self.header_path(), &bytes).expect("the session header");
        bytes
    }

    /// The recording directory of `record_id`.
    fn recording_dir(&self, record_id: &str) -> PathBuf {
        Path::new(&self.session())
            .join("record-artifacts")
            .join(record_id)
    }
}

/// A session header of an earlier recording `record_id` in `status`, as the `record` command
/// writes one, with `hold` when given.
fn earlier_header(record_id: &str, status: &str, hold: Option<Value>) -> Value {
    let mut header = json!({
        "schema_version": "session-record-v0",
        "record_id": record_id,
        "task_id": TASK,
        "instance": INSTANCE,
        "status": status,
        "holder": null,
        "lease_id": null,
        "started_at_unix_ms": 1_700_000_000_000_u64,
        "updated_at_unix_ms": 1_700_000_000_000_u64,
        "steps": []
    });
    if let Some(hold) = hold {
        header["hold"] = hold;
    }
    header
}

fn instance_id() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

/// One instance's status, paused with `pause` (its reason and revision) when given.
fn instance(alias: &str, pause: Option<(&str, u64)>) -> RuntimeInstanceStatus {
    RuntimeInstanceStatus::new(alias, instance_id(), false, 0, false, false, false)
        .expect("instance status")
        .with_pause(pause.map(|(reason, revision)| InstancePauseState {
            revision,
            pause_reason: SchedulingPauseReason::new(reason).expect("a pause reason"),
            since_unix_ms: 1,
            stage: InstancePauseStage::Released,
        }))
        .expect("a paused instance's status")
}

/// The scripted answer to a `Status`: `instances` in `owner_epoch`.
fn status(
    owner_epoch: OwnerEpoch,
    instances: Vec<RuntimeInstanceStatus>,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, instances).expect("scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

fn scope() -> SchedulingPauseScope {
    SchedulingPauseScope::Instance {
        instance_alias: INSTANCE.to_owned(),
    }
}

/// The scripted answer to the instance's `PauseScheduling`: paused at revision 3, nothing drained.
fn paused(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::completed(
        request,
        RuntimeResult::SchedulingPaused {
            scope: scope(),
            revision: 3,
            drained: Some(SchedulingDrainSummary {
                finished: 0,
                cancelled: 0,
            }),
        },
    )
}

/// The scripted answer to the instance's `ResumeScheduling`: resumed, its self-check passed.
fn resumed(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::completed(
        request,
        RuntimeResult::SchedulingResumed {
            scope: scope(),
            revision: 2,
            selfcheck: Some(SchedulingResumeSelfCheck {
                capture: SchedulingResumeCaptureCheck {
                    ok: true,
                    width: Some(16),
                    height: Some(9),
                    capture_backend: Some("adb_screencap".to_owned()),
                },
                touch: SchedulingResumeTouchCheck {
                    ok: true,
                    backend: Some("adb_shell_input".to_owned()),
                    max_x: Some(16),
                    max_y: Some(9),
                    invasive: false,
                },
                failure_code: None,
            }),
        },
    )
}

/// A refusal of the Runtime: denied with `code`.
fn denied(request: &RuntimeRequest, code: RuntimeErrorCode) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(code, false).receipt_outcome(None, None),
    )
}

/// A connection whose status shows the instance not paused and whose pause is denied.
fn refusing_the_pause(owner_epoch: OwnerEpoch) -> ScriptedConnection {
    ScriptedConnection::new()
        .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
        .request(|request| denied(request, RuntimeErrorCode::InvalidRequest))
}

/// The operation of each request a connection received, as the wire spells it.
fn operations(record: &ScriptedConnectionRecord) -> Vec<String> {
    record
        .requests
        .iter()
        .map(|request| {
            json!(request.operation())["operation"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// The handshake, then `then`.
fn wire(then: &[&str]) -> Vec<String> {
    HANDSHAKE
        .iter()
        .chain(then)
        .map(|operation| (*operation).to_owned())
        .collect()
}

/// The instance pause a connection asked for: its scope, its reason and its drain.
fn pause_asked(record: &ScriptedConnectionRecord) -> (SchedulingPauseScope, String, u64) {
    record
        .requests
        .iter()
        .find_map(|request| match request.operation() {
            RuntimeOperation::PauseScheduling {
                scope,
                pause_reason,
                drain_timeout_ms,
            } => Some((
                scope.clone(),
                pause_reason.as_str().to_owned(),
                *drain_timeout_ms,
            )),
            _ => None,
        })
        .expect("a PauseScheduling")
}

/// The resume a connection asked for: its scope and the pause it expected.
fn resume_asked(
    record: &ScriptedConnectionRecord,
) -> (SchedulingPauseScope, Option<SchedulingPauseExpectation>) {
    record
        .requests
        .iter()
        .find_map(|request| match request.operation() {
            RuntimeOperation::ResumeScheduling { scope, expected } => {
                Some((scope.clone(), *expected))
            }
            _ => None,
        })
        .expect("a ResumeScheduling")
}

/// `record start` under the recording's own pause on a scripted Runtime that answers it: the
/// pause's reason, after the Runtime's record is checked.
fn start_held(place: &Place) -> String {
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(paused),
        ]
    });
    let (code, started) = place.start(RECORD_ID, &[]);
    assert_eq!(code, 0, "{started}");
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "pause_scheduling"])
    );
    let (_, reason, _) = pause_asked(&record.connections[0]);
    reason
}

#[test]
fn s17_record_start_pauses_the_instance_with_its_own_reason() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(paused),
        ]
    });
    let (code, started) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{started}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "pause_scheduling"])
    );
    let header = place.header();
    let started_at = header["started_at_unix_ms"].as_u64().expect("its start");
    let own = format!("lab-record-{started_at}");
    assert_eq!(
        pause_asked(&record.connections[0]),
        (scope(), own.clone(), 300_000),
        "the instance paused for the recording as the pause row pauses it"
    );
    assert_eq!(header["hold"], json!({"reason": own, "own": true}));
    assert_eq!(started["record"]["hold"], header["hold"], "{started}");
    assert_eq!(started["hold"]["state"], "own", "{started}");
    assert_eq!(started["hold"]["reason"], json!(own), "{started}");
    assert_eq!(
        started["hold"]["paused"]["kind"], "scheduling_paused",
        "{started}"
    );
    assert!(started["hold"].get("taken_over").is_none(), "{started}");
    assert!(started["hold"].get("warnings").is_none(), "{started}");
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["hold"], json!({"state": "own", "reason": own}));
}

#[test]
fn s17_an_instance_paused_by_another_party_is_held_by_that_pause_and_never_resumed() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![ScriptedConnection::new().request(status(
            owner_epoch,
            vec![instance(INSTANCE, Some((OTHER, 5)))],
        ))]
    });
    let (code, started) = place.start(RECORD_ID, &[]);
    assert_eq!(code, 0, "{started}");
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    let (code, archived) = place.record("archive", &[]);
    let record = runtime.finish();

    // One connection, the start's: nothing paused, and the archive asked nothing.
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the start's"
    );
    assert_eq!(record.connections.len(), 1);
    assert_eq!(operations(&record.connections[0]), wire(&["status"]));
    assert_eq!(
        started["record"]["hold"],
        json!({"reason": OTHER, "own": false}),
        "{started}"
    );
    assert_eq!(started["hold"]["state"], "foreign", "{started}");
    assert_eq!(shown["hold"], json!({"state": "foreign", "reason": OTHER}));
    assert_eq!(code, 0, "{archived}");
    assert_eq!(
        archived["hold"],
        json!({"reason": OTHER, "own": false, "release": "not_own"})
    );
    let header = place.header();
    assert_eq!(header["status"], "archived", "{header}");
    assert!(header.get("hold").is_none(), "{header}");
}

#[test]
fn s17_with_no_runtime_the_recording_starts_holding_nothing() {
    let _guard = env_lock();
    let isolated = Isolated::new();
    let session = isolated.path("session");
    let record = |action: &str, more: &[&str]| {
        let mut line = vec![
            "--json",
            "--instance",
            INSTANCE,
            "record",
            action,
            "--state-dir",
            session.as_str(),
        ];
        line.extend(more);
        let exit = printed(&line);
        (exit.code, exit.line.expect("a line"))
    };
    let (code, started) = record("start", &["--task-id", TASK, "--record-id", RECORD_ID]);
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["hold"]["state"], "none", "{started}");
    assert_eq!(
        started["hold"]["warnings"],
        json!(["no_runtime_listening"]),
        "{started}"
    );
    assert!(started["hold"]["runtime_error"].is_string(), "{started}");
    let header: Value = serde_json::from_slice(
        &fs::read(Path::new(&session).join(format!("record-{INSTANCE}.json")))
            .expect("the session header"),
    )
    .expect("JSON");
    // Fix round 2: a header without a hold has exactly the members it had before the slice.
    let members = header
        .as_object()
        .expect("the header is an object")
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        members,
        std::collections::BTreeSet::from([
            "schema_version",
            "record_id",
            "task_id",
            "instance",
            "status",
            "holder",
            "lease_id",
            "started_at_unix_ms",
            "updated_at_unix_ms",
            "steps",
        ]),
        "a header without a hold: {header}"
    );
    let (code, archived) = record("archive", &[]);
    assert_eq!(code, 0, "{archived}");
    assert!(archived.get("hold").is_none(), "{archived}");
    assert!(!isolated.runtime.exists(), "no Runtime state root was made");
}

#[test]
fn s17_a_refused_pause_starts_nothing() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            refusing_the_pause(owner_epoch),
            refusing_the_pause(owner_epoch),
        ]
    });
    // No header before: the one written before the pause does not stay.
    let (code, refused) = place.start(RECORD_ID, &[]);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["outcome"]["code"], "device_error", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("started nothing")),
        "{refused}"
    );
    assert!(!place.header_path().exists(), "no session header");
    assert!(
        !place.recording_dir(RECORD_ID).exists(),
        "no recording directory"
    );
    // A stopped header before: put back byte for byte.
    let earlier = place.write_header(&earlier_header("record_s17_earlier", "stopped", None));
    let (code, again) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{again}");
    assert_eq!(again["outcome"]["code"], "device_error", "{again}");
    assert_eq!(
        fs::read(place.header_path()).expect("the earlier header"),
        earlier,
        "the earlier header put back as it was"
    );
    assert!(
        !place.recording_dir(RECORD_ID).exists(),
        "no recording directory"
    );
    assert!(record.unscripted.is_empty(), "a connection beyond the two");
    assert_eq!(record.connections.len(), 2);
    for connection in &record.connections {
        assert_eq!(
            operations(connection),
            wire(&["status", "pause_scheduling"])
        );
    }
}

#[test]
fn s17_the_archive_releases_its_own_pause_found_by_its_reason_after_a_restart() {
    let _guard = env_lock();
    let place = Place::new();
    let own = start_held(&place);
    // The Runtime restarted: a new owner epoch, the pause restored at revision 1, and another
    // instance paused by another party.
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(
                    owner_epoch,
                    vec![
                        instance(INSTANCE, Some((own.as_str(), 1))),
                        instance(BESIDE, Some((OTHER, 4))),
                    ],
                ))
                .request(resumed),
        ]
    });
    let epoch = runtime.owner_epoch();
    let (code, archived) = place.record("archive", &[]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{archived}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "resume_scheduling"])
    );
    assert_eq!(
        resume_asked(&record.connections[0]),
        (
            scope(),
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: 1
            })
        ),
        "the recording's own pause, by the pause the status shows"
    );
    assert_eq!(archived["hold"]["reason"], json!(own), "{archived}");
    assert_eq!(archived["hold"]["own"], true, "{archived}");
    assert_eq!(archived["hold"]["release"], "resumed", "{archived}");
    assert_eq!(
        archived["hold"]["resumed"]["kind"], "scheduling_resumed",
        "{archived}"
    );
    let header = place.header();
    assert_eq!(header["status"], "archived", "{header}");
    assert!(header.get("hold").is_none(), "{header}");
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["hold"], json!({"state": "none"}));
}

#[test]
fn s17_a_release_that_fails_keeps_the_hold_and_names_the_resume() {
    let _guard = env_lock();
    let place = Place::new();
    let own = start_held(&place);
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(
                    owner_epoch,
                    vec![instance(INSTANCE, Some((own.as_str(), 1)))],
                ))
                .request(|request| denied(request, RuntimeErrorCode::LeaseBusy)),
        ]
    });
    let epoch = runtime.owner_epoch();
    let (code, kept) = place.record("archive", &[]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{kept}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "resume_scheduling"])
    );
    assert_eq!(
        resume_asked(&record.connections[0]),
        (
            scope(),
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: 1
            })
        )
    );
    assert_eq!(kept["outcome"]["code"], "device_error", "{kept}");
    let details = &kept["details"];
    assert_eq!(
        details["hold"],
        json!({"reason": own, "own": true, "kept": true})
    );
    // Fix round 2: the command that tries the release again, never an unconditional resume.
    let session = place.session();
    let release = json!([
        "record",
        "archive",
        "--record-id",
        RECORD_ID,
        "--instance",
        INSTANCE,
        "--state-dir",
        session
    ]);
    assert_eq!(details["next"], json!({"release": release}), "{kept}");
    // The archive stands: its manifest written, the header archived and keeping the hold.
    assert_eq!(details["lab"]["manifest"]["written"], true, "{kept}");
    let manifest = details["lab"]["manifest"]["path"]
        .as_str()
        .expect("the manifest's path");
    assert!(Path::new(manifest).is_file(), "{manifest}");
    let header = place.header();
    assert_eq!(header["status"], "archived", "{header}");
    assert_eq!(
        header["hold"],
        json!({"reason": own, "own": true, "kept": true})
    );
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["hold"], json!({"state": "kept", "reason": own}));

    // Named again with no Runtime answering: the release is tried again and the hold still kept.
    let missing = place.isolated.temp.path().join("no-runtime");
    let _missing = use_runtime_state_root(&missing);
    let (code, again) = place.record("archive", &["--record-id", RECORD_ID]);
    assert_eq!(code, 1, "{again}");
    assert_eq!(again["outcome"]["code"], "runtime_not_running", "{again}");
    assert_eq!(
        again["details"]["lab"]["status"], "already_archived",
        "{again}"
    );
    assert_eq!(
        again["details"]["next"],
        json!({"release": release}),
        "{again}"
    );
    assert_eq!(
        place.header()["hold"],
        json!({"reason": own, "own": true, "kept": true})
    );
}

#[test]
fn s17_a_refused_stop_keeps_the_hold() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(paused),
        ]
    });
    let (code, started) = place.start(RECORD_ID, &[]);
    assert_eq!(code, 0, "{started}");
    let (code, refused) = place.record("stop", &["--timeout-ms", "0"]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["outcome"]["code"], "validation_failed", "{refused}");
    // The start's connection only: the refused stop asked the Runtime nothing.
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the start's"
    );
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "pause_scheduling"])
    );
    let header = place.header();
    assert_eq!(header["status"], "active", "{header}");
    assert_eq!(header["hold"], started["record"]["hold"], "{header}");
    assert_eq!(header["hold"]["own"], true, "{header}");
}

#[test]
fn s17_record_start_force_takes_the_hold_over() {
    let _guard = env_lock();
    let place = Place::new();
    // The reason the first start paused with, which the second connection's status shows.
    let set = Arc::new(Mutex::new(None::<String>));
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        let seen = Arc::clone(&set);
        let shown = Arc::clone(&set);
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(move |request| {
                    if let RuntimeOperation::PauseScheduling { pause_reason, .. } =
                        request.operation()
                    {
                        *seen.lock().expect("the reason") = Some(pause_reason.as_str().to_owned());
                    }
                    paused(request)
                }),
            ScriptedConnection::new().request(move |request| {
                let reason = shown
                    .lock()
                    .expect("the reason")
                    .clone()
                    .expect("the first start paused");
                status(
                    owner_epoch,
                    vec![instance(INSTANCE, Some((reason.as_str(), 3)))],
                )(request)
            }),
        ]
    });
    let (code, first) = place.start("record_s17_first", &[]);
    assert_eq!(code, 0, "{first}");
    let (code, second) = place.start("record_s17_second", &["--force"]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{second}");
    assert!(record.unscripted.is_empty(), "a connection beyond the two");
    assert_eq!(record.connections.len(), 2);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "pause_scheduling"])
    );
    assert_eq!(operations(&record.connections[1]), wire(&["status"]));
    let (_, own, _) = pause_asked(&record.connections[0]);
    let header = place.header();
    assert_eq!(header["record_id"], "record_s17_second", "{header}");
    assert_eq!(header["hold"], json!({"reason": own, "own": true}));
    assert_eq!(
        second["hold"],
        json!({"state": "own", "reason": own, "taken_over": true})
    );
}

#[test]
fn s17_a_lost_answer_to_the_pause_leaves_the_hold_for_record_stop() {
    let _guard = env_lock();
    let place = Place::new();
    // The connection ends once the pause was sent: whether the pause was made is not known.
    let lost = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(|_| ScriptedAnswer::Close),
        ]
    });
    let (code, failed) = place.start(RECORD_ID, &[]);
    let record = lost.finish();

    assert_eq!(code, 1, "{failed}");
    assert_eq!(failed["outcome"]["code"], "runtime_not_running", "{failed}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "pause_scheduling"])
    );
    let (_, own, _) = pause_asked(&record.connections[0]);
    // The header stays as written before the pause, naming the own hold; the failure carries it.
    let header = place.header();
    assert_eq!(header["status"], "active", "{header}");
    assert_eq!(header["hold"], json!({"reason": own, "own": true}));
    assert_eq!(
        failed["details"]["hold"],
        json!({"reason": own, "own": true}),
        "{failed}"
    );
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["hold"], json!({"state": "own", "reason": own}));

    // The next record stop releases the pause the status shows with that reason.
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(
                    owner_epoch,
                    vec![instance(INSTANCE, Some((own.as_str(), 1)))],
                ))
                .request(resumed),
        ]
    });
    let epoch = runtime.owner_epoch();
    let (code, stopped) = place.record("stop", &[]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{stopped}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "resume_scheduling"])
    );
    assert_eq!(
        resume_asked(&record.connections[0]),
        (
            scope(),
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: 1
            })
        )
    );
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert_eq!(stopped["hold"]["reason"], json!(own), "{stopped}");
    assert_eq!(stopped["hold"]["own"], true, "{stopped}");
    assert_eq!(stopped["hold"]["release"], "resumed", "{stopped}");
    assert_eq!(
        stopped["hold"]["resumed"]["kind"], "scheduling_resumed",
        "{stopped}"
    );
    assert!(stopped["record"].get("hold").is_none(), "{stopped}");
    let header = place.header();
    assert_eq!(header["status"], "stopped", "{header}");
    assert!(header.get("hold").is_none(), "{header}");
}

#[test]
fn s17_record_reopen_carries_an_own_hold_to_the_reopened_recording() {
    let _guard = env_lock();
    let place = Place::new();
    // Two recordings, each started and archived while nothing listened: neither holds anything.
    for record_id in ["record_s17_a", "record_s17_b"] {
        let (code, started) = place.start(record_id, &[]);
        assert_eq!(code, 0, "{started}");
        let (code, archived) = place.record("archive", &[]);
        assert_eq!(code, 0, "{archived}");
    }
    // The header, the second recording's, kept an own pause its end could not release.
    let mut header = place.header();
    assert_eq!(header["record_id"], "record_s17_b", "{header}");
    header["hold"] = json!({"reason": KEPT, "own": true, "kept": true});
    place.write_header(&header);

    // The first recording reopened: the header it writes names that own pause, and asks nothing.
    let (code, reopened) = place.record("reopen", &["--record-id", "record_s17_a"]);
    assert_eq!(code, 0, "{reopened}");
    let header = place.header();
    assert_eq!(header["record_id"], "record_s17_a", "{header}");
    assert_eq!(header["status"], "active", "{header}");
    assert_eq!(
        header["hold"],
        json!({"reason": KEPT, "own": true, "kept": true})
    );
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["hold"], json!({"state": "kept", "reason": KEPT}));

    // The reopened recording's end releases it by its reason.
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(
                    owner_epoch,
                    vec![instance(INSTANCE, Some((KEPT, 2)))],
                ))
                .request(resumed),
        ]
    });
    let epoch = runtime.owner_epoch();
    let (code, archived) = place.record("archive", &[]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{archived}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "resume_scheduling"])
    );
    assert_eq!(
        resume_asked(&record.connections[0]),
        (
            scope(),
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: 2
            })
        )
    );
    assert_eq!(archived["hold"]["reason"], KEPT, "{archived}");
    assert_eq!(archived["hold"]["release"], "resumed", "{archived}");
    let header = place.header();
    assert_eq!(header["record_id"], "record_s17_a", "{header}");
    assert_eq!(header["status"], "archived", "{header}");
    assert!(header.get("hold").is_none(), "{header}");
}

#[test]
fn s17_record_start_after_an_end_that_kept_its_hold_takes_it_over() {
    let _guard = env_lock();
    let place = Place::new();
    place.write_header(&earlier_header(
        "record_s17_earlier",
        "archived",
        Some(json!({"reason": KEPT, "own": true, "kept": true})),
    ));
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![ScriptedConnection::new().request(status(
            owner_epoch,
            vec![instance(INSTANCE, Some((KEPT, 2)))],
        ))]
    });
    // No --force: the earlier recording is not active.
    let (code, started) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 0, "{started}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(operations(&record.connections[0]), wire(&["status"]));
    let header = place.header();
    assert_eq!(header["record_id"], RECORD_ID, "{header}");
    assert_eq!(header["hold"], json!({"reason": KEPT, "own": true}));
    assert_eq!(
        started["hold"],
        json!({"state": "own", "reason": KEPT, "taken_over": true})
    );
}

#[test]
fn s17_the_status_read_before_a_press_names_what_it_found() {
    let _guard = env_lock();
    let place = Place::new();
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new().request(status(owner_epoch, vec![instance(INSTANCE, None)])),
            ScriptedConnection::new().request(status(
                owner_epoch,
                vec![instance(INSTANCE, Some((OTHER, 1)))],
            )),
            ScriptedConnection::new().request(|_| ScriptedAnswer::Close),
        ]
    });
    let not_held = crate::commands::hold_warning_read(INSTANCE);
    let held = crate::commands::hold_warning_read(INSTANCE);
    let unanswered = crate::commands::hold_warning_read(INSTANCE);
    let record = runtime.finish();

    assert_eq!(not_held, Some("instance_not_held"));
    assert_eq!(held, None, "another party's pause holds the instance");
    assert_eq!(unanswered, Some("instance_hold_unread"));
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the three"
    );
    assert_eq!(record.connections.len(), 3);
    for connection in &record.connections {
        assert_eq!(operations(connection), wire(&["status"]));
    }
    // No Runtime at all: whether the instance is held is not known either.
    let missing = place.isolated.temp.path().join("no-runtime");
    let _missing = use_runtime_state_root(&missing);
    assert_eq!(
        crate::commands::hold_warning_read(INSTANCE),
        Some("instance_hold_unread")
    );
}

#[test]
fn s17_train_nothing_listening_at_the_runtime_address_starts_holding_nothing() {
    let _guard = env_lock();
    let place = Place::new();
    // The installation's runtime-info names a loopback address where nothing listens any more: a
    // scripted Runtime that took no connection and stopped.
    let gone = ScriptedRuntime::start(&place.runtime(), |_| Vec::new());
    let record = gone.finish();
    assert!(record.connections.is_empty());
    assert!(record.unscripted.is_empty());
    assert!(place.runtime().join(RUNTIME_INFO_FILE).is_file());

    let (code, started) = place.start(RECORD_ID, &[]);
    assert_eq!(code, 0, "{started}");
    assert_eq!(started["hold"]["state"], "none", "{started}");
    assert_eq!(
        started["hold"]["warnings"],
        json!(["no_runtime_listening"]),
        "{started}"
    );
    assert!(started["hold"]["runtime_error"].is_string(), "{started}");
    assert!(started["record"].get("hold").is_none(), "{started}");
    let header = place.header();
    assert_eq!(header["record_id"], RECORD_ID, "{header}");
    assert_eq!(header["status"], "active", "{header}");
    assert!(header.get("hold").is_none(), "{header}");
}

#[test]
fn s17_train_force_over_a_header_that_cannot_be_read_says_so() {
    let _guard = env_lock();
    let place = Place::new();
    fs::create_dir_all(place.session()).expect("the session directory");
    fs::write(place.header_path(), b"not a session header").expect("an unreadable header");

    let (code, started) = place.start(RECORD_ID, &["--force"]);
    assert_eq!(code, 0, "{started}");
    // Nothing listens here (no runtime-info), and the replaced header could not be read.
    assert_eq!(
        started["hold"]["warnings"],
        json!(["no_runtime_listening", "replaced_header_unread"]),
        "{started}"
    );
    assert_eq!(
        started["hold"]["replaced_header"]["path"], started["path"],
        "{started}"
    );
    assert!(
        started["hold"]["replaced_header"]["error"].is_string(),
        "{started}"
    );
    let header = place.header();
    assert_eq!(header["record_id"], RECORD_ID, "{header}");
    assert_eq!(header["status"], "active", "{header}");
    assert!(header.get("hold").is_none(), "{header}");
}

#[test]
fn s17_fix_an_unresolved_state_root_starts_nothing() {
    let _guard = env_lock();
    let place = Place::new();
    // The variable names no state root: the caller's mistake, not "no Runtime".
    let _blank = use_runtime_state_root(Path::new(" "));
    let (code, refused) = place.start(RECORD_ID, &[]);

    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["outcome"]["code"], "validation_failed", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("started nothing")),
        "{refused}"
    );
    assert!(!place.header_path().exists(), "no session header");
    assert!(
        !place.recording_dir(RECORD_ID).exists(),
        "no recording directory"
    );
}

#[test]
fn s17_fix_a_probe_that_times_out_fails_the_start_as_a_failed_handshake() {
    let _guard = env_lock();
    let place = Place::new();
    // A failed connect of the client, as `record start` meets one (its state root holds no
    // runtime-info here; the error is all that matters).
    let Err(error) = RuntimeClient::connect(crate::lab_client_config(place.runtime())) else {
        panic!("a connect to a state root without a runtime-info succeeded");
    };
    let timed_out = Probed::of_connection(Err(io::Error::from(io::ErrorKind::TimedOut)));
    let accepted = Probed::of_connection(Ok(()));
    let refused = Probed::of_connection(Err(io::Error::from(io::ErrorKind::ConnectionRefused)));
    assert_eq!(timed_out, Probed::MayListen);
    assert_eq!(accepted, Probed::MayListen);
    assert_eq!(refused, Probed::NothingListens);

    // A timed-out connection fails the start as one whose handshake failed: one path, one code.
    let Err(after_timeout) = not_reached(error.clone(), timed_out) else {
        panic!("a timed-out connection was taken for nothing listening");
    };
    let Err(after_handshake) = not_reached(error.clone(), accepted) else {
        panic!("an accepted connection was taken for nothing listening");
    };
    assert_eq!(after_timeout.code, "runtime_not_running", "{after_timeout}");
    assert_eq!(after_timeout.code, after_handshake.code);
    assert_eq!(after_timeout.class, after_handshake.class);
    // Only a refused connection is nothing listening, with the connect's own failure as its text.
    assert_eq!(
        not_reached(error.clone(), refused).ok(),
        Some(error.to_string())
    );
}

#[test]
fn s17_fix_a_handshake_that_fails_starts_nothing() {
    let _guard = env_lock();
    let place = Place::new();
    let other = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("an owner epoch")
        .transport();
    // The Runtime drops the `BindChannel`; the client's one reconnect meets another owner epoch.
    let runtime = ScriptedRuntime::start(&place.runtime(), |_| {
        vec![
            ScriptedConnection::dropping_bind(),
            ScriptedConnection::in_other_epoch(other),
        ]
    });
    let (code, failed) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{failed}");
    assert_eq!(failed["outcome"]["code"], "runtime_not_running", "{failed}");
    assert!(
        failed["message"]
            .as_str()
            .is_some_and(|message| message.contains("started nothing")),
        "{failed}"
    );
    assert!(!place.header_path().exists(), "no session header");
    assert!(
        !place.recording_dir(RECORD_ID).exists(),
        "no recording directory"
    );
    // The handshake, then `health` on the reconnect, and nothing more on either.
    assert_eq!(record.connections.len(), 2);
    assert_eq!(operations(&record.connections[0]), wire(&[]));
    assert_eq!(operations(&record.connections[1]), ["health"]);
    // The plain connection that tells whether anything listens: accepted, nothing sent on it.
    assert_eq!(record.unscripted.len(), 1, "{:?}", record.unscripted);
    assert!(
        record.unscripted[0].requests.is_empty(),
        "{:?}",
        record.unscripted
    );
}

#[test]
fn s17_fix_a_stop_whose_release_fails_names_the_retry() {
    let _guard = env_lock();
    let place = Place::new();
    let own = start_held(&place);
    let runtime = ScriptedRuntime::start(&place.runtime(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(
                    owner_epoch,
                    vec![instance(INSTANCE, Some((own.as_str(), 1)))],
                ))
                .request(|request| denied(request, RuntimeErrorCode::LeaseBusy)),
        ]
    });
    let (code, kept) = place.record("stop", &[]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{kept}");
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    assert_eq!(record.connections.len(), 1);
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "resume_scheduling"])
    );
    assert_eq!(kept["outcome"]["code"], "device_error", "{kept}");
    let details = &kept["details"];
    // The stop stands; its hold is kept, and the retry is `record stop`, never an unconditional
    // resume.
    assert_eq!(details["status"], "stopped", "{kept}");
    assert_eq!(
        details["hold"],
        json!({"reason": own, "own": true, "kept": true})
    );
    let session = place.session();
    let release = json!([
        "record",
        "stop",
        "--instance",
        INSTANCE,
        "--state-dir",
        session
    ]);
    assert_eq!(details["next"], json!({"release": release}), "{kept}");
    let header = place.header();
    assert_eq!(header["status"], "stopped", "{header}");
    assert_eq!(
        header["hold"],
        json!({"reason": own, "own": true, "kept": true})
    );
    let (code, shown) = place.record("status", &[]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["hold"], json!({"state": "kept", "reason": own}));
}
