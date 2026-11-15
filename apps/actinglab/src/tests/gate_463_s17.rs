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
// - `s17_with_no_runtime_the_recording_starts_holding_nothing`: no Runtime: the start answers
//   `hold {state: none, warnings: [runtime_not_answering]}`, its header has no `hold` member, and
//   the archive has nothing to release.
// - `s17_a_refused_pause_starts_nothing`: the pause is denied: the start fails, no session header
//   and no recording directory are written.
// - `s17_the_archive_releases_its_own_pause_found_by_its_reason_after_a_restart`: a second scripted
//   Runtime (a new owner epoch, the pause restored at revision 1, another instance paused by
//   another party beside it): `status`, then one `resume_scheduling` of the recording's instance
//   expecting that epoch and revision 1; the header keeps no hold.
// - `s17_a_release_that_fails_keeps_the_hold_and_names_the_resume`: the resume is denied: the
//   manifest and the archived header stand, the failure's details carry `hold {kept: true}` and
//   `next.resume` with the pause the status showed, `record status` shows `kept`; named again
//   with no Runtime answering, the archive tries the release again and answers
//   `runtime_not_running` with `next.resume` without `--expect`.
// - `s17_a_refused_stop_keeps_the_hold`: `record stop --timeout-ms 0` is refused before anything:
//   no connection, the header still active with its own hold.
// - `s17_record_start_force_takes_the_hold_over`: a second `record start --force` finds the
//   instance paused with the first recording's reason: `status` only, the new header holds that
//   pause as its own (`taken_over`), one pause in all.
// No device, no clock, no sleep; made-up aliases.

use super::gate_434_lab5b4::{Isolated, printed};
use super::*;
use actingcommand_contract::{
    InstancePauseStage, InstancePauseState, OwnerEpoch, RuntimeControlPlaneStatus,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeInstanceStatus, RuntimeOperation,
    RuntimeReceiptState, RuntimeRequest, RuntimeResult, SchedulingDrainSummary,
    SchedulingPauseExpectation, SchedulingPauseReason, SchedulingPauseScope,
    SchedulingResumeCaptureCheck, SchedulingResumeSelfCheck, SchedulingResumeTouchCheck,
};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};

/// The made-up instance the recordings are of, and another one beside it.
const INSTANCE: &str = "neutral.a";
const BESIDE: &str = "neutral.b";
const TASK: &str = "neutral_task";
const RECORD_ID: &str = "record_s17";
/// Another party's pause reason.
const OTHER: &str = "operator";
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
        json!(["runtime_not_answering"]),
        "{started}"
    );
    assert!(started["hold"]["runtime_error"].is_string(), "{started}");
    let header: Value = serde_json::from_slice(
        &fs::read(Path::new(&session).join(format!("record-{INSTANCE}.json")))
            .expect("the session header"),
    )
    .expect("JSON");
    assert!(
        header.get("hold").is_none(),
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
            ScriptedConnection::new()
                .request(status(owner_epoch, vec![instance(INSTANCE, None)]))
                .request(|request| denied(request, RuntimeErrorCode::InvalidRequest)),
        ]
    });
    let (code, refused) = place.start(RECORD_ID, &[]);
    let record = runtime.finish();

    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["outcome"]["code"], "device_error", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("started nothing")),
        "{refused}"
    );
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "pause_scheduling"])
    );
    assert!(!place.header_path().exists(), "no session header");
    assert!(
        !Path::new(&place.session())
            .join("record-artifacts")
            .join(RECORD_ID)
            .exists(),
        "no recording directory"
    );
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
    assert_eq!(
        operations(&record.connections[0]),
        wire(&["status", "resume_scheduling"])
    );
    assert_eq!(kept["outcome"]["code"], "device_error", "{kept}");
    let details = &kept["details"];
    assert_eq!(
        details["hold"],
        json!({"reason": own, "own": true, "kept": true})
    );
    let expected = json!({"owner_epoch": epoch, "revision": 1}).to_string();
    assert_eq!(
        details["next"]["resume"],
        json!(["resume", INSTANCE, "--expect", expected])
    );
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
        again["details"]["next"]["resume"],
        json!(["resume", INSTANCE])
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
